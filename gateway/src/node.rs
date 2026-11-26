//! The gateway's client for the node's JSON-RPC.
//!
//! The only way to call the node is [`NodeClient::call`], which takes a
//! [`Method`]. The enum is the allowlist: a method that is not a variant
//! cannot be sent, whatever a visitor asks for.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::config::NetworkConfig;

/// Every node method the gateway may call. Read-only, and each one's answer
/// is projected field by field before anything is served (see `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    GetBlockchainInfo,
    GetNetworkInfo,
    Uptime,
}

impl Method {
    pub const ALL: [Method; 3] = [
        Method::GetBlockchainInfo,
        Method::GetNetworkInfo,
        Method::Uptime,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Method::GetBlockchainInfo => "getblockchaininfo",
            Method::GetNetworkInfo => "getnetworkinfo",
            Method::Uptime => "uptime",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("node unreachable: {0}")]
    Transport(String),
    #[error("node credentials: {0}")]
    Credentials(String),
    #[error("node answered HTTP {0}")]
    Http(u16),
    #[error("node error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("node answer malformed: {0}")]
    Malformed(String),
}

#[derive(Debug, Clone)]
enum Auth {
    None,
    /// Read on every call: the node writes a new cookie at each start.
    CookieFile(PathBuf),
    Bearer(String),
}

#[derive(Debug, Clone)]
pub struct NodeClient {
    http: reqwest::Client,
    url: String,
    auth: Auth,
}

/// A node answer larger than this is refused unread. The methods on the
/// allowlist answer in a few kilobytes.
const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;

impl NodeClient {
    pub fn new(net: &NetworkConfig) -> Result<Self, NodeError> {
        let auth = match (&net.rpc_cookie_file, &net.rpc_token_file) {
            (Some(path), None) => Auth::CookieFile(path.clone()),
            (None, Some(path)) => {
                let token = std::fs::read_to_string(path)
                    .map_err(|e| NodeError::Credentials(format!("{}: {e}", path.display())))?;
                let token = token.trim().to_owned();
                if token.is_empty() {
                    return Err(NodeError::Credentials(format!("{}: empty", path.display())));
                }
                Auth::Bearer(token)
            }
            (None, None) => Auth::None,
            (Some(_), Some(_)) => {
                return Err(NodeError::Credentials(
                    "both a cookie file and a token file".into(),
                ));
            }
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| NodeError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            url: net.rpc_url.clone(),
            auth,
        })
    }

    async fn authorization(&self) -> Result<Option<String>, NodeError> {
        match &self.auth {
            Auth::None => Ok(None),
            Auth::Bearer(token) => Ok(Some(format!("Bearer {token}"))),
            Auth::CookieFile(path) => {
                let cookie = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| NodeError::Credentials(format!("{}: {e}", path.display())))?;
                let encoded = base64::engine::general_purpose::STANDARD.encode(cookie.trim());
                Ok(Some(format!("Basic {encoded}")))
            }
        }
    }

    /// Call one allowlisted method with no parameters.
    pub async fn call(&self, method: Method) -> Result<Value, NodeError> {
        let body = json!({"jsonrpc": "2.0", "id": 0, "method": method.name(), "params": []});
        let mut request = self.http.post(&self.url).json(&body);
        if let Some(value) = self.authorization().await? {
            request = request.header(reqwest::header::AUTHORIZATION, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| NodeError::Transport(e.without_url().to_string()))?;
        let status = response.status();
        // Core answers an RPC error with a non-2xx status and a JSON body,
        // so the body is read first and the status judged after.
        let bytes = read_capped(response).await?;
        let answer: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) if !status.is_success() => return Err(NodeError::Http(status.as_u16())),
            Err(e) => return Err(NodeError::Malformed(e.to_string())),
        };
        if let Some(error) = answer.get("error").filter(|e| !e.is_null()) {
            return Err(NodeError::Rpc {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        if !status.is_success() {
            return Err(NodeError::Http(status.as_u16()));
        }
        match answer.get("result") {
            Some(result) => Ok(result.clone()),
            None => Err(NodeError::Malformed("no result".into())),
        }
    }
}

async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, NodeError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| NodeError::Transport(e.without_url().to_string()))?
    {
        if bytes.len() + chunk.len() > MAX_ANSWER_BYTES {
            return Err(NodeError::Malformed(format!(
                "answer over {MAX_ANSWER_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The allowlist holds read-only methods only. A method that changes
    /// the node, or one whose answer the gateway does not project, must not
    /// be added without changing this test deliberately.
    #[test]
    fn allowlist_is_exactly_the_read_only_status_methods() {
        let names: Vec<_> = Method::ALL.iter().map(|m| m.name()).collect();
        assert_eq!(names, ["getblockchaininfo", "getnetworkinfo", "uptime"]);
    }
}
