//! The gateway's configuration: one TOML file.
//!
//! Unknown keys are an error, so a misspelt limit cannot silently leave the
//! default in force.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("config: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where the gateway listens. Loopback unless a proxy on another host
    /// fronts it.
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// The built frontend. Without it only the JSON API is served.
    #[serde(default)]
    pub static_dir: Option<PathBuf>,
    /// Proxies whose `X-Forwarded-For` is believed. A request from any
    /// other peer is limited by its socket address.
    #[serde(default)]
    pub trusted_proxies: Vec<IpAddr>,
    #[serde(default)]
    pub limits: Limits,
    /// The networks served, in the order given. At least one.
    #[serde(rename = "network")]
    pub networks: Vec<NetworkConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// The name in URLs: `main`, `signet`, `testnet4` or `regtest`.
    pub name: String,
    /// The node's JSON-RPC endpoint, `http://127.0.0.1:8332` for example.
    pub rpc_url: String,
    /// A Core-style cookie file, read again on every call so a node restart
    /// needs no gateway restart.
    #[serde(default)]
    pub rpc_cookie_file: Option<PathBuf>,
    /// A file holding a bearer token for the node, read at startup.
    #[serde(default)]
    pub rpc_token_file: Option<PathBuf>,
    /// How often the node is asked for its tip, in seconds.
    #[serde(default = "default_poll_secs")]
    pub poll_secs: u64,
}

impl NetworkConfig {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_secs)
    }
}

/// Requests a client may make. Each class is a token bucket: `burst`
/// requests at once, refilled at `per_minute`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_page")]
    pub page: Bucket,
    /// Buckets idle this long are dropped, in seconds.
    #[serde(default = "default_idle_secs")]
    pub idle_secs: u64,
    /// The most clients tracked at once. Past it, a new client is refused
    /// until idle ones are dropped, so the table cannot be grown without
    /// bound.
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bucket {
    pub per_minute: u32,
    pub burst: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            page: default_page(),
            idle_secs: default_idle_secs(),
            max_clients: default_max_clients(),
        }
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8080))
}
fn default_poll_secs() -> u64 {
    5
}
fn default_page() -> Bucket {
    Bucket {
        per_minute: 60,
        burst: 30,
    }
}
fn default_idle_secs() -> u64 {
    600
}
fn default_max_clients() -> usize {
    100_000
}

pub const NETWORK_NAMES: [&str; 4] = ["main", "signet", "testnet4", "regtest"];

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let config: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })?;
        config.finish()
    }

    /// Expand a leading `~/` in every path, then validate.
    fn finish(mut self) -> Result<Self, ConfigError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        self.expand_home(home.as_deref())?;
        self.validate()?;
        self.check_files()?;
        Ok(self)
    }

    /// What must exist on disk for the configuration to work. The file
    /// server looks a path up only when asked for it, so without this a
    /// misspelt `static_dir` would start and answer 404 to every page.
    pub fn check_files(&self) -> Result<(), ConfigError> {
        if let Some(dir) = &self.static_dir {
            let shell = dir.join("index.html");
            if !shell.is_file() {
                return Err(ConfigError::Invalid(format!(
                    "static_dir: {} is not a file (build the frontend, or remove static_dir)",
                    shell.display()
                )));
            }
        }
        Ok(())
    }

    /// Replace a leading `~/` in each configured path with `home`. Nothing
    /// else is expanded: no `~user`, no environment variables.
    pub fn expand_home(&mut self, home: Option<&Path>) -> Result<(), ConfigError> {
        let expand = |path: &mut Option<PathBuf>| -> Result<(), ConfigError> {
            let Some(p) = path.as_ref() else {
                return Ok(());
            };
            let Ok(rest) = p.strip_prefix("~") else {
                return Ok(());
            };
            let Some(home) = home else {
                return Err(ConfigError::Invalid(format!(
                    "{}: HOME is not set, so ~ cannot be expanded",
                    p.display()
                )));
            };
            *path = Some(home.join(rest));
            Ok(())
        };
        expand(&mut self.static_dir)?;
        for net in &mut self.networks {
            expand(&mut net.rpc_cookie_file)?;
            expand(&mut net.rpc_token_file)?;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |m: String| Err(ConfigError::Invalid(m));
        if self.networks.is_empty() {
            return invalid("at least one [[network]] is needed".into());
        }
        let mut seen = BTreeSet::new();
        for net in &self.networks {
            if !NETWORK_NAMES.contains(&net.name.as_str()) {
                return invalid(format!(
                    "network name {:?} is not one of {NETWORK_NAMES:?}",
                    net.name
                ));
            }
            if !seen.insert(net.name.as_str()) {
                return invalid(format!("network {:?} is configured twice", net.name));
            }
            if net.rpc_cookie_file.is_some() && net.rpc_token_file.is_some() {
                return invalid(format!(
                    "network {:?}: give rpc_cookie_file or rpc_token_file, not both",
                    net.name
                ));
            }
            // The gateway is built without a TLS client: it is meant to sit
            // on the node's host and reach it over loopback. An https URL
            // would pass a looser check and then fail on every call.
            match reqwest::Url::parse(&net.rpc_url) {
                Ok(url) if url.scheme() == "http" && url.has_host() => {}
                Ok(url) if url.scheme() == "https" => {
                    return invalid(format!(
                        "network {:?}: rpc_url must be an http:// URL (https is not supported; \
                         run the gateway on the node's host)",
                        net.name
                    ));
                }
                Ok(_) => {
                    return invalid(format!(
                        "network {:?}: rpc_url must be an http:// URL with a host",
                        net.name
                    ));
                }
                Err(e) => {
                    return invalid(format!("network {:?}: rpc_url: {e}", net.name));
                }
            }
            if net.poll_secs == 0 {
                return invalid(format!(
                    "network {:?}: poll_secs must be at least 1",
                    net.name
                ));
            }
        }
        let page = self.limits.page;
        if page.per_minute == 0 || page.burst == 0 {
            return invalid("limits.page: per_minute and burst must be at least 1".into());
        }
        // A bucket dropped as idle comes back full. If that could happen
        // before an emptied bucket has refilled by itself, going quiet would
        // be a way to get the burst back early, and at zero every sweep
        // would empty the table.
        let refill_secs = (u64::from(page.burst) * 60).div_ceil(u64::from(page.per_minute));
        if self.limits.idle_secs < refill_secs.max(1) {
            return invalid(format!(
                "limits.idle_secs must be at least {} (the time an empty bucket takes to refill)",
                refill_secs.max(1)
            ));
        }
        if self.limits.max_clients == 0 {
            return invalid("limits.max_clients must be at least 1".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        let mut config: Config = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: "test".into(),
            source: Box::new(source),
        })?;
        config.expand_home(Some(Path::new("/home/op")))?;
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn a_leading_tilde_is_expanded_in_every_path() {
        let c = parse(
            "static_dir = \"~/site\"\n[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\nrpc_cookie_file=\"~/.bitcoin/.cookie\"\n",
        )
        .unwrap();
        assert_eq!(c.static_dir.unwrap(), Path::new("/home/op/site"));
        assert_eq!(
            c.networks[0].rpc_cookie_file.as_deref().unwrap(),
            Path::new("/home/op/.bitcoin/.cookie")
        );
        // Only a leading `~` component: `~user` and a `~` further in stay.
        let c = parse(
            "[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\nrpc_token_file=\"/etc/~x/~user\"\n",
        )
        .unwrap();
        assert_eq!(
            c.networks[0].rpc_token_file.as_deref().unwrap(),
            Path::new("/etc/~x/~user")
        );
    }

    #[test]
    fn a_tilde_without_home_is_an_error() {
        let mut c: Config = toml::from_str(
            "[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\nrpc_cookie_file=\"~/.cookie\"\n",
        )
        .unwrap();
        let e = c.expand_home(None).unwrap_err();
        assert!(e.to_string().contains("HOME"), "{e}");
    }

    /// The shipped example must load as it stands.
    #[test]
    fn the_example_config_is_valid() {
        let text = include_str!("../../satd-site.example.toml");
        let c = parse(text).unwrap();
        assert_eq!(
            c.networks[0].rpc_cookie_file.as_deref().unwrap(),
            Path::new("/home/op/.bitcoin/.cookie")
        );
    }

    #[test]
    fn idle_secs_shorter_than_a_refill_is_refused() {
        let net = "[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\n";
        let zero = format!("[limits]\nidle_secs = 0\n{net}");
        assert!(parse(&zero).unwrap_err().to_string().contains("idle_secs"));
        // 30 tokens at 60 a minute refill in 30 s.
        let short = format!("[limits]\nidle_secs = 29\n{net}");
        assert!(
            parse(&short)
                .unwrap_err()
                .to_string()
                .contains("at least 30")
        );
        let exact = format!("[limits]\nidle_secs = 30\n{net}");
        parse(&exact).unwrap();
    }

    #[test]
    fn a_static_dir_without_the_shell_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "static_dir = {:?}\n[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\n",
            dir.path()
        );
        let config = parse(&text).unwrap();
        let e = config.check_files().unwrap_err();
        assert!(e.to_string().contains("index.html"), "{e}");
        std::fs::write(dir.path().join("index.html"), "x").unwrap();
        config.check_files().unwrap();
        // No static_dir at all is fine: the API is served alone.
        parse("[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\n")
            .unwrap()
            .check_files()
            .unwrap();
    }

    /// `--check` must refuse a URL no call could be sent to.
    #[test]
    fn a_malformed_node_url_is_refused() {
        for bad in [
            "http://",
            "http://:8332",
            "http://127.0.0.1:port",
            "http://[::1",
            "ftp://127.0.0.1",
        ] {
            let text = format!("[[network]]\nname=\"main\"\nrpc_url=\"{bad}\"\n");
            let e = parse(&text).unwrap_err();
            assert!(e.to_string().contains("rpc_url"), "{bad}: {e}");
        }
        for good in [
            "http://127.0.0.1:8332",
            "http://[::1]:8332/",
            "http://localhost:8332/wallet",
        ] {
            let text = format!("[[network]]\nname=\"main\"\nrpc_url=\"{good}\"\n");
            parse(&text).unwrap_or_else(|e| panic!("{good}: {e}"));
        }
    }

    #[test]
    fn an_https_node_url_is_refused() {
        let e =
            parse("[[network]]\nname=\"main\"\nrpc_url=\"https://127.0.0.1:8332\"\n").unwrap_err();
        assert!(e.to_string().contains("https is not supported"), "{e}");
    }

    #[test]
    fn minimal_config_takes_defaults() {
        let c = parse("[[network]]\nname = \"regtest\"\nrpc_url = \"http://127.0.0.1:18443\"\n")
            .unwrap();
        assert_eq!(c.listen, default_listen());
        assert_eq!(c.limits.page, default_page());
        assert_eq!(c.networks[0].poll_secs, 5);
        assert!(c.static_dir.is_none());
    }

    #[test]
    fn unknown_key_is_refused() {
        let e =
            parse("lisen = \"127.0.0.1:1\"\n[[network]]\nname=\"main\"\nrpc_url=\"http://x\"\n")
                .unwrap_err();
        assert!(e.to_string().contains("lisen"), "{e}");
    }

    #[test]
    fn no_network_is_refused() {
        // `network` is a required key, so its absence is a parse error.
        assert!(parse("listen = \"127.0.0.1:1\"\n").is_err());
        assert!(matches!(
            parse("network = []\n").unwrap_err(),
            ConfigError::Invalid(_)
        ));
    }

    #[test]
    fn duplicate_and_unknown_networks_are_refused() {
        let dup = "[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\n[[network]]\nname=\"main\"\nrpc_url=\"http://b\"\n";
        assert!(parse(dup).unwrap_err().to_string().contains("twice"));
        let unknown = "[[network]]\nname=\"mainnet\"\nrpc_url=\"http://a\"\n";
        assert!(parse(unknown).unwrap_err().to_string().contains("mainnet"));
    }

    #[test]
    fn both_credentials_are_refused() {
        let both = "[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\nrpc_cookie_file=\"/c\"\nrpc_token_file=\"/t\"\n";
        assert!(parse(both).unwrap_err().to_string().contains("not both"));
    }

    #[test]
    fn zero_limits_are_refused() {
        let zero = "[limits]\npage = { per_minute = 0, burst = 1 }\n[[network]]\nname=\"main\"\nrpc_url=\"http://a\"\n";
        assert!(parse(zero).unwrap_err().to_string().contains("per_minute"));
    }
}
