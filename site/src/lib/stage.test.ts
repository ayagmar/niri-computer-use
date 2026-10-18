import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clicks, end, folderName, loop, sceneAt } from './stage.ts';

test('each click lands on the coordinates its call sent', () => {
	assert.deepEqual(
		clicks.map(([, x, y]) => [x, y]),
		[
			[979, 88],
			[1061, 88],
			[1130, 100],
		],
	);
	for (const [when, x, y] of clicks) {
		assert.deepEqual(sceneAt(when).pointer, [x, y]);
		assert.deepEqual(sceneAt(when + 0.1).pulse?.slice(0, 2), [x, y]);
	}
});

test('focus moves to Files with the focus call and back to the terminal on release', () => {
	assert.equal(sceneAt(3.5).focus, 'terminal');
	assert.equal(sceneAt(3.6).focus, 'files');
	assert.equal(sceneAt(16.4).focus, 'files');
	assert.equal(sceneAt(16.5).focus, 'terminal');
	assert.equal(sceneAt(16.5).pointer, null);
});

test('the lease is held from acquire_desktop until release_desktop', () => {
	assert.equal(sceneAt(2.3).lease, false);
	assert.equal(sceneAt(2.5).lease, true);
	assert.equal(sceneAt(16.5).lease, false);
});

test('the right-click opens the menu, and choosing its item opens the name dialog', () => {
	assert.equal(sceneAt(8.7).menu, 'shut');
	assert.equal(sceneAt(9.5).menu, 'open');
	assert.equal(sceneAt(11).menu, 'chosen');
	assert.equal(sceneAt(12).menu, 'shut');
	assert.equal(sceneAt(12).dialog, true);
});

test('the folder replaces the two screenshots once the typed name is submitted', () => {
	assert.equal(sceneAt(7.2).selected, 2);
	const typing = sceneAt(14.8);
	assert.ok(typing.typed.length > 0 && typing.typed.length < folderName.length);
	assert.equal(typing.folder, false);
	const done = sceneAt(15.4);
	assert.equal(done.folder, true);
	assert.equal(done.selected, 0);
	assert.equal(done.dialog, false);
});

test('the loop ends on the finished errand, with a rest after the last call', () => {
	const last = sceneAt(end);
	assert.equal(last.folder, true);
	assert.equal(last.focus, 'terminal');
	assert.ok(loop > end);
});
