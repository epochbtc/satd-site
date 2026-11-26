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

## Run it

```sh
(cd web && npm ci && npm run build)        # the pages, into web/build
cp satd-site.example.toml satd-site.toml   # point it at your node; set static_dir
cargo run -p satd-site -- --config satd-site.toml
```

Then open <http://127.0.0.1:8080/>. `--check` validates the configuration
(and the built pages) and exits. The gateway calls only read-only node
methods, so give it a read-only credential.

The frontend needs Node 22.17 or later (`web/.nvmrc`). The gateway reads
the built pages once, at startup: restart it after every frontend build.

| Route | Answers |
|---|---|
| `GET /healthz` | 200 while the process runs |
| `GET /readyz` | 200 when every network's snapshot is fresh, else 503 |
| `GET /api/v1/networks` | the networks served |
| `GET /api/v1/{network}/status` | the node's tip, version, peers and uptime |
| `GET /`, `/about` | mainnet's pages; `/signet`, `/signet/about` and so on for the others |

Each `/api/` request and each page is charged to its client: an IPv4
address or an IPv6 /64 (scripts, styles and images are not). Over the limit the answer is 429 with `Retry-After`. Requests are
answered from a snapshot the gateway refreshes on a timer and never reach
the node; when the node is down the last snapshot is served, marked
`stale`.

## How a page is served

The frontend (`web/`, SvelteKit) is built ahead of time into one HTML file
per page, with each live value written as a marker such as
`__SATD_height__`. The gateway fills the markers from its cache when it
sends a page, so the numbers are in the HTML: a visitor with script off
(Tor Browser's Safest level) reads the same page as one with it on. The
app then takes over and keeps the numbers current from `/api/`.

No page loads anything from another origin: no fonts, scripts, images or
analytics from elsewhere. The build checks this (`web/scripts/check-build.js`)
and every page carries a Content-Security-Policy that enforces it.

## Develop

```sh
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo deny check
cd web && npm run check && npm test && npm run build
```

`npm run dev` serves the frontend with live reload and sends `/api/` to a
gateway on `127.0.0.1:8080` (`SATD_SITE_GATEWAY` to change it). With
`SATD_SITE_WEB_BUILD=web/build`, `cargo test` also fills every page of the
real build.

## Layout

| Path | What |
|---|---|
| `gateway/` | The Rust server: node clients, snapshot cache, rate limits, JSON API, static file serving |
| `web/` | The SvelteKit frontend, built to static files the gateway serves |

## License

[MIT](LICENSE).
