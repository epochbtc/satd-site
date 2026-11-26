//! Pages: the built frontend's HTML, filled in from the cache.
//!
//! The frontend is built ahead of time (`web/`), with every live value
//! written as a marker, `__SATD_<name>__`. At startup the gateway reads each
//! built page and splits it at its markers; a request fills them from the
//! snapshot cache, so a visitor with script off reads the same numbers as
//! one with it on. A marker this module does not know is a startup error:
//! the frontend and the gateway disagree, and the page would show a gap.
//!
//! What fills a marker is HTML-escaped (or, for `data`, JSON escaped for a
//! `<script>` element) and is never scanned for markers again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::SiteConfig;
use crate::node::Method;
use crate::status::Snapshot;

/// Every marker a built page may carry.
const SLOTS: &[&str] = &[
    // On <html>: the networks served, space separated.
    "served",
    // The JSON the app starts from (`web/src/lib/site.svelte.js`).
    "data",
    // The network's node status.
    "fresh",
    "sync",
    "height",
    "headers",
    "peers",
    "peers_out",
    "peers_in",
    "uptime",
    "user_agent",
    "fetched",
    // The operator, from `[site]`.
    "named",
    "operator",
    "has_contact",
    "contact",
    // The node methods the gateway may call.
    "methods",
];

#[derive(Debug, thiserror::Error)]
pub enum PageError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: unknown marker __SATD_{name}__ (is the gateway older than the frontend?)")]
    UnknownSlot { path: PathBuf, name: String },
    #[error("{path}: a symbolic link in the built frontend (copy the files in instead)")]
    Symlink { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Text(String),
    Slot(&'static str),
}

/// One built page, split at its markers.
#[derive(Debug, Clone)]
pub struct Template {
    parts: Vec<Part>,
}

impl Template {
    pub fn parse(html: &str, path: &Path) -> Result<Self, PageError> {
        const OPEN: &str = "__SATD_";
        let mut parts = Vec::new();
        let mut rest = html;
        while let Some(at) = rest.find(OPEN) {
            let after = &rest[at + OPEN.len()..];
            // The longest known name followed by `__`: names may hold single
            // underscores (`peers_out`) and may be followed by anything.
            let known = SLOTS
                .iter()
                .filter(|s| after.starts_with(*s) && after[s.len()..].starts_with("__"))
                .max_by_key(|s| s.len());
            let Some(slot) = known else {
                // Shaped like a marker (`__SATD_name__`, any letters, digits
                // or underscores) but not one we know.
                let word = after
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    .count();
                if let Some(end) = after[..word].find("__").filter(|e| *e > 0) {
                    return Err(PageError::UnknownSlot {
                        path: path.to_owned(),
                        name: after[..end].to_owned(),
                    });
                }
                // Not a marker: keep the text and look past it.
                parts.push(Part::Text(rest[..at + OPEN.len()].to_owned()));
                rest = after;
                continue;
            };
            let name = *slot;
            parts.push(Part::Text(rest[..at].to_owned()));
            parts.push(Part::Slot(name));
            rest = &after[name.len() + 2..];
        }
        parts.push(Part::Text(rest.to_owned()));
        parts.retain(|p| !matches!(p, Part::Text(t) if t.is_empty()));
        Ok(Self { parts })
    }

    pub fn render(&self, values: &Values) -> String {
        let mut out = String::with_capacity(8 * 1024);
        for part in &self.parts {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Slot("data") => out.push_str(&script_json(&values.data)),
                Part::Slot(name) => {
                    escape_html(values.text.get(name).map_or("", String::as_str), &mut out)
                }
            }
        }
        out
    }
}

/// Every built page, by the path it is served at (`""`, `about`,
/// `signet`, `signet/about`), and the not-found page.
#[derive(Debug, Clone)]
pub struct Pages {
    pages: HashMap<String, Template>,
    not_found: Option<Template>,
}

impl Pages {
    /// Read every `.html` file under `dir`. `dir` itself may be a symbolic
    /// link; anything under it may not, so a link loop cannot hang startup
    /// and no page is read from outside `dir`.
    pub fn load(dir: &Path) -> Result<Self, PageError> {
        let mut pages = HashMap::new();
        let mut not_found = None;
        let mut stack = vec![dir.to_owned()];
        while let Some(d) = stack.pop() {
            let read = |source| PageError::Read {
                path: d.clone(),
                source,
            };
            for entry in std::fs::read_dir(&d).map_err(read)? {
                let entry = entry.map_err(read)?;
                let path = entry.path();
                // The entry's own type: a link is not followed.
                let kind = entry.file_type().map_err(|source| PageError::Read {
                    path: path.clone(),
                    source,
                })?;
                if kind.is_symlink() {
                    return Err(PageError::Symlink { path });
                }
                if kind.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Some(rel) = path
                    .strip_prefix(dir)
                    .ok()
                    .and_then(|r| r.to_str())
                    .and_then(|r| r.strip_suffix(".html"))
                else {
                    continue;
                };
                let html = std::fs::read_to_string(&path).map_err(|source| PageError::Read {
                    path: path.clone(),
                    source,
                })?;
                let template = Template::parse(&html, &path)?;
                let key = rel.replace('\\', "/");
                match key.as_str() {
                    "404" => not_found = Some(template),
                    "index" => {
                        pages.insert(String::new(), template);
                    }
                    _ => {
                        pages.insert(key, template);
                    }
                }
            }
        }
        Ok(Self { pages, not_found })
    }

    /// The page served at `path` (no leading or trailing slash).
    pub fn get(&self, path: &str) -> Option<&Template> {
        self.pages.get(path)
    }

    pub fn not_found(&self) -> Option<&Template> {
        self.not_found.as_ref()
    }
}

/// The networks with a path prefix. Mainnet is the root.
pub const PREFIXED: [&str; 3] = ["signet", "testnet4", "regtest"];

/// Which network a page path is for, and the page within it:
/// `signet/about` is signet's `about`; `about` is mainnet's.
pub fn split_network(path: &str) -> (&str, &str) {
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    if PREFIXED.contains(&first) {
        (first, rest)
    } else {
        ("main", path)
    }
}

/// The path a network's page is served at.
pub fn page_path(network: &str, page: &str) -> String {
    match (network, page) {
        ("main", page) => page.to_owned(),
        (network, "") => network.to_owned(),
        (network, page) => format!("{network}/{page}"),
    }
}

/// What the app reads at startup, embedded in the page.
#[derive(Debug, Serialize)]
pub struct Embedded<'a> {
    pub network: &'a str,
    pub networks: Vec<&'a str>,
    pub site: &'a SiteConfig,
    pub methods: Vec<&'static str>,
    pub status: Option<&'a Snapshot>,
}

/// The values for one page view.
#[derive(Debug, Default)]
pub struct Values {
    text: HashMap<&'static str, String>,
    data: String,
}

impl Values {
    pub fn new(embedded: &Embedded<'_>) -> Self {
        let mut text = HashMap::new();
        let mut set = |k: &'static str, v: String| {
            text.insert(k, v);
        };
        set("served", embedded.networks.join(" "));
        set("methods", embedded.methods.join(", "));
        let site = embedded.site;
        set("named", yes_no(site.operator.is_some()));
        set("operator", site.operator.clone().unwrap_or_default());
        set("has_contact", yes_no(site.contact.is_some()));
        set("contact", site.contact.clone().unwrap_or_default());
        match embedded.status {
            None => {
                set("fresh", "none".into());
                set("sync", "synced".into());
            }
            Some(s) => {
                let n = &s.node;
                set("fresh", if s.stale { "stale" } else { "live" }.into());
                set(
                    "sync",
                    if n.initial_block_download {
                        "syncing"
                    } else {
                        "synced"
                    }
                    .into(),
                );
                set("height", count(n.height));
                set("headers", count(n.headers));
                set("peers", counted(n.connections, "peer", "peers"));
                set("peers_out", count(n.connections_out));
                set("peers_in", count(n.connections_in));
                set("uptime", duration(n.uptime_secs));
                set("user_agent", n.user_agent.clone());
                set("fetched", clock(s.fetched_at));
            }
        }
        let data = serde_json::to_string(embedded).unwrap_or_else(|_| "null".into());
        Self { text, data }
    }

    #[cfg(test)]
    fn text(&self, name: &str) -> Option<&str> {
        self.text.get(name).map(String::as_str)
    }
}

pub fn methods() -> Vec<&'static str> {
    Method::ALL.iter().map(|m| m.name()).collect()
}

fn yes_no(b: bool) -> String {
    if b { "yes" } else { "no" }.into()
}

// The formatting below is the frontend's (`web/src/lib/format.js`): both
// are tested against `web/src/lib/format-vectors.json`.

pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

pub fn duration(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = secs % 86_400 / 3_600;
    let minutes = secs % 3_600 / 60;
    let d = || counted(days, "day", "days");
    let h = || counted(hours, "hour", "hours");
    let m = || counted(minutes, "minute", "minutes");
    match (days, hours, minutes) {
        (1.., 1.., _) => format!("{}, {}", d(), h()),
        (1.., 0, _) => d(),
        (0, 1.., 1..) => format!("{}, {}", h(), m()),
        (0, 1.., 0) => h(),
        (0, 0, 1..) => m(),
        (0, 0, 0) => "less than a minute".into(),
    }
}

/// A Unix time as `HH:MM UTC`.
pub fn clock(unix: u64) -> String {
    format!("{:02}:{:02} UTC", unix % 86_400 / 3_600, unix % 3_600 / 60)
}

fn escape_html(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// JSON that is safe inside `<script type="application/json">`: nothing in
/// it can close the element or open a comment.
fn script_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::NodeStatus;
    use serde_json::Value;

    fn path() -> PathBuf {
        PathBuf::from("test.html")
    }

    fn snapshot(stale: bool, ibd: bool) -> Snapshot {
        Snapshot {
            network: "main".into(),
            fetched_at: 1_795_000_000,
            stale,
            node: NodeStatus {
                chain: "main".into(),
                height: 925_113,
                headers: 925_200,
                best_block_hash: "00".repeat(32),
                initial_block_download: ibd,
                user_agent: "/satd:0.8.0/".into(),
                protocol_version: 70016,
                connections: 10,
                connections_in: 2,
                connections_out: 8,
                uptime_secs: 90_000,
            },
        }
    }

    fn embedded<'a>(site: &'a SiteConfig, status: Option<&'a Snapshot>) -> Embedded<'a> {
        Embedded {
            network: "main",
            networks: vec!["main", "signet"],
            site,
            methods: methods(),
            status,
        }
    }

    #[test]
    fn formatting_matches_the_frontends_vectors() {
        let vectors: Value =
            serde_json::from_str(include_str!("../../web/src/lib/format-vectors.json")).unwrap();
        let cases = |name: &str| {
            vectors[name]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| (c[0].as_u64().unwrap(), c[1].as_str().unwrap().to_owned()))
                .collect::<Vec<_>>()
        };
        for (n, want) in cases("count") {
            assert_eq!(count(n), want, "count({n})");
        }
        for (n, want) in cases("peers") {
            assert_eq!(counted(n, "peer", "peers"), want, "peers({n})");
        }
        for (n, want) in cases("duration") {
            assert_eq!(duration(n), want, "duration({n})");
        }
        for (n, want) in cases("clock") {
            assert_eq!(clock(n), want, "clock({n})");
        }
    }

    #[test]
    fn markers_are_filled_and_escaped() {
        let t = Template::parse(
            r#"<html data-served="__SATD_served__"><b>__SATD_height__</b> <i>__SATD_operator__</i></html>"#,
            &path(),
        )
        .unwrap();
        let site = SiteConfig {
            operator: Some(r#"<script>alert("x")</script> & co"#.into()),
            contact: None,
        };
        let s = snapshot(false, false);
        let html = t.render(&Values::new(&embedded(&site, Some(&s))));
        assert_eq!(
            html,
            r#"<html data-served="main signet"><b>925,113</b> <i>&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; co</i></html>"#
        );
    }

    #[test]
    fn a_filled_value_is_not_scanned_for_markers() {
        let t = Template::parse("__SATD_operator__|__SATD_contact__", &path()).unwrap();
        let site = SiteConfig {
            operator: Some("__SATD_contact__".into()),
            contact: Some("c".into()),
        };
        assert_eq!(
            t.render(&Values::new(&embedded(&site, None))),
            "__SATD_contact__|c"
        );
    }

    #[test]
    fn the_embedded_json_cannot_close_its_script_element() {
        let t = Template::parse(
            r#"<script type="application/json">__SATD_data__</script>"#,
            &path(),
        )
        .unwrap();
        let site = SiteConfig {
            operator: Some("</script><script>alert(1)</script><!--".into()),
            contact: None,
        };
        let html = t.render(&Values::new(&embedded(&site, None)));
        let inner = html
            .strip_prefix(r#"<script type="application/json">"#)
            .and_then(|h| h.strip_suffix("</script>"))
            .unwrap();
        assert!(!inner.contains('<') && !inner.contains('>'), "{inner}");
        let back: Value = serde_json::from_str(inner).unwrap();
        assert_eq!(
            back["site"]["operator"],
            "</script><script>alert(1)</script><!--"
        );
        assert_eq!(back["network"], "main");
        assert_eq!(back["networks"], serde_json::json!(["main", "signet"]));
        assert!(back["status"].is_null());
    }

    #[test]
    fn an_unknown_marker_is_refused_at_load() {
        let e = Template::parse("<b>__SATD_nope__</b>", &path()).unwrap_err();
        assert!(e.to_string().contains("__SATD_nope__"), "{e}");
    }

    #[test]
    fn a_marker_with_digits_or_capitals_is_refused_too() {
        for text in ["__SATD_v2_tip__", "__SATD_PEERS__", "x __SATD_height2__ y"] {
            let e = Template::parse(text, &path()).unwrap_err();
            assert!(matches!(e, PageError::UnknownSlot { .. }), "{text}: {e}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_under_the_build_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "x").unwrap();
        Pages::load(dir.path()).unwrap();
        // A loop would otherwise be walked for ever.
        std::os::unix::fs::symlink(".", dir.path().join("loop")).unwrap();
        let e = Pages::load(dir.path()).unwrap_err();
        assert!(matches!(e, PageError::Symlink { .. }), "{e}");
        std::fs::remove_file(dir.path().join("loop")).unwrap();
        // A page from outside the build.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x.html"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("x.html"), dir.path().join("about.html"))
            .unwrap();
        let e = Pages::load(dir.path()).unwrap_err();
        assert!(matches!(e, PageError::Symlink { .. }), "{e}");
        // The build directory itself may be a link.
        std::fs::remove_file(dir.path().join("about.html")).unwrap();
        let link = outside.path().join("build");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        Pages::load(&link).unwrap();
    }

    #[test]
    fn text_that_only_looks_like_a_marker_is_kept() {
        for text in [
            "__SATD_",
            "__SATD_x",
            "__SATD___",
            "a __SATD_ b __SATD_height__",
        ] {
            let t = Template::parse(text, &path()).unwrap();
            let site = SiteConfig::default();
            let got = t.render(&Values::new(&embedded(&site, None)));
            assert_eq!(got, text.replace("__SATD_height__", ""), "{text}");
        }
    }

    #[test]
    fn page_state_follows_the_snapshot() {
        let site = SiteConfig::default();
        let v = Values::new(&embedded(&site, None));
        assert_eq!(v.text("fresh"), Some("none"));
        assert_eq!(v.text("height"), None);
        let s = snapshot(true, true);
        let v = Values::new(&embedded(&site, Some(&s)));
        assert_eq!(v.text("fresh"), Some("stale"));
        assert_eq!(v.text("sync"), Some("syncing"));
        assert_eq!(v.text("peers"), Some("10 peers"));
        assert_eq!(v.text("uptime"), Some("1 day, 1 hour"));
        assert_eq!(v.text("fetched"), Some("11:06 UTC"));
        assert_eq!(v.text("named"), Some("no"));
        assert_eq!(
            v.text("methods"),
            Some("getblockchaininfo, getnetworkinfo, uptime")
        );
    }

    #[test]
    fn paths_split_into_network_and_page() {
        assert_eq!(split_network(""), ("main", ""));
        assert_eq!(split_network("about"), ("main", "about"));
        assert_eq!(split_network("signet"), ("signet", ""));
        assert_eq!(split_network("signet/about"), ("signet", "about"));
        assert_eq!(split_network("main/about"), ("main", "main/about"));
        assert_eq!(page_path("main", ""), "");
        assert_eq!(page_path("main", "about"), "about");
        assert_eq!(page_path("signet", ""), "signet");
        assert_eq!(page_path("signet", "about"), "signet/about");
    }
}
