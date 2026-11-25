//! satd-site's gateway: the only thing the public reaches.
//!
//! It holds the node's credentials, calls a fixed allowlist of node methods
//! ([`node::Method`]), keeps what they answer in a snapshot cache
//! ([`status`]), and serves that cache over HTTP ([`http`]) under per-client
//! rate limits ([`limit`]). A page view never reaches the node.

pub mod config;
pub mod http;
pub mod limit;
pub mod node;
pub mod status;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use config::Config;

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Node(#[from] node::NodeError),
    #[error("cannot listen on {addr}: {source}")]
    Listen {
        addr: SocketAddr,
        source: std::io::Error,
    },
}

/// A started gateway: its bound address and the task serving it.
pub struct Running {
    pub addr: SocketAddr,
    pub state: Arc<http::AppState>,
    pub server: tokio::task::JoinHandle<std::io::Result<()>>,
}

/// Bind, start one poller per network and the bucket sweeper, and serve.
pub async fn start(config: &Config) -> Result<Running, StartError> {
    let mut networks = Vec::new();
    for net in &config.networks {
        let client = node::NodeClient::new(net)?;
        let cache = Arc::new(status::StatusCache::new(net.name.clone()));
        tokio::spawn(cache.clone().run(client, net.poll_interval()));
        networks.push(cache);
    }
    let idle = Duration::from_secs(config.limits.idle_secs);
    let state = Arc::new(http::AppState {
        networks,
        limiter: limit::RateLimiter::new(config.limits.page, idle, config.limits.max_clients),
        trusted_proxies: config.trusted_proxies.clone(),
    });
    let sweeper = state.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        loop {
            ticker.tick().await;
            sweeper.limiter.sweep_at(std::time::Instant::now());
        }
    });

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|source| StartError::Listen {
            addr: config.listen,
            source,
        })?;
    let addr = listener.local_addr().map_err(|source| StartError::Listen {
        addr: config.listen,
        source,
    })?;
    let app = http::router(state.clone(), config.static_dir.as_deref());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await
    });
    Ok(Running {
        addr,
        state,
        server,
    })
}

/// Resolves on SIGINT or SIGTERM, so a service manager's stop lets
/// requests in flight finish.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(term) => term,
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
