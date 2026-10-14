// Run by `npm test`, which `npm run build` runs first. Node strips the types itself.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { brief, callAt, clamp, step, stopAt, stopMarks, timing, WIDTH, type Call, type Run } from './timing.ts';

const DEMO = new URL('../../public/demo/', import.meta.url);
const runs: [string, Run][] = readdirSync(DEMO)
	.filter((file) => file.endsWith('.json'))
	.map((file) => [file, JSON.parse(readFileSync(new URL(file, DEMO), 'utf8'))]);
const recorded = (name: string) => runs.find(([file]) => file === `${name}.json`)![1];
const tool = (call: Call) => call.request.split(' ')[0];
const call = (start: number, request: string, ms: number | null = 0, you = false): Call => ({
	start,
	request,
	answer: request.startsWith('acquire_desktop') ? 'lease taken' : 'ok',
	error: false,
	you,
	ms,
});

test('every recording on the site draws all its calls inside the diagram', () => {
	assert.ok(runs.length >= 5);
	for (const [file, run] of runs) {
		const t = timing(run);
		const last = Math.max(...run.calls.map((c) => c.start + (c.ms ?? 0) / 1000));
		assert.ok(t.duration > last, `${file}: the diagram ends before its last answer`);
		for (const pulse of [...t.calls, ...t.you]) {
			assert.ok(pulse.x >= 0 && pulse.x1 <= WIDTH, `${file}: ${pulse.text} is off the diagram`);
		}
		assert.equal(t.calls.length + t.you.length, run.calls.length, `${file}: a call isn't drawn`);
	}
});

test('a time past the diagram, as at the end of a longer recording, stays on it', () => {
	assert.equal(clamp(11.47, 10), 10);
	assert.equal(clamp(-0.1, 10), 0);
	assert.equal(clamp(4.2, 10), 4.2);
});

test('the arrow keys reach each of two calls that start together', () => {
	const { calls } = recorded('errand');
	const second = callAt(calls, calls[1].start);
	assert.equal(calls[0].start, calls[1].start);
	assert.equal(second, 1);
	assert.equal(step(calls, second, -1), 0);
	assert.equal(step(calls, 0, 1), 1);
	assert.equal(step(calls, 0, -1), 0);
	assert.equal(step(calls, calls.length - 1, 1), calls.length - 1);
	assert.equal(step(calls, -1, 1), 0);
});

test('a call that answered before the stop is not cut, however wide its pulse is drawn', () => {
	const { calls } = recorded('errand');
	assert.equal(tool(calls[0]), 'status');
	const stop = stopAt(calls, calls[0].start + 0.01);
	assert.equal(stop.fates[0], 'done');
	assert.equal(stop.cut, null);
});

test('a stop cuts the running action, refuses later actions and lets looks answer', () => {
	const { calls } = recorded('stop');
	const running = calls.findIndex((c) => tool(c) === 'focus');
	const time = calls[running].start + 0.2;
	const stop = stopAt(calls, time);
	assert.equal(stop.cut, running);
	assert.equal(stop.held, true);
	calls.forEach((c, index) => {
		if (c.start <= time) return;
		const expected = c.you || tool(c) === 'release_desktop' ? 'void' : ['status', 'desktop_state', 'screenshot', 'elements', 'wait_for'].includes(tool(c)) ? 'answered' : 'refused';
		assert.equal(stop.fates[index], expected, `${tool(c)} at ${c.start} s`);
	});
	assert.equal(stop.refused, stop.fates.indexOf('refused'));
	assert.notEqual(stop.refused, null);
});

test('a look running when the stop lands still answers', () => {
	const calls = [call(0, 'acquire_desktop'), call(1, 'screenshot {}', 500), call(2, 'click {}', 40)];
	const stop = stopAt(calls, 1.2);
	assert.deepEqual(stop.fates, ['done', 'done', 'refused']);
	assert.equal(stop.cut, null);
	assert.equal(stop.refused, 2);
});

test('after the recorded stop the lease is no longer held', () => {
	const { calls } = recorded('stop');
	const own = calls.findIndex((c) => c.you && c.request.endsWith(' stop'));
	assert.equal(stopAt(calls, calls[own].start + 0.1).held, false);
	assert.equal(stopAt(calls, 0).held, false);
	assert.equal(stopAt(calls, 0).cut, null);
});

test('the page stops on the brief calls exactly as on the whole ones', () => {
	for (const [file, run] of runs) {
		const short = brief(run.calls);
		for (let time = 0; time <= timing(run).duration; time += 0.05) {
			assert.deepEqual(stopAt(short, time), stopAt(run.calls, time), `${file} at ${time.toFixed(2)} s`);
		}
	}
});

test('a call picked with the arrow keys stays picked while the recording plays on to the next call', () => {
	const { calls } = recorded('errand');
	const together = calls[0].start;
	const next = calls.find((c) => c.start > together)!.start;
	assert.equal(callAt(calls, together + 0.4, 0), 0);
	assert.equal(callAt(calls, together + 0.4), 1);
	assert.equal(callAt(calls, next, 0), callAt(calls, next));
});

test('a stop drawn at the end of the diagram stays inside it', () => {
	for (const x of [0, 500, WIDTH - 40, WIDTH]) {
		const marks = stopMarks(x, 'taken back');
		assert.ok(marks.pulse[0] >= 0 && marks.pulse[1] <= WIDTH, `pulse at ${x}`);
		const width = 'taken back'.length * 7.5;
		const [left, right] = marks.label.end ? [marks.label.x - width, marks.label.x] : [marks.label.x, marks.label.x + width];
		assert.ok(left >= 0 && right <= WIDTH, `label at ${x}`);
	}
});
