# satd-site

The public face of a [satd](https://github.com/epochbtc/satd) node: a small
gateway and a web frontend that show what the node knows, using nothing but
the node's own public surfaces.

- **Network survey**: the reachable Bitcoin network as satd's crawl counts
  it, with the BIP 324 transport split and implementations told apart by
  behaviour. No clearnet address is ever published.
- **Node attestation (NAP)**: what coin holders have signed, weighted by the
  coins behind each statement.
- **MCP**: a read-only MCP endpoint for agents.

It runs beside a stock satd and talks to it over JSON-RPC and Esplora. The
gateway holds the node's credentials, calls a fixed allowlist of methods,
serves pages from a cache, and rate limits everything else. Anyone can run
it against their own node.

**Status:** early. Nothing here is ready to deploy yet.

## Run the gateway

```sh
cp satd-site.example.toml satd-site.toml   # point it at your node
cargo run -p satd-site -- --config satd-site.toml
curl http://127.0.0.1:8080/api/v1/networks
```

`--check` validates the configuration and exits. The gateway calls only
read-only node methods, so give it a read-only credential.

| Route | Answers |
|---|---|
| `GET /healthz` | 200 while the process runs |
| `GET /readyz` | 200 when every network's snapshot is fresh, else 503 |
| `GET /api/v1/networks` | the networks served |
| `GET /api/v1/{network}/status` | the node's tip, version, peers and uptime |

Each `/api/` request is charged to its client: an IPv4 address or an IPv6
/64. Over the limit the answer is 429 with `Retry-After`. Requests are
answered from a snapshot the gateway refreshes on a timer and never reach
the node; when the node is down the last snapshot is served, marked
`stale`.

## Develop

```sh
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo deny check
```

## Layout

| Path | What |
|---|---|
| `gateway/` | The Rust server: node clients, snapshot cache, rate limits, JSON API, static file serving |
| `web/` | The SvelteKit frontend, built to static files the gateway serves |

## License

[MIT](LICENSE).
