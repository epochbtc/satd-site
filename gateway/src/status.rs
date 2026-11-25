//! The node status snapshot: what the home page shows about the node.
//!
//! A poller asks the node on a timer and stores the projected answer. A
//! request is answered from the stored snapshot and never reaches the node.
//! When the node cannot be asked, the last snapshot stays and is marked
//! stale, so the site degrades to old numbers instead of errors.

use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::node::{Method, NodeClient, NodeError};

/// The node's status as served. Every field is copied out of the node's
/// answer by name: nothing else the node says is passed on. In particular
/// `getnetworkinfo`'s `localaddresses` (the node's own addresses) and its
/// warnings are left behind.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NodeStatus {
    pub chain: String,
    pub height: u64,
    pub headers: u64,
    pub best_block_hash: String,
    pub initial_block_download: bool,
    /// The node's user agent, `/satd:0.8.0/`.
    pub user_agent: String,
    pub protocol_version: u64,
    pub connections: u64,
    pub connections_in: u64,
    pub connections_out: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub network: String,
    /// When the node last answered, Unix seconds.
    pub fetched_at: u64,
    /// The node did not answer the latest poll: `node` is from `fetched_at`.
    pub stale: bool,
    pub node: NodeStatus,
}

fn field<'a>(value: &'a Value, method: Method, name: &str) -> Result<&'a Value, NodeError> {
    value
        .get(name)
        .ok_or_else(|| NodeError::Malformed(format!("{}: no {name}", method.name())))
}

fn string(value: &Value, method: Method, name: &str) -> Result<String, NodeError> {
    field(value, method, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| NodeError::Malformed(format!("{}: {name} is not a string", method.name())))
}

fn number(value: &Value, method: Method, name: &str) -> Result<u64, NodeError> {
    field(value, method, name)?
        .as_u64()
        .ok_or_else(|| NodeError::Malformed(format!("{}: {name} is not a count", method.name())))
}

impl NodeStatus {
    pub fn project(chain: &Value, network: &Value, uptime: &Value) -> Result<Self, NodeError> {
        use Method::{GetBlockchainInfo as C, GetNetworkInfo as N};
        Ok(Self {
            chain: string(chain, C, "chain")?,
            height: number(chain, C, "blocks")?,
            headers: number(chain, C, "headers")?,
            best_block_hash: string(chain, C, "bestblockhash")?,
            initial_block_download: field(chain, C, "initialblockdownload")?
                .as_bool()
                .ok_or_else(|| {
                    NodeError::Malformed(
                        "getblockchaininfo: initialblockdownload is not a boolean".into(),
                    )
                })?,
            user_agent: string(network, N, "subversion")?,
            protocol_version: number(network, N, "protocolversion")?,
            connections: number(network, N, "connections")?,
            connections_in: number(network, N, "connections_in")?,
            connections_out: number(network, N, "connections_out")?,
            uptime_secs: uptime
                .as_u64()
                .ok_or_else(|| NodeError::Malformed("uptime: not a count".into()))?,
        })
    }

    pub async fn fetch(client: &NodeClient) -> Result<Self, NodeError> {
        let chain = client.call(Method::GetBlockchainInfo).await?;
        let network = client.call(Method::GetNetworkInfo).await?;
        let uptime = client.call(Method::Uptime).await?;
        Self::project(&chain, &network, &uptime)
    }
}

/// One network's stored snapshot. `None` until the node has answered once.
#[derive(Debug)]
pub struct StatusCache {
    network: String,
    inner: RwLock<Option<Arc<Snapshot>>>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl StatusCache {
    pub fn new(network: impl Into<String>) -> Self {
        Self {
            network: network.into(),
            inner: RwLock::new(None),
        }
    }

    pub fn network(&self) -> &str {
        &self.network
    }

    pub fn get(&self) -> Option<Arc<Snapshot>> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn store(&self, node: NodeStatus) {
        let snapshot = Snapshot {
            network: self.network.clone(),
            fetched_at: unix_now(),
            stale: false,
            node,
        };
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(snapshot));
    }

    /// The node did not answer: keep what is stored and mark it stale.
    pub fn mark_stale(&self) {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = guard.as_ref()
            && !current.stale
        {
            let mut snapshot = (**current).clone();
            snapshot.stale = true;
            *guard = Some(Arc::new(snapshot));
        }
    }

    /// One poll: ask the node, store or mark stale. Returns the error, if
    /// any, for the caller to log.
    pub async fn refresh(&self, client: &NodeClient) -> Result<(), NodeError> {
        match NodeStatus::fetch(client).await {
            Ok(node) => {
                self.store(node);
                Ok(())
            }
            Err(e) => {
                self.mark_stale();
                Err(e)
            }
        }
    }

    /// Poll for ever. The first poll happens at once.
    pub async fn run(self: Arc<Self>, client: NodeClient, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut failing = false;
        loop {
            ticker.tick().await;
            match self.refresh(&client).await {
                Ok(()) => {
                    if failing {
                        tracing::info!(network = %self.network, "node answering again");
                    }
                    failing = false;
                }
                Err(e) => {
                    // Logged once per outage, not once per poll.
                    if !failing {
                        tracing::warn!(network = %self.network, error = %e, "node poll failed");
                    }
                    failing = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn chain() -> Value {
        json!({"chain": "regtest", "blocks": 101, "headers": 101,
               "bestblockhash": "00".repeat(32), "initialblockdownload": false,
               "warnings": ["do not serve me"]})
    }
    pub(crate) fn network() -> Value {
        json!({"subversion": "/satd:0.8.0/", "protocolversion": 70016,
               "connections": 10, "connections_in": 2, "connections_out": 8,
               "localaddresses": [{"address": "203.0.113.7", "port": 8333, "score": 1}]})
    }

    #[test]
    fn projection_copies_named_fields_only() {
        let status = NodeStatus::project(&chain(), &network(), &json!(42)).unwrap();
        assert_eq!(status.height, 101);
        assert_eq!(status.user_agent, "/satd:0.8.0/");
        assert_eq!(status.uptime_secs, 42);
        // The node's own address and its warnings are not in what is served.
        let served = serde_json::to_string(&status).unwrap();
        assert!(!served.contains("203.0.113.7"), "{served}");
        assert!(!served.contains("do not serve me"), "{served}");
    }

    #[test]
    fn a_missing_field_is_an_error_not_a_default() {
        let mut c = chain();
        c.as_object_mut().unwrap().remove("blocks");
        let e = NodeStatus::project(&c, &network(), &json!(1)).unwrap_err();
        assert!(e.to_string().contains("blocks"), "{e}");
    }

    #[test]
    fn stale_keeps_the_last_snapshot() {
        let cache = StatusCache::new("regtest");
        assert!(cache.get().is_none());
        cache.mark_stale();
        assert!(
            cache.get().is_none(),
            "nothing to mark before a first answer"
        );
        cache.store(NodeStatus::project(&chain(), &network(), &json!(1)).unwrap());
        assert!(!cache.get().unwrap().stale);
        cache.mark_stale();
        let snapshot = cache.get().unwrap();
        assert!(snapshot.stale);
        assert_eq!(snapshot.node.height, 101);
        cache.store(NodeStatus::project(&chain(), &network(), &json!(2)).unwrap());
        assert!(!cache.get().unwrap().stale);
    }
}
