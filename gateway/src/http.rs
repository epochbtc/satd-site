//! The public HTTP surface.
//!
//! | Route | Answers |
//! |---|---|
//! | `GET /healthz` | 200 while the process runs |
//! | `GET /readyz` | 200 when every network has a fresh snapshot, else 503 |
//! | `GET /api/v1/networks` | the networks served |
//! | `GET /api/v1/{network}/status` | that network's node status snapshot |
//! | `GET /`, `/about`, `/signet`, `/signet/about`, ... | a page of the built frontend, filled in from the cache ([`crate::page`]) |
//! | `GET /_app/...` and other files | the built frontend's scripts, styles and images |
//!
//! Every `/api/` request and every page is charged to its client's bucket;
//! scripts and styles are not, since each page view needs them. No request
//! reaches the node: handlers read the snapshot cache only.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION, REFERRER_POLICY, RETRY_AFTER,
    X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;
use tower::ServiceExt as _;
use tower_http::services::ServeDir;

use crate::config::SiteConfig;
use crate::limit::{ClientKey, Decision, RateLimiter};
use crate::page::{self, Embedded, Pages, Values};
use crate::status::StatusCache;

#[derive(Debug)]
pub struct AppState {
    pub networks: Vec<Arc<StatusCache>>,
    pub limiter: RateLimiter,
    pub trusted_proxies: Vec<IpAddr>,
    pub site: SiteConfig,
    /// The built frontend, when one is configured.
    pub frontend: Option<Frontend>,
}

#[derive(Debug)]
pub struct Frontend {
    pub dir: std::path::PathBuf,
    pub pages: Pages,
}

impl AppState {
    fn network(&self, name: &str) -> Option<&Arc<StatusCache>> {
        self.networks.iter().find(|n| n.network() == name)
    }
}

/// The address a request is limited by.
///
/// The socket's peer, unless that peer is a trusted proxy: then the
/// rightmost `X-Forwarded-For` entry that is not itself a trusted proxy,
/// which is the address the outermost trusted proxy saw. Entries further
/// left are whatever the client chose to send and are never believed.
pub fn client_addr(peer: IpAddr, headers: &HeaderMap, trusted: &[IpAddr]) -> IpAddr {
    let canonical = |a: IpAddr| match a {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(a, IpAddr::V4),
        v4 => v4,
    };
    let is_trusted = |a: IpAddr| trusted.iter().any(|t| canonical(*t) == canonical(a));
    if !is_trusted(peer) {
        return peer;
    }
    let forwarded = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect::<Vec<_>>();
    for entry in forwarded.iter().rev() {
        match entry.parse::<IpAddr>() {
            Ok(addr) if is_trusted(addr) => continue,
            Ok(addr) => return addr,
            // An entry that is not an address ends the chain we can trust.
            Err(_) => break,
        }
    }
    peer
}

/// Charge one request to its client: `Some` is the 429 to send instead.
fn over_limit(state: &AppState, peer: SocketAddr, headers: &HeaderMap) -> Option<Response> {
    let addr = client_addr(peer.ip(), headers, &state.trusted_proxies);
    match state.limiter.check(ClientKey::from(addr)) {
        Decision::Allowed => None,
        Decision::Limited { retry_after } => {
            let secs = retry_after.as_secs_f64().ceil().max(1.0) as u64;
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "rate limited", "retry_after_secs": secs})),
            )
                .into_response();
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(secs));
            Some(response)
        }
    }
}

async fn rate_limit(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    match over_limit(&state, peer, request.headers()) {
        None => next.run(request).await,
        Some(limited) => limited,
    }
}

async fn common_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

async fn healthz() -> &'static str {
    "ok\n"
}

async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    let ready = state
        .networks
        .iter()
        .all(|n| n.get().is_some_and(|s| !s.stale));
    if ready {
        "ready\n".into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response()
    }
}

async fn networks(State(state): State<Arc<AppState>>) -> Response {
    let names: Vec<&str> = state.networks.iter().map(|n| n.network()).collect();
    Json(json!({"networks": names})).into_response()
}

async fn status(State(state): State<Arc<AppState>>, Path(network): Path<String>) -> Response {
    let Some(cache) = state.network(&network) else {
        return api_error(StatusCode::NOT_FOUND, "no such network");
    };
    match cache.get() {
        Some(snapshot) => {
            let mut response = Json(&*snapshot).into_response();
            // A snapshot is at most one poll old; let a proxy hold it briefly.
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("public, max-age=5"));
            response
        }
        None => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the node has not been reached yet",
        ),
    }
}

async fn api_not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "no such endpoint")
}

fn html_response(status: StatusCode, html: String) -> Response {
    let mut response = (status, html).into_response();
    let h = response.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    // Each view is filled from the cache at that moment.
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // The pages carry their own policy for scripts and styles (a <meta>
    // the frontend build writes); these are the parts a <meta> cannot set.
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'; base-uri 'self'; object-src 'none'"),
    );
    h.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

fn render(
    state: &AppState,
    template: &page::Template,
    network: &str,
    status: StatusCode,
) -> Response {
    let snapshot = state.network(network).and_then(|c| c.get());
    let embedded = Embedded {
        network,
        networks: state.networks.iter().map(|n| n.network()).collect(),
        site: &state.site,
        methods: page::methods(),
        status: snapshot.as_deref(),
    };
    html_response(status, template.render(&Values::new(&embedded)))
}

fn not_found_page(state: &AppState, frontend: &Frontend, path: &str) -> Response {
    let Some(template) = frontend.pages.not_found() else {
        return (StatusCode::NOT_FOUND, "not found\n").into_response();
    };
    let (network, _) = page::split_network(path);
    let network = if state.network(network).is_some() {
        network
    } else {
        state.networks.first().map_or("main", |n| n.network())
    };
    render(state, template, network, StatusCode::NOT_FOUND)
}

fn redirect(to: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(v) = HeaderValue::from_str(to) {
        response.headers_mut().insert(LOCATION, v);
    }
    response
}

/// Everything outside `/api/` and the health routes: the built frontend.
async fn frontend(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let Some(frontend) = &state.frontend else {
        return (StatusCode::NOT_FOUND, "not found\n").into_response();
    };
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let uri: Uri = request.uri().clone();
    let path = uri.path().trim_start_matches('/');

    // One address per page: `/about/` is `/about`.
    if let Some(bare) = path.strip_suffix('/') {
        return redirect(&format!("/{}", bare.trim_end_matches('/')));
    }

    let is_page = frontend.pages.get(path).is_some();
    // A built page is a template: it is only ever sent filled in.
    if is_page || path.ends_with(".html") || !path.rsplit('/').next().unwrap_or("").contains('.') {
        if let Some(limited) = over_limit(&state, peer, request.headers()) {
            return limited;
        }
        if !is_page {
            return not_found_page(&state, frontend, path);
        }
        let (network, page_name) = page::split_network(path);
        if state.network(network).is_none() {
            // Mainnet is the root; a gateway without it sends the root's
            // pages to the first network it serves.
            return match (network, state.networks.first()) {
                ("main", Some(first)) => {
                    redirect(&format!("/{}", page::page_path(first.network(), page_name)))
                }
                _ => not_found_page(&state, frontend, path),
            };
        }
        let Some(template) = frontend.pages.get(path) else {
            return not_found_page(&state, frontend, path);
        };
        return render(&state, template, network, StatusCode::OK);
    }

    // A script, stylesheet or image.
    let immutable = path.starts_with("_app/immutable/");
    let headers = request.headers().clone();
    let mut response = match ServeDir::new(&frontend.dir).oneshot(request).await {
        Ok(r) => r.into_response(),
        Err(never) => match never {},
    };
    if response.status() == StatusCode::NOT_FOUND {
        if let Some(limited) = over_limit(&state, peer, &headers) {
            return limited;
        }
        return not_found_page(&state, frontend, path);
    }
    if response.status().is_success() {
        let cache = if immutable {
            "public, max-age=31536000, immutable"
        } else {
            "public, max-age=300"
        };
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    }
    response
}

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/networks", get(networks))
        .route("/{network}/status", get(status))
        .fallback(api_not_found)
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .nest("/api/v1", api)
        .fallback(frontend)
        .layer(middleware::from_fn(common_headers))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(xff: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in xff {
            h.append("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        h
    }
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_untrusted_peer_cannot_name_its_own_address() {
        let got = client_addr(
            ip("198.51.100.4"),
            &headers(&["203.0.113.9"]),
            &[ip("127.0.0.1")],
        );
        assert_eq!(got, ip("198.51.100.4"));
    }

    #[test]
    fn a_trusted_proxy_is_believed_for_its_own_entry_only() {
        let trusted = [ip("127.0.0.1")];
        // The client sent a forged entry; the proxy appended the real one.
        let got = client_addr(
            ip("127.0.0.1"),
            &headers(&["10.9.9.9, 203.0.113.9"]),
            &trusted,
        );
        assert_eq!(got, ip("203.0.113.9"));
        // The same across two header lines.
        let got = client_addr(
            ip("127.0.0.1"),
            &headers(&["10.9.9.9", "203.0.113.9"]),
            &trusted,
        );
        assert_eq!(got, ip("203.0.113.9"));
    }

    #[test]
    fn a_chain_of_trusted_proxies_is_walked_from_the_right() {
        let trusted = [ip("127.0.0.1"), ip("192.0.2.1")];
        let got = client_addr(
            ip("127.0.0.1"),
            &headers(&["203.0.113.9, 192.0.2.1"]),
            &trusted,
        );
        assert_eq!(got, ip("203.0.113.9"));
    }

    #[test]
    fn a_trusted_proxy_with_no_usable_header_is_limited_as_itself() {
        let trusted = [ip("127.0.0.1")];
        assert_eq!(
            client_addr(ip("127.0.0.1"), &headers(&[]), &trusted),
            ip("127.0.0.1")
        );
        assert_eq!(
            client_addr(
                ip("127.0.0.1"),
                &headers(&["203.0.113.9, unknown"]),
                &trusted
            ),
            ip("127.0.0.1")
        );
    }

    #[test]
    fn an_ipv4_mapped_peer_matches_its_ipv4_proxy_entry() {
        let got = client_addr(
            ip("::ffff:127.0.0.1"),
            &headers(&["203.0.113.9"]),
            &[ip("127.0.0.1")],
        );
        assert_eq!(got, ip("203.0.113.9"));
    }
}
