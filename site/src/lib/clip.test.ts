// Run by `npm test`, which `npm run build` runs first. Node strips the types itself.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { mp4Seconds } from './clip.ts';

test('reads the home page clip’s length from its header', () => {
	const clip = readFileSync(new URL('../../public/clips/errand.mp4', import.meta.url));
	// ffprobe reports 18.73 s for this file.
	assert.ok(Math.abs(mp4Seconds(clip) - 18.73) < 0.05, `got ${mp4Seconds(clip)}`);
});

test('refuses a file with no movie header', () => {
	assert.throws(() => mp4Seconds(new Uint8Array(64)), /no mvhd/);
});
