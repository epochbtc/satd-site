//! The gateway against a stand-in node: a small JSON-RPC server that can be
//! switched off, and that records every method it is asked and the
//! credentials it is shown.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use satd_site::config::{Bucket, Config, Limits, NetworkConfig};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeNode {
    down: AtomicBool,
    height: AtomicU64,
    calls: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
}

async fn rpc(
    State(node): State<Arc<FakeNode>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let method = body["method"].as_str().unwrap_or("").to_owned();
    node.calls.lock().unwrap().push(method.clone());
    if let Some(a) = headers.get("authorization") {
        node.authorizations
            .lock()
            .unwrap()
            .push(a.to_str().unwrap().to_owned());
    }
    if node.down.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "down").into_response();
    }
    let result = match method.as_str() {
        "getblockchaininfo" => json!({
            "chain": "regtest", "blocks": node.height.load(Ordering::SeqCst), "headers": 150,
            "bestblockhash": "ab".repeat(32), "initialblockdownload": false,
            "warnings": ["internal warning text"]}),
        "getnetworkinfo" => json!({
            "subversion": "/satd:0.8.0/", "protocolversion": 70016,
            "connections": 10, "connections_in": 2, "connections_out": 8,
            "localaddresses": [{"address": "203.0.113.7", "port": 8333, "score": 1}]}),
        "uptime" => json!(1234),
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"result": null, "error": {"code": -32601, "message": "Method not found"}, "id": 0})),
            )
                .into_response();
        }
    };
    Json(json!({"result": result, "error": null, "id": 0})).into_response()
}

async fn fake_node() -> (Arc<FakeNode>, SocketAddr) {
    let node = Arc::new(FakeNode::default());
    node.height.store(150, Ordering::SeqCst);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().route("/", post(rpc)).with_state(node.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (node, addr)
}

fn config(node: SocketAddr) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        static_dir: None,
        trusted_proxies: vec![],
        site: Default::default(),
        limits: Limits {
            page: Bucket {
                per_minute: 6000,
                burst: 1000,
            },
            ..Limits::default()
        },
        networks: vec![NetworkConfig {
            name: "regtest".into(),
            rpc_url: format!("http://{node}/"),
            rpc_cookie_file: None,
            rpc_token_file: None,
            poll_secs: 1,
        }],
    }
}

async fn get(addr: SocketAddr, path: &str) -> (u16, HeaderMap, String) {
    let response = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    (status, headers, response.text().await.unwrap())
}

/// Poll `path` until `want` holds of its body, or fail after ten seconds.
async fn wait_for(addr: SocketAddr, path: &str, want: impl Fn(u16, &str) -> bool) -> String {
    for _ in 0..200 {
        let (status, _, body) = get(addr, path).await;
        if want(status, &body) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{path} never reached the wanted state");
}

#[tokio::test]
async fn status_is_served_from_the_snapshot_with_named_fields_only() {
    let (node, node_addr) = fake_node().await;
    let gw = satd_site::start(&config(node_addr)).await.unwrap();

    let body = wait_for(gw.addr, "/api/v1/regtest/status", |s, _| s == 200).await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["network"], "regtest");
    assert_eq!(v["stale"], false);
    assert_eq!(v["node"]["height"], 150);
    assert_eq!(v["node"]["user_agent"], "/satd:0.8.0/");
    assert_eq!(v["node"]["connections_out"], 8);
    assert_eq!(v["node"]["uptime_secs"], 1234);
    // What the node said beyond the projected fields is not passed on.
    assert!(!body.contains("203.0.113.7"), "{body}");
    assert!(!body.contains("internal warning text"), "{body}");

    // The node was asked only allowlisted methods.
    let calls = node.calls.lock().unwrap().clone();
    assert!(!calls.is_empty());
    for call in &calls {
        assert!(
            ["getblockchaininfo", "getnetworkinfo", "uptime"].contains(&call.as_str()),
            "unexpected node call {call}"
        );
    }
}

#[tokio::test]
async fn requests_do_not_reach_the_node() {
    let (node, node_addr) = fake_node().await;
    let mut config = config(node_addr);
    // One poll at start, then none for the length of the test.
    config.networks[0].poll_secs = 3600;
    let gw = satd_site::start(&config).await.unwrap();
    wait_for(gw.addr, "/api/v1/regtest/status", |s, _| s == 200).await;

    let before = node.calls.lock().unwrap().len();
    for _ in 0..50 {
        let (status, _, _) = get(gw.addr, "/api/v1/regtest/status").await;
        assert_eq!(status, 200);
    }
    assert_eq!(node.calls.lock().unwrap().len(), before);
}

#[tokio::test]
async fn a_node_outage_serves_the_last_snapshot_marked_stale() {
    let (node, node_addr) = fake_node().await;
    let gw = satd_site::start(&config(node_addr)).await.unwrap();
    wait_for(gw.addr, "/api/v1/regtest/status", |s, _| s == 200).await;
    assert_eq!(get(gw.addr, "/readyz").await.0, 200);

    node.down.store(true, Ordering::SeqCst);
    let body = wait_for(gw.addr, "/api/v1/regtest/status", |s, b| {
        s == 200 && b.contains("\"stale\":true")
    })
    .await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["node"]["height"], 150, "the last answer is kept");
    assert_eq!(get(gw.addr, "/readyz").await.0, 503);
    assert_eq!(get(gw.addr, "/healthz").await.0, 200);

    node.height.store(151, Ordering::SeqCst);
    node.down.store(false, Ordering::SeqCst);
    let body = wait_for(gw.addr, "/api/v1/regtest/status", |s, b| {
        s == 200 && b.contains("\"stale\":false")
    })
    .await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["node"]["height"], 151);
}

#[tokio::test]
async fn before_the_node_answers_status_is_503_and_not_ready() {
    let (node, node_addr) = fake_node().await;
    node.down.store(true, Ordering::SeqCst);
    let gw = satd_site::start(&config(node_addr)).await.unwrap();
    // Wait until the first poll has been made and failed.
    for _ in 0..200 {
        if !node.calls.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let (status, _, body) = get(gw.addr, "/api/v1/regtest/status").await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(get(gw.addr, "/readyz").await.0, 503);
}

#[tokio::test]
async fn over_the_limit_is_429_with_retry_after_and_health_is_not_charged() {
    let (_node, node_addr) = fake_node().await;
    let mut config = config(node_addr);
    config.limits.page = Bucket {
        per_minute: 1,
        burst: 3,
    };
    let gw = satd_site::start(&config).await.unwrap();

    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(get(gw.addr, "/api/v1/networks").await.0);
    }
    assert_eq!(statuses, [200, 200, 200, 429, 429]);

    let (status, headers, body) = get(gw.addr, "/api/v1/networks").await;
    assert_eq!(status, 429);
    let retry: u64 = headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry), "retry-after {retry}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["retry_after_secs"], retry);

    // Health checks are outside the limit.
    for _ in 0..10 {
        assert_eq!(get(gw.addr, "/healthz").await.0, 200);
    }
}

#[tokio::test]
async fn unknown_network_and_unknown_endpoint_are_404_json() {
    let (_node, node_addr) = fake_node().await;
    let gw = satd_site::start(&config(node_addr)).await.unwrap();
    let (status, _, body) = get(gw.addr, "/api/v1/main/status").await;
    assert_eq!(status, 404);
    assert!(body.contains("no such network"), "{body}");
    let (status, _, body) = get(gw.addr, "/api/v1/regtest/getcrawlnodes").await;
    assert_eq!(status, 404);
    assert!(body.contains("no such endpoint"), "{body}");
    let (status, headers, body) = get(gw.addr, "/api/v1/networks").await;
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"networks":["regtest"]}"#);
    assert_eq!(headers["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn the_cookie_is_read_on_every_call() {
    let (node, node_addr) = fake_node().await;
    let dir = tempfile::tempdir().unwrap();
    let cookie = dir.path().join(".cookie");
    std::fs::write(&cookie, "__cookie__:first\n").unwrap();
    let mut config = config(node_addr);
    config.networks[0].rpc_cookie_file = Some(cookie.clone());
    let gw = satd_site::start(&config).await.unwrap();
    wait_for(gw.addr, "/api/v1/regtest/status", |s, _| s == 200).await;
    // base64("__cookie__:first")
    let first = "Basic X19jb29raWVfXzpmaXJzdA==";
    assert!(
        node.authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|a| a == first)
    );

    // The node restarts and writes a new cookie; the gateway is not restarted.
    std::fs::write(&cookie, "__cookie__:second\n").unwrap();
    let second = "Basic X19jb29raWVfXzpzZWNvbmQ=";
    for _ in 0..200 {
        if node
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .any(|a| a == second)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the new cookie was never presented");
}

#[tokio::test]
async fn a_bearer_token_file_is_presented() {
    let (node, node_addr) = fake_node().await;
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "s3cret\n").unwrap();
    let mut config = config(node_addr);
    config.networks[0].rpc_token_file = Some(token);
    let gw = satd_site::start(&config).await.unwrap();
    wait_for(gw.addr, "/api/v1/regtest/status", |s, _| s == 200).await;
    assert!(
        node.authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|a| a == "Bearer s3cret")
    );
}

/// A small built frontend: the same files and markers `web/` builds, for
/// one prefixed network (regtest) and the root.
fn fixture_frontend() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let page = |name: &str| {
        format!(
            r#"<!doctype html><html data-served="__SATD_served__"><title>{name}</title>
<p data-fresh="__SATD_fresh__">block <b>__SATD_height__</b> with __SATD_peers__, up __SATD_uptime__</p>
<p data-named="__SATD_named__">by __SATD_operator__; methods __SATD_methods__</p>
<script type="application/json" id="satd-data">__SATD_data__</script></html>"#
        )
    };
    for (file, name) in [
        ("index.html", "main home"),
        ("about.html", "main about"),
        ("regtest.html", "regtest home"),
        ("404.html", "not found"),
    ] {
        std::fs::write(dir.path().join(file), page(name)).unwrap();
    }
    std::fs::create_dir_all(dir.path().join("regtest")).unwrap();
    std::fs::write(dir.path().join("regtest/about.html"), page("regtest about")).unwrap();
    std::fs::create_dir_all(dir.path().join("_app/immutable")).unwrap();
    std::fs::write(dir.path().join("_app/immutable/app.js"), "console.log(1)").unwrap();
    std::fs::write(dir.path().join("favicon.svg"), "<svg/>").unwrap();
    std::fs::write(dir.path().join("LICENSE"), "MIT").unwrap();
    dir
}

fn no_redirects() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn embedded(body: &str) -> Value {
    let start = body.find(r#"id="satd-data">"#).unwrap() + r#"id="satd-data">"#.len();
    let end = start + body[start..].find("</script>").unwrap();
    serde_json::from_str(&body[start..end]).unwrap()
}

#[tokio::test]
async fn a_page_is_sent_with_its_markers_filled_from_the_cache() {
    let (_node, node_addr) = fake_node().await;
    let dir = fixture_frontend();
    let mut config = config(node_addr);
    config.static_dir = Some(dir.path().to_owned());
    config.site.operator = Some("Example <Ops>".into());
    let gw = satd_site::start(&config).await.unwrap();
    wait_for(gw.addr, "/api/v1/regtest/status", |s, _| s == 200).await;

    let (status, headers, body) = get(gw.addr, "/regtest").await;
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(headers["cache-control"], "no-cache");
    assert!(body.contains("<title>regtest home</title>"), "{body}");
    assert!(body.contains(r#"data-served="regtest""#), "{body}");
    assert!(body.contains(r#"data-fresh="live""#), "{body}");
    assert!(
        body.contains("block <b>150</b> with 10 peers, up 20 minutes"),
        "{body}"
    );
    assert!(body.contains("by Example &lt;Ops&gt;"), "{body}");
    assert!(
        body.contains("methods getblockchaininfo, getnetworkinfo, uptime"),
        "{body}"
    );
    assert!(!body.contains("__SATD_"), "{body}");
    // What the node said beyond the projected fields is not in the page.
    assert!(
        !body.contains("203.0.113.7") && !body.contains("internal warning"),
        "{body}"
    );

    let data = embedded(&body).await;
    assert_eq!(data["network"], "regtest");
    assert_eq!(data["networks"], json!(["regtest"]));
    assert_eq!(data["status"]["node"]["height"], 150);
    assert_eq!(data["site"]["operator"], "Example <Ops>");

    let (status, _, body) = get(gw.addr, "/regtest/about").await;
    assert_eq!(status, 200);
    assert!(body.contains("<title>regtest about</title>"), "{body}");
}

#[tokio::test]
async fn a_page_before_the_node_answers_says_so() {
    let (node, node_addr) = fake_node().await;
    node.down.store(true, Ordering::SeqCst);
    let dir = fixture_frontend();
    let mut config = config(node_addr);
    config.static_dir = Some(dir.path().to_owned());
    let gw = satd_site::start(&config).await.unwrap();
    let (status, _, body) = get(gw.addr, "/regtest").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#"data-fresh="none""#), "{body}");
    assert!(body.contains("block <b></b>"), "{body}");
    assert!(embedded(&body).await["status"].is_null());
}

#[tokio::test]
async fn a_built_page_is_never_sent_unfilled() {
    let (_node, node_addr) = fake_node().await;
    let dir = fixture_frontend();
    let mut config = config(node_addr);
    config.static_dir = Some(dir.path().to_owned());
    let gw = satd_site::start(&config).await.unwrap();
    for path in [
        "/index.html",
        "/regtest.html",
        "/regtest/about.html",
        "/404.html",
        "/nope",
        "/regtest/nope",
        "/signet",
    ] {
        let (status, _, body) = get(gw.addr, path).await;
        assert_eq!(status, 404, "{path}");
        assert!(body.contains("<title>not found</title>"), "{path}: {body}");
        assert!(!body.contains("__SATD_"), "{path}: {body}");
    }
    // An API miss is JSON, not the page, under any version.
    for path in ["/api/v1/nope", "/api", "/api/v2/regtest/status"] {
        let (status, _, body) = get(gw.addr, path).await;
        assert_eq!(status, 404, "{path}");
        assert!(body.starts_with('{'), "{path}: {body}");
    }
}

#[tokio::test]
async fn the_root_goes_to_the_first_network_when_mainnet_is_not_served() {
    let (_node, node_addr) = fake_node().await;
    let dir = fixture_frontend();
    let mut config = config(node_addr);
    config.static_dir = Some(dir.path().to_owned());
    let gw = satd_site::start(&config).await.unwrap();
    let client = no_redirects();
    for (from, to) in [
        ("/", "/regtest"),
        ("/about", "/regtest/about"),
        ("/regtest/", "/regtest"),
        ("/regtest/about/", "/regtest/about"),
    ] {
        let r = client
            .get(format!("http://{}{from}", gw.addr))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 302, "{from}");
        assert_eq!(r.headers()["location"], to, "{from}");
    }
}

#[tokio::test]
async fn the_apps_files_are_served_and_cached() {
    let (_node, node_addr) = fake_node().await;
    let dir = fixture_frontend();
    let mut config = config(node_addr);
    config.static_dir = Some(dir.path().to_owned());
    let gw = satd_site::start(&config).await.unwrap();
    let (status, headers, body) = get(gw.addr, "/_app/immutable/app.js").await;
    assert_eq!((status, body.as_str()), (200, "console.log(1)"));
    assert_eq!(
        headers["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let (status, headers, _) = get(gw.addr, "/favicon.svg").await;
    assert_eq!(status, 200);
    assert_eq!(headers["cache-control"], "public, max-age=300");
    let (status, _, body) = get(gw.addr, "/_app/immutable/gone.js").await;
    assert_eq!(status, 404);
    assert!(body.contains("<title>not found</title>"), "{body}");
    // A file without an extension is served like any other.
    let (status, _, body) = get(gw.addr, "/LICENSE").await;
    assert_eq!((status, body.as_str()), (200, "MIT"));
    // A directory is a plain miss, not a listing or a redirect.
    let r = no_redirects()
        .get(format!("http://{}/_app", gw.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);
    assert!(r.text().await.unwrap().contains("<title>not found</title>"));
}

#[tokio::test]
async fn pages_are_charged_to_the_client_and_files_are_not() {
    let (_node, node_addr) = fake_node().await;
    let dir = fixture_frontend();
    let mut config = config(node_addr);
    config.static_dir = Some(dir.path().to_owned());
    config.limits.page = Bucket {
        per_minute: 1,
        burst: 2,
    };
    config.limits.idle_secs = 120;
    let gw = satd_site::start(&config).await.unwrap();
    for _ in 0..5 {
        assert_eq!(get(gw.addr, "/_app/immutable/app.js").await.0, 200);
    }
    assert_eq!(get(gw.addr, "/regtest").await.0, 200);
    assert_eq!(get(gw.addr, "/regtest/about").await.0, 200);
    let (status, headers, _) = get(gw.addr, "/regtest").await;
    assert_eq!(status, 429);
    assert!(headers.contains_key("retry-after"));
}

/// The real build, when CI has made one: every page it holds is sent with
/// no marker left and with its data readable.
#[tokio::test]
async fn the_built_frontend_is_filled_in_on_every_page() {
    let Some(build) = std::env::var_os("SATD_SITE_WEB_BUILD") else {
        eprintln!("SATD_SITE_WEB_BUILD is not set; build web/ and set it to run this test");
        return;
    };
    let (_node, node_addr) = fake_node().await;
    let mut config = config(node_addr);
    config.static_dir = Some(build.into());
    config.check_files().unwrap();
    let gw = satd_site::start(&config).await.unwrap();
    wait_for(gw.addr, "/api/v1/regtest/status", |s, _| s == 200).await;
    for path in ["/regtest", "/regtest/about", "/nope"] {
        let (status, _, body) = get(gw.addr, path).await;
        assert_eq!(status, if path == "/nope" { 404 } else { 200 }, "{path}");
        assert!(!body.contains("__SATD_"), "{path}: {body}");
        assert_eq!(embedded(&body).await["network"], "regtest", "{path}");
        assert!(body.contains(r#"data-served="regtest""#), "{path}");
    }
    let (_, _, home) = get(gw.addr, "/regtest").await;
    assert!(home.contains(r#">150</span>"#), "{home}");
    assert!(home.contains("10 peers"), "{home}");
}
