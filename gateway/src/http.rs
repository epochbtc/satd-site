//! The public HTTP surface.
//!
//! | Route | Answers |
//! |---|---|
//! | `GET /healthz` | 200 while the process runs |
//! | `GET /readyz` | 200 when every network has a fresh snapshot, else 503 |
//! | `GET /api/v1/networks` | the networks served |
//! | `GET /api/v1/{network}/status` | that network's node status snapshot |
//! | anything else | a file of the built frontend, if one is configured |
//!
//! Every `/api/` request is charged to its client's bucket. No request
//! reaches the node: handlers read the snapshot cache only.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::header::{CACHE_CONTROL, RETRY_AFTER, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};

use crate::limit::{ClientKey, Decision, RateLimiter};
use crate::status::StatusCache;

#[derive(Debug)]
pub struct AppState {
    pub networks: Vec<Arc<StatusCache>>,
    pub limiter: RateLimiter,
    pub trusted_proxies: Vec<IpAddr>,
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

async fn rate_limit(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let addr = client_addr(peer.ip(), request.headers(), &state.trusted_proxies);
    match state.limiter.check(ClientKey::from(addr)) {
        Decision::Allowed => next.run(request).await,
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
            response
        }
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

pub fn router(state: Arc<AppState>, static_dir: Option<&std::path::Path>) -> Router {
    let api = Router::new()
        .route("/networks", get(networks))
        .route("/{network}/status", get(status))
        .fallback(api_not_found)
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit));

    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .nest("/api/v1", api);
    if let Some(dir) = static_dir {
        // The frontend is a single-page app: an unknown path gets its shell.
        let files = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
        app = app.fallback_service(files);
    }
    app.layer(middleware::from_fn(common_headers))
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
