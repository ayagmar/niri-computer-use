// Run by `npm test`, which `npm run build` runs first. Node strips the types itself.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { releases, summary } from './news.ts';

const sample = `# Changelog

Intro text.

## Unreleased

### Fixed

- Not out yet.

## 0.2.0 - 2026-11-02

### Added

- One.
  - A nested line that belongs to One.
- Two.

### Fixed

- Three.

## 0.1.0 - 2026-10-14

The first release.

### Upgrading

- Stop old servers first.

### Added

- Everything.
`;

test('lists released versions newest first and leaves out Unreleased', () => {
	assert.deepEqual(
		releases(sample).map((r) => [r.version, r.date]),
		[
			['0.2.0', '2026-11-02'],
			['0.1.0', '2026-10-14'],
		],
	);
});

test('counts top-level entries per heading, not their nested lines', () => {
	assert.equal(summary(releases(sample)[0]), 'Added 2 · Fixed 1');
});

test('the summary names kinds of change and leaves notes such as Upgrading out', () => {
	assert.equal(summary(releases(sample)[1]), 'Added 1');
});

test('the real changelog gives every release a date and at least one entry', () => {
	const real = releases(readFileSync(new URL('../../../CHANGELOG.md', import.meta.url), 'utf8'));
	assert.ok(real.length >= 4);
	for (const release of real) {
		const entries = release.counts.reduce((sum, [, n]) => sum + n, 0);
		assert.ok(entries > 0, `${release.version} lists no entries`);
	}
});
