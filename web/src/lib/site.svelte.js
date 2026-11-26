import { browser } from '$app/env';
import { count, counted, duration, clock } from './format.js';

/**
 * @typedef {object} NodeStatus
 * @property {string} chain
 * @property {number} height
 * @property {number} headers
 * @property {string} best_block_hash
 * @property {boolean} initial_block_download
 * @property {string} user_agent
 * @property {number} protocol_version
 * @property {number} connections
 * @property {number} connections_in
 * @property {number} connections_out
 * @property {number} uptime_secs
 *
 * @typedef {object} Snapshot
 * @property {string} network
 * @property {number} fetched_at
 * @property {boolean} stale
 * @property {NodeStatus} node
 *
 * @typedef {object} SiteData
 * @property {string} network      the network this page is for
 * @property {string[]} networks   every network the gateway serves
 * @property {{ operator: string | null, contact: string | null }} site
 * @property {string[]} methods  the node methods the gateway may call
 * @property {Snapshot | null} status
 */

/**
 * What the gateway wrote into the page, or nothing when the page was not
 * served by a gateway (`npm run dev`, or a plain file server).
 * @returns {SiteData | null}
 */
function readEmbedded() {
	const el = document.getElementById('satd-data');
	const text = el?.textContent ?? '';
	if (text.startsWith('__SATD_')) return null;
	try {
		return JSON.parse(text);
	} catch {
		return null;
	}
}

/** The network a path is for: its first segment, or mainnet. @param {string} path */
export function networkOfPath(path) {
	const first = path.split('/')[1];
	return first === 'signet' || first === 'testnet4' || first === 'regtest' ? first : 'main';
}

const embedded = browser ? readEmbedded() : null;

export const site = $state({
	/** @type {string[]} */
	networks: embedded?.networks ?? [],
	operator: embedded?.site.operator ?? null,
	contact: embedded?.site.contact ?? null,
	/** @type {string[]} */
	methods: embedded?.methods ?? [],
	/** @type {Record<string, Snapshot | null>} */
	status: embedded ? { [embedded.network]: embedded.status } : {}
});

/**
 * The words a page shows for one network's snapshot, under the slot names
 * the gateway fills (gateway/src/page.rs).
 * @param {Snapshot | null | undefined} s
 */
export function statusSlots(s) {
	if (!s) {
		return { fresh: 'none', sync: 'synced' };
	}
	const n = s.node;
	return {
		fresh: s.stale ? 'stale' : 'live',
		sync: n.initial_block_download ? 'syncing' : 'synced',
		height: count(n.height),
		headers: count(n.headers),
		peers: counted(n.connections, 'peer', 'peers'),
		peers_out: count(n.connections_out),
		peers_in: count(n.connections_in),
		uptime: duration(n.uptime_secs),
		user_agent: n.user_agent,
		fetched: clock(s.fetched_at)
	};
}

const POLL_MS = 15_000;

/**
 * Keep one network's snapshot current while the page is visible.
 * Returns a function that stops it.
 * @param {string} network
 */
export function follow(network) {
	let stopped = false;
	/** @type {ReturnType<typeof setTimeout> | undefined} */
	let timer;
	async function poll() {
		if (stopped) return;
		if (document.visibilityState === 'visible') {
			try {
				const res = await fetch(`/api/v1/${network}/status`, { cache: 'no-store' });
				if (res.ok) site.status[network] = await res.json();
			} catch {
				// Keep what is shown; the next poll tries again.
			}
		}
		if (!stopped) timer = setTimeout(poll, POLL_MS);
	}
	const fresh = site.status[network];
	const age = fresh ? Date.now() / 1000 - fresh.fetched_at : Infinity;
	timer = setTimeout(poll, age * 1000 < POLL_MS ? POLL_MS - age * 1000 : 0);
	return () => {
		stopped = true;
		clearTimeout(timer);
	};
}
