// Checks on the built site, run after every build.
//
// 1. No page or stylesheet loads anything from another origin. A link a
//    visitor follows (<a href>) may go anywhere; everything the browser
//    fetches by itself (scripts, styles, images, fonts, frames, preloads)
//    must come from this site. Tor Browser users and privacy both depend
//    on it.
// 2. Each page carries the markers the gateway fills, so a page the
//    gateway cannot fill is caught here rather than in production.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const root = process.argv[2] ?? 'build';

/** @param {string} dir @returns {string[]} */
function walk(dir) {
	return readdirSync(dir).flatMap((/** @type {string} */ name) => {
		const path = join(dir, name);
		return statSync(path).isDirectory() ? walk(path) : [path];
	});
}

// A URL that leaves this origin: a scheme, or a protocol-relative //host.
const foreign = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;
// data: URIs are inline and fetch nothing.
const inline = /^data:/i;

/** @type {string[]} */
const problems = [];
const files = walk(root);

for (const file of files) {
	const name = relative(root, file);
	if (file.endsWith('.html')) {
		const html = readFileSync(file, 'utf8');
		// Every attribute that makes the browser fetch: src, srcset, poster,
		// data, and href on anything but <a>/<area>.
		for (const m of html.matchAll(/<([a-z][a-z0-9-]*)\b([^>]*)>/gi)) {
			const [, tag, attrs] = m;
			for (const a of attrs.matchAll(/\b(src|srcset|poster|data|href|action|formaction)\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))/gi)) {
				const attr = a[1].toLowerCase();
				const value = (a[3] ?? a[4] ?? a[5] ?? '').trim();
				if (attr === 'href' && /^(a|area)$/i.test(tag)) continue;
				const urls = attr === 'srcset' ? value.split(',').map((/** @type {string} */ v) => v.trim().split(/\s+/)[0]) : [value];
				for (const url of urls) {
					if (foreign.test(url) && !inline.test(url)) {
						problems.push(`${name}: <${tag} ${attr}="${url}"> loads from another origin`);
					}
				}
			}
		}
		for (const m of html.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/gi)) checkCss(name, m[1]);
		if (!html.includes('__SATD_data__')) problems.push(`${name}: no __SATD_data__ marker`);
		if (!html.includes('data-served="__SATD_served__"')) {
			problems.push(`${name}: no data-served marker on <html>`);
		}
	} else if (file.endsWith('.css')) {
		checkCss(name, readFileSync(file, 'utf8'));
	} else if (file.endsWith('.js')) {
		const js = readFileSync(file, 'utf8');
		// The app fetches its own /api only. A literal absolute URL in a
		// fetch, import or worker is a request to somewhere else.
		for (const m of js.matchAll(/\b(?:fetch|import|Worker|EventSource|WebSocket)\s*\(\s*["'`]((?:[a-z][a-z0-9+.-]*:)?\/\/[^"'`]*)/gi)) {
			problems.push(`${name}: requests ${m[1]}`);
		}
	}
}

/** @param {string} name @param {string} css */
function checkCss(name, css) {
	for (const m of css.matchAll(/(?:url\(\s*["']?|@import\s+["'])([^"')\s]+)/gi)) {
		if (foreign.test(m[1]) && !inline.test(m[1])) {
			problems.push(`${name}: stylesheet loads ${m[1]}`);
		}
	}
}

const pages = files.filter((f) => f.endsWith('.html')).map((f) => relative(root, f));
for (const want of ['index.html', 'about.html', 'signet.html', 'signet/about.html', '404.html']) {
	if (!pages.includes(want)) problems.push(`missing page ${want}`);
}

if (problems.length) {
	console.error(problems.join('\n'));
	process.exit(1);
}
console.log(`check-build: ${pages.length} pages, ${files.length} files, nothing loads from another origin`);
