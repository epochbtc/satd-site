import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			// Every page is prerendered to a file the gateway serves. The gateway
			// fills each file's markers (src/lib/slot.js) first.
			adapter: adapter({ pages: 'build', assets: 'build', fallback: '404.html', strict: true }),
			prerender: { handleHttpError: 'fail', handleMissingId: 'fail' },
			// One script for the whole site: fewer round trips over onion circuits.
			output: { bundleStrategy: 'single' },
			// Written into each page as a <meta>: the browser loads nothing from
			// anywhere but this site. The gateway's header adds what a <meta>
			// cannot carry (frame-ancestors).
			csp: {
				mode: 'hash',
				directives: {
					'default-src': ['none'],
					'script-src': ['self'],
					'style-src': ['self'],
					// The one style attribute SvelteKit writes: its route announcer's.
					// src/lib/style.css hides the announcer too, in case a Kit upgrade
					// changes this hash.
					'style-src-attr': ['unsafe-hashes', 'sha256-S8qMpvofolR8Mpjy4kQvEm7m1q8clzU4dfDH0AmvZjo='],
					'img-src': ['self', 'data:'],
					'font-src': ['self'],
					'connect-src': ['self'],
					'manifest-src': ['self'],
					'base-uri': ['self'],
					'form-action': ['self']
				}
			}
		})
	],
	server: {
		// `npm run dev` against a gateway on its default address.
		proxy: { '/api': process.env.SATD_SITE_GATEWAY ?? 'http://127.0.0.1:8080' }
	}
});
