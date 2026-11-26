import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { count, counted, duration, clock } from './format.js';

const vectors = JSON.parse(readFileSync(new URL('./format-vectors.json', import.meta.url), 'utf8'));

/** @type {Record<string, (n: number) => string>} */
const formatters = {
	count,
	peers: (n) => counted(n, 'peer', 'peers'),
	duration,
	clock
};

for (const [name, cases] of Object.entries(vectors)) {
	test(`${name} matches the shared vectors`, () => {
		for (const [input, want] of cases) {
			assert.equal(formatters[name](input), want, `${name}(${input})`);
		}
	});
}
