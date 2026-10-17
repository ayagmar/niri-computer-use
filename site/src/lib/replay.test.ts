import { test } from 'node:test';
import assert from 'node:assert/strict';
import { callState } from './replay.ts';

test('a call is to come before it starts, in flight for its round trip, then made', () => {
	assert.equal(callState(3.5, 3.56, 858), 'next');
	assert.equal(callState(4.3, 3.56, 858), 'now');
	assert.equal(callState(4.5, 3.56, 858), 'done');
});

test('a call of a few milliseconds stays in flight for half a second', () => {
	assert.equal(callState(1.6, 1.2, 3), 'now');
	assert.equal(callState(1.7, 1.2, 3), 'done');
});
