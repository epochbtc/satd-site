<script>
	import { page } from '$app/state';
	import { slot } from '#lib/slot.js';
	import { site, statusSlots, follow } from '#lib/site.svelte.js';
	import { networkName } from '#lib/format.js';

	const net = $derived(page.params.net ?? 'main');
	const name = $derived(networkName(net));
	const s = $derived(/** @type {Record<string, string | undefined>} */ (statusSlots(site.status[net])));
	const cli = $derived(net === 'main' ? 'sat-cli' : `sat-cli -${net}`);

	$effect(() => follow(net));

	// A new block: mark the tip once. The first value is the one the page was
	// served with, and is not new.
	/** @type {string | undefined} */
	let seen;
	let fresh = $state(false);
	$effect(() => {
		const height = s.height;
		if (seen !== undefined && height !== undefined && height !== seen) {
			fresh = false;
			requestAnimationFrame(() => (fresh = true));
		}
		seen = height;
	});
</script>

<svelte:head>
	<title>satd {name} node</title>
	<meta name="description" content="What a public satd node on {name} sees right now." />
</svelte:head>

<article class="flow">
	<section class="node" data-fresh={slot('fresh', s.fresh)} data-sync={slot('sync', s.sync)}>
		<h1 class="visually-hidden">The {name} node</h1>
		<p class="lede" aria-live="polite">
			<span class="if-none">This {name} node has not answered yet. Its numbers appear here once it does.</span>
			<span class="if-stale"
				>The node did not answer the last request. As of {slot('fetched', s.fetched)}:</span
			>
			<span class="if-synced">
				This {name} node is at block
				<span class="num tip" class:fresh>{slot('height', s.height)}</span>, connected to
				<span class="num">{slot('peers', s.peers)}</span>, and has been running for
				<span class="num">{slot('uptime', s.uptime)}</span>.
			</span>
			<span class="if-syncing">
				This {name} node is catching up with the chain: block
				<span class="num tip" class:fresh>{slot('height', s.height)}</span> of
				<span class="num">{slot('headers', s.headers)}</span> headers so far, with
				<span class="num">{slot('peers', s.peers)}</span>. It has been running for
				<span class="num">{slot('uptime', s.uptime)}</span>.
			</span>
		</p>
		<p class="detail prose if-answered">
			{slot('peers_out', s.peers_out)} of its connections are outbound and {slot(
				'peers_in',
				s.peers_in
			)} inbound. It announces itself as <code>{slot('user_agent', s.user_agent)}</code>.
		</p>
		<details class="asked">
			<summary>What the node was asked</summary>
			<pre><code>{cli} getblockchaininfo
{cli} getnetworkinfo
{cli} uptime</code></pre>
			<p>
				The site asks every few seconds and serves the last answer to everyone, so reading this page
				costs the node nothing. Your own node answers the same commands.
			</p>
		</details>
	</section>

	<section class="coming flow" aria-labelledby="coming">
		<h2 id="coming">What this site will show</h2>
		<dl>
			<dt>The network survey</dt>
			<dd>
				The reachable Bitcoin network as this node's crawler counts it: how many nodes, which
				software they run, how many speak the encrypted v2 transport. No node's clearnet address is
				published.
			</dd>
			<dt>Node attestations</dt>
			<dd>
				What coin holders have signed about the software they run, weighted by the coins behind each
				statement.
			</dd>
			<dt>MCP for agents</dt>
			<dd>A read-only MCP endpoint that an agent can query with a token earned by proof of work.</dd>
		</dl>
		<p class="prose">None of these is published here yet.</p>
	</section>

	<section class="own flow" aria-labelledby="own">
		<h2 id="own">Run your own</h2>
		<p class="prose">
			Everything on this site comes from one node's public interfaces. A satd node of your own
			answers the same questions for you, without asking anyone.
			<a href="https://epochbtc.github.io/satd/installing.html">Install satd</a>.
		</p>
	</section>
</article>

<style>
	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}

	.lede {
		font-size: var(--step-2);
		line-height: 1.3;
		letter-spacing: -0.01em;
		text-wrap: pretty;
	}
	.if-stale {
		display: block;
		font-size: var(--step-0);
		line-height: 1.6;
		color: var(--warn);
		margin-bottom: 0.5rem;
	}

	/* The gateway sets data-fresh and data-sync; each sentence is in the page
	   and these pick the one that is true. */
	[data-fresh='none'] :is(.if-stale, .if-synced, .if-syncing, .if-answered),
	[data-fresh='live'] :is(.if-none, .if-stale),
	[data-fresh='stale'] .if-none,
	[data-sync='synced'] .if-syncing,
	[data-sync='syncing'] .if-synced {
		display: none;
	}
	[data-fresh='stale'] .lede .num {
		color: var(--ink-faint);
	}

	.tip {
		color: var(--copper);
		font-weight: 700;
		text-decoration: underline 2px transparent;
		text-underline-offset: 0.2em;
	}
	/* A new block: the tip is underlined, then the line fades. */
	.tip.fresh {
		animation: tip 3s ease-out;
	}
	@keyframes tip {
		from {
			text-decoration-color: currentColor;
		}
		to {
			text-decoration-color: transparent;
		}
	}

	.detail {
		margin-top: 1.25rem;
	}

	.asked {
		margin-top: 1.5rem;
		font-size: var(--step--1);
		color: var(--ink-faint);
	}
	.asked summary {
		cursor: pointer;
		width: fit-content;
	}
	.asked pre {
		margin: 0.75rem 0;
		padding: 0.75rem 1rem;
		background: var(--panel);
		border-left: 2px solid var(--line);
		color: var(--ink);
	}

	.coming,
	.own {
		margin-top: 4rem;
	}
	dl {
		margin: 0;
	}
	dt {
		font-weight: 600;
		margin-top: 1rem;
	}
	dd {
		margin: 0.25rem 0 0;
		color: var(--ink-soft);
	}
</style>
