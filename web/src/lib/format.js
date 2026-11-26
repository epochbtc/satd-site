// How the site writes numbers and times. The gateway writes the same
// strings into the HTML it serves (gateway/src/page.rs); both are checked
// against format-vectors.json, so a page reads the same before and after
// its script runs.

/** @param {number} n */
export function count(n) {
	return String(Math.trunc(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/**
 * @param {number} n
 * @param {string} one
 * @param {string} many
 */
export function counted(n, one, many) {
	return `${count(n)} ${n === 1 ? one : many}`;
}

/** @param {number} secs */
export function duration(secs) {
	const days = Math.floor(secs / 86400);
	const hours = Math.floor((secs % 86400) / 3600);
	const minutes = Math.floor((secs % 3600) / 60);
	if (days > 0) {
		return hours > 0
			? `${counted(days, 'day', 'days')}, ${counted(hours, 'hour', 'hours')}`
			: counted(days, 'day', 'days');
	}
	if (hours > 0) {
		return minutes > 0
			? `${counted(hours, 'hour', 'hours')}, ${counted(minutes, 'minute', 'minutes')}`
			: counted(hours, 'hour', 'hours');
	}
	return minutes > 0 ? counted(minutes, 'minute', 'minutes') : 'less than a minute';
}

/** A Unix time as `HH:MM UTC`. @param {number} unix */
export function clock(unix) {
	const d = new Date(unix * 1000);
	const pad = (/** @type {number} */ n) => String(n).padStart(2, '0');
	return `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())} UTC`;
}

/** @type {Record<string, string>} */
const NETWORK_NAMES = {
	main: 'mainnet',
	signet: 'signet',
	testnet4: 'testnet4',
	regtest: 'regtest'
};

/** @param {string} network */
export function networkName(network) {
	return NETWORK_NAMES[network] ?? network;
}
