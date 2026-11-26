import { defineParams } from '@sveltejs/kit/params';

/** The networks that live under a path prefix. Mainnet is the site's root and has none. */
const PREFIXED = ['signet', 'testnet4', 'regtest'];

export const params = defineParams({
	net: (/** @type {string} */ param) => (PREFIXED.includes(param) ? param : undefined)
});
