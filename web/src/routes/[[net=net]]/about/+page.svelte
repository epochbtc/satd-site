<script>
	import { page } from '$app/state';
	import { slot } from '#lib/slot.js';
	import { site } from '#lib/site.svelte.js';
	import { networkName } from '#lib/format.js';

	const net = $derived(page.params.net ?? 'main');
	const named = $derived(site.operator ? 'yes' : 'no');
	const contact = $derived(site.contact ? 'yes' : 'no');
</script>

<svelte:head>
	<title>About this satd site</title>
	<meta name="description" content="Who runs this satd site, what it records and where its numbers come from." />
</svelte:head>

<article class="flow">
	<h1>About this site</h1>

	<section class="flow" data-named={slot('named', named)} data-contact={slot('has_contact', contact)}>
		<h2>Who runs it</h2>
		<p class="if-named">This site is run by {slot('operator', site.operator ?? undefined)}.</p>
		<p class="if-unnamed">Whoever runs this site has not given a name.</p>
		<p class="if-contact">To reach them: {slot('contact', site.contact ?? undefined)}.</p>
		<p class="prose">
			If this site's node or its crawler is a problem for your node, that is the place to say so.
		</p>
	</section>

	<section class="flow">
		<h2>What it keeps about you</h2>
		<p class="prose">
			Nothing written down. There is no request log. To share the node fairly, the site counts each
			visitor's recent requests in memory, by IPv4 address or IPv6 /64, and forgets a visitor who
			has been quiet for a few minutes. Those counts are never written to disk.
		</p>
		<p class="prose">
			Every font, script and image on these pages is served by this site. No page asks another
			server for anything, and there is no analytics.
		</p>
	</section>

	<section class="flow">
		<h2>Where the numbers come from</h2>
		<p class="prose">
			The {networkName(net)} pages show what one satd node answers. The site holds the node's credentials
			and calls only read-only methods from a fixed list:
			<code>{slot('methods', site.methods.join(', '))}</code>. It asks on a
			timer and serves the last answer, so no visitor's request reaches the node.
		</p>
		<p class="prose">
			The same answers are served as JSON under <a href="/api/v1/networks" rel="external"><code>/api/v1/</code></a>.
		</p>
	</section>
</article>

<style>
	section {
		margin-top: 3rem;
	}
	[data-named='yes'] .if-unnamed,
	[data-named='no'] .if-named,
	[data-contact='no'] .if-contact {
		display: none;
	}
</style>
