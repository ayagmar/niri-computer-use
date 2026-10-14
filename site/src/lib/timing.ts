// A recording's calls as a timing diagram: one lane for the agent's calls, one for the
// lease, one for what niri saw, one for what you did; and what the server would answer had
// you pressed the stop key at a given moment. Pure, so the page draws the diagram at build
// time, the browser runs the stop with the same code, and `timing.test.ts` checks both.

export interface Call {
	/** Seconds into the video. */
	start: number;
	request: string;
	answer: string;
	/** The answer refused or stopped. */
	error: boolean;
	/** A command the user ran, not a call. */
	you: boolean;
	/** The round trip, or null for a user command. */
	ms: number | null;
}

export interface Run {
	prompt: string;
	calls: Call[];
	/** How the recording was made. */
	note?: string;
	/** The recorded output, such as `DP-1 · 2560×1440, shown at 1920×1080`. */
	output?: string;
}

/** Who holds the lease after each call: taken by `acquire_desktop`, given back by
 * `release_desktop`, taken back by the user's stop command. */
export function leaseAfter(calls: Call[]): ('free' | 'held' | 'stopped')[] {
	let lease: 'free' | 'held' | 'stopped' = 'free';
	return calls.map((call) => {
		if (isStop(call)) lease = 'stopped';
		else if (call.request.startsWith('acquire_desktop') && call.answer.startsWith('lease taken')) lease = 'held';
		else if (call.request.startsWith('release_desktop') && call.answer.startsWith('released')) lease = 'free';
		return lease;
	});
}

/** Seconds into the recording as m:ss.s, as a player shows them. */
export const at = (seconds: number) =>
	`${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(1).padStart(4, '0')}`;

const isStop = (call: Call) => call.you && call.request.endsWith(' stop');
const tool = (call: Call) => call.request.split(' ')[0];
/** When a call's answer came back, in seconds into the video. */
const end = (call: Call) => call.start + (call.ms ?? 0) / 1000;

/** The diagram's width in SVG units; the lanes' labels sit outside it, in HTML. */
export const WIDTH = 1000;
/** Units per character of the 11-unit mono labels, to keep labels from overlapping. */
const CHAR = 6.7;
const GAP = 10;

export interface Label {
	x: number;
	row: number;
	text: string;
}

export interface Pulse extends Label {
	/** Where the pulse ends: its answer, or a few units on for a call too quick to see. */
	x1: number;
	error: boolean;
	/** Index into the run's calls. */
	call: number;
}

export interface Sighting extends Label {
	/** Index into the run's calls. */
	call: number;
}

/** The tools that only look, and need no lease. */
const LOOKS = new Set(['status', 'desktop_state', 'screenshot', 'elements', 'wait_for']);

export interface Timing {
	/** Seconds the diagram spans. */
	duration: number;
	/** Seconds at each whole-second grid line. */
	seconds: number[];
	calls: Pulse[];
	/** The lease line as [x, held] steps; `stopped` marks the stretch after a stop. */
	lease: { x: number; state: 'free' | 'held' | 'stopped' }[];
	/** Where the lease is held, as [from, to]. */
	held: [number, number][];
	niri: Sighting[];
	you: Pulse[];
}

/** The diagram of `run`. */
export function timing(run: Run): Timing {
	const duration = Math.ceil(Math.max(...run.calls.map(end)) + 1);
	const x = (seconds: number) => (WIDTH * seconds) / duration;
	const calls = rows(
		run.calls.flatMap((call, index) => (call.you ? [] : [pulse(call, index, x)])),
	);
	const you = rows(run.calls.flatMap((call, index) => (call.you ? [userPulse(call, index, x)] : [])));
	const niri = rows(
		run.calls.flatMap((call, index) => {
			const text = saw(call);
			return text ? [{ x: x(end(call)), row: 0, text, call: index }] : [];
		}),
	);
	const states = leaseAfter(run.calls);
	const lease = [{ x: 0, state: 'free' as const }];
	run.calls.forEach((call, index) => {
		if (states[index] !== lease.at(-1)!.state) {
			lease.push({ x: x(end(call)), state: states[index] });
		}
	});
	const held = lease.flatMap((step, index): [number, number][] =>
		step.state === 'held' ? [[step.x, lease[index + 1]?.x ?? WIDTH]] : [],
	);
	return {
		duration,
		seconds: Array.from({ length: duration + 1 }, (_, second) => second),
		calls,
		lease,
		held,
		niri,
		you,
	};
}

function pulse(call: Call, index: number, x: (s: number) => number): Pulse {
	const start = x(call.start);
	// A call too quick to see still gets a pulse a few units wide.
	const x1 = Math.max(start + 4, x(end(call)));
	return { x: start, x1, row: 0, text: tool(call), error: call.error, call: index };
}

function userPulse(call: Call, index: number, x: (s: number) => number): Pulse {
	const start = x(call.start);
	const text = isStop(call) ? 'stop key' : call.answer.split(';')[0].replace(/^you /, '');
	return { x: start, x1: start + 14, row: 0, text, error: false, call: index };
}

/** What niri saw, from the answer: a focus move, a new size, or the observed word when it
 * says more than `sent`. */
export function saw(call: Call): string | null {
	if (call.you || call.error) return null;
	const focus = call.answer.match(/focus (\w+) → (\w+)/);
	if (focus) return `focus ${focus[1]} → ${focus[2]}`;
	const size = call.answer.match(/window (\d+) size (\d+×\d+)/);
	if (size) return `${size[1]}: ${size[2]}`;
	const back = call.answer.match(/focus back on (\d+)/);
	if (back) return `focus back on ${back[1]}`;
	const observed = call.answer.match(/observed: (\w+)/);
	return observed && observed[1] !== 'sent' ? observed[1] : null;
}

/** Puts each label on the lowest row where it doesn't run into the label before it. */
function rows<T extends Label>(labels: T[]): T[] {
	const ends: number[] = [];
	return labels.map((label) => {
		let row = ends.findIndex((end) => end + GAP <= label.x);
		if (row === -1) row = ends.length;
		ends[row] = label.x + label.text.length * CHAR;
		return { ...label, row };
	});
}

/** What became of a call had you pressed the stop key at some moment: done before it, an
 * action cut short by it, refused after it, a look that still answers, or something that no
 * longer happens, such as giving back a lease already taken back or a later command of yours. */
export type Fate = 'done' | 'cut' | 'refused' | 'answered' | 'void';

export interface Stop {
	/** Each call's fate, by index into the run's calls. */
	fates: Fate[];
	/** The action the stop cut short, if one was running. */
	cut: number | null;
	/** The first action refused after the stop. */
	refused: number | null;
	/** Whether the agent held the lease at that moment. */
	held: boolean;
}

/** What the server would have answered had you pressed the stop key `time` seconds in. Only
 * an action can be cut short: a look runs to its answer, since the stop cancels only actions. */
export function stopAt(calls: Call[], time: number): Stop {
	const states = leaseAfter(calls);
	let held = false;
	const fates = calls.map((call, index): Fate => {
		if (end(call) <= time) {
			held = states[index] === 'held';
			return 'done';
		}
		const kind = afterStop(call);
		if (call.start > time) return kind;
		return kind === 'refused' ? 'cut' : 'done';
	});
	return {
		fates,
		cut: nullable(fates.indexOf('cut')),
		refused: nullable(fates.indexOf('refused')),
		held,
	};
}

/** What a call that starts after the stop gets: an action is refused, a look still answers,
 * and giving the lease back, or anything you did yourself, doesn't happen. */
function afterStop(call: Call): Fate {
	if (call.you || tool(call) === 'release_desktop') return 'void';
	return LOOKS.has(tool(call)) ? 'answered' : 'refused';
}

const nullable = (index: number) => (index === -1 ? null : index);

/** The last call started by `time`, or -1 before the first. Of calls that start together,
 * the last one. */
export function callAt(calls: Call[], time: number): number {
	let index = -1;
	while (index + 1 < calls.length && calls[index + 1].start <= time) index++;
	return index;
}

/** The call one step from `from` in `direction`, kept within the run; from -1, the first. */
export function step(calls: Call[], from: number, direction: 1 | -1): number {
	if (from < 0) return 0;
	return Math.min(calls.length - 1, Math.max(0, from + direction));
}

/** `time` kept within the diagram's `duration`; a recording runs a little past its last call. */
export const clamp = (time: number, duration: number) => Math.min(duration, Math.max(0, time));

/** The calls with only what `stopAt` reads of them, for the page's script. */
export const brief = (calls: Call[]): Call[] =>
	calls.map((call) => ({
		...call,
		request: isStop(call) ? `${tool(call)} stop` : tool(call),
		answer: call.answer.slice(0, 12),
	}));
