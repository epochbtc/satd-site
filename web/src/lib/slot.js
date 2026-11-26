import { building } from '$app/env';

/**
 * A value the gateway writes into the page.
 *
 * When the site is built, a page is rendered once with no node to ask, so
 * each live value is written as a marker, `__SATD_<name>__`. The gateway
 * replaces every marker with the value from its cache, HTML-escaped, before
 * it sends the page (gateway/src/page.rs). In the browser the same call
 * returns the value itself, so the hydrated page matches what was sent.
 *
 * @param {string} name
 * @param {string | undefined} value
 */
export function slot(name, value) {
	return building ? `__SATD_${name}__` : (value ?? '');
}
