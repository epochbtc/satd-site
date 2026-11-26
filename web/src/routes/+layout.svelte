<script>
	import '#lib/style.css';
	import { page } from '$app/state';
	import { networkName } from '#lib/format.js';

	let { children } = $props();

	const NETWORKS = ['main', 'signet', 'testnet4', 'regtest'];
	const net = $derived(page.params.net ?? 'main');
	/** The page within its network: '' for the network's home, '/about' and so on. */
	const within = $derived.by(() => {
		const path = net === 'main' ? page.url.pathname : page.url.pathname.slice(net.length + 1);
		return path === '/' ? '' : path;
	});
	/** @param {string} network @param {string} path */
	const href = (network, path) => (network === 'main' ? '' : `/${network}`) + path || '/';
</script>

<div class="page">
	<header class="masthead">
		<a class="wordmark" href={href(net, '')}>satd</a>
		<!-- Every network is in the page; the gateway's data-served on <html>
		     shows the ones it serves. -->
		<nav class="networks" aria-label="Network">
			{#each NETWORKS as n (n)}
				<a data-net={n} href={href(n, within)} aria-current={n === net ? 'page' : undefined}
					>{networkName(n)}</a
				>
			{/each}
		</nav>
	</header>

	<main>
		{@render children()}
	</main>

	<footer class="colophon">
		<p>
			Served by a <a href="https://github.com/epochbtc/satd">satd</a> node through
			<a href="https://github.com/epochbtc/satd-site">satd-site</a>, whose code anyone can run beside
			their own node.
		</p>
		<ul>
			<li><a href="https://epochbtc.github.io/satd/">Operator Manual</a></li>
			<li><a href="https://github.com/epochbtc/satd/releases">Releases</a></li>
			<li><a href={href(net, '/about')}>About this site</a></li>
		</ul>
	</footer>
</div>

<style>
	.masthead {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 1rem;
		padding: 1.5rem 0 3rem;
	}
	.wordmark {
		font-size: var(--step-1);
		font-weight: 700;
		letter-spacing: -0.02em;
		text-decoration: none;
	}
	.networks {
		display: flex;
		gap: 1rem;
		font-size: var(--step--1);
	}
	.networks a {
		display: none;
		color: var(--ink-faint);
		text-decoration: none;
	}
	.networks a[aria-current='page'] {
		color: var(--ink);
		text-decoration: underline;
		text-decoration-color: var(--copper);
		text-decoration-thickness: 2px;
		text-underline-offset: 0.35em;
	}
	:global(html[data-served~='main']) .networks a[data-net='main'],
	:global(html[data-served~='signet']) .networks a[data-net='signet'],
	:global(html[data-served~='testnet4']) .networks a[data-net='testnet4'],
	:global(html[data-served~='regtest']) .networks a[data-net='regtest'] {
		display: inline;
	}
	/* One network served: nothing to switch between. */
	:global(html[data-served='main']) .networks,
	:global(html[data-served='signet']) .networks,
	:global(html[data-served='testnet4']) .networks,
	:global(html[data-served='regtest']) .networks {
		display: none;
	}

	.colophon {
		margin-top: 5rem;
		padding: 1.5rem 0 2.5rem;
		border-top: 1px solid var(--line);
		font-size: var(--step--1);
		color: var(--ink-faint);
	}
	.colophon ul {
		list-style: none;
		padding: 0;
		margin: 0.75rem 0 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 1.25rem;
	}
</style>
