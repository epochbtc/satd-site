<script>
	import { page } from '$app/state';
</script>

<svelte:head>
	<title>Not found</title>
</svelte:head>

<article class="flow">
	<h1>{page.status === 404 ? 'Nothing here' : 'Something went wrong'}</h1>
	<p class="prose">
		{#if page.status === 404}
			This site has no page at that address. Start again from <a href="/">the home page</a>.
		{:else}
			The page could not be shown. Try again in a moment, or start from <a href="/">the home page</a>.
		{/if}
	</p>
</article>
