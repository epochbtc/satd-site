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

## Layout

| Path | What |
|---|---|
| `gateway/` | The Rust server: node clients, snapshot cache, rate limits, JSON API, static file serving |
| `web/` | The SvelteKit frontend, built to static files the gateway serves |

## License

[MIT](LICENSE).
