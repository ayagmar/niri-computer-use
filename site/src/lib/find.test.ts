import { test } from 'node:test';
import assert from 'node:assert/strict';
import { matches } from './find.ts';

test('finds a pattern in any case, at its offsets in the text', () => {
	assert.deepEqual(matches('The lease, the LEASE', 'lease'), [
		[4, 9],
		[15, 20],
	]);
});

test('offsets stay inside text whose lowercase is longer, such as İ', () => {
	assert.deepEqual(matches('missingİİİabc', 'abc'), [[10, 13]]);
	for (const [, end] of matches('İİİi', 'i')) assert.ok(end <= 'İİİi'.length);
});

test('takes the pattern literally', () => {
	assert.deepEqual(matches('cli(1), safety(7)', '(1)'), [[3, 6]]);
	assert.deepEqual(matches('a.b', '.'), [[1, 2]]);
});
