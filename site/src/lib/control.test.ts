import { test } from 'node:test';
import assert from 'node:assert/strict';
import { demo } from './control.ts';

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
