import { test } from 'node:test';
import assert from 'node:assert/strict';
import { demo, tickets } from './control.ts';

test('a start that a pause overtook is stale when it settles', () => {
	const starts = tickets();
	const pending = starts.take();
	starts.cancel();
	assert.equal(starts.current(pending), false);
});

test('a start that a newer start overtook is stale, and the newer one counts', () => {
	const starts = tickets();
	const first = starts.take();
	const second = starts.take();
	assert.equal(starts.current(first), false);
	assert.equal(starts.current(second), true);
});

test('the demo plays once, when first seen, and then its follow-up', () => {
	const run = demo();
	assert.equal(run.see(), true);
	assert.equal(run.follow(), true);
	assert.equal(run.see(), false);
});

test('a row picked before the diagram is seen keeps the demo from running at all', () => {
	const run = demo();
	run.pick();
	assert.equal(run.see(), false);
	assert.equal(run.follow(), false);
});

test('a row picked while the demo plays cancels its follow-up', () => {
	const run = demo();
	assert.equal(run.see(), true);
	run.pick();
	assert.equal(run.follow(), false);
});
