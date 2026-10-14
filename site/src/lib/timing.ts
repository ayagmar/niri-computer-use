// A recording's calls as a timing diagram: one lane for the agent's calls, one for the
// lease, one for what niri saw, one for what you did. Pure, so the page draws it at build
// time and the browser only moves a cursor over it.
import { leaseAfter, type Call, type Run } from './demo';

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
	x1: number;
	error: boolean;
	/** Index into the run's calls. */
	call: number;
	/** What the call would get once the stop flag is set: an action is refused, a look still
	 * answers, and giving the lease back has nothing to give. */
	afterStop: 'refused' | 'answered' | 'moot';
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
	niri: Label[];
	you: Pulse[];
	/** Each call's start in seconds, for the cursor and the readout. */
	starts: number[];
}

/** The diagram of `run`. */
export function timing(run: Run): Timing {
	const end = Math.max(...run.calls.map((call) => call.start + (call.ms ?? 0) / 1000));
	const duration = Math.ceil(end + 1);
	const x = (seconds: number) => (WIDTH * seconds) / duration;
	const calls = rows(
		run.calls.flatMap((call, index) => (call.you ? [] : [pulse(call, index, x)])),
	);
	const you = rows(run.calls.flatMap((call, index) => (call.you ? [userPulse(call, index, x)] : [])));
	const niri = rows(
		run.calls.flatMap((call) => {
			const text = saw(call);
			return text ? [{ x: x(call.start + (call.ms ?? 0) / 1000), row: 0, text }] : [];
		}),
	);
	const states = leaseAfter(run.calls);
	const lease = [{ x: 0, state: 'free' as const }];
	run.calls.forEach((call, index) => {
		if (states[index] !== lease.at(-1)!.state) {
			lease.push({ x: x(call.start + (call.ms ?? 0) / 1000), state: states[index] });
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
		starts: run.calls.map((call) => call.start),
	};
}

function pulse(call: Call, index: number, x: (s: number) => number): Pulse {
	const start = x(call.start);
	// A call too quick to see still gets a pulse a few units wide.
	const x1 = Math.max(start + 4, x(call.start + (call.ms ?? 0) / 1000));
	const tool = call.request.split(' ')[0];
	const afterStop = LOOKS.has(tool) ? 'answered' : tool === 'release_desktop' ? 'moot' : 'refused';
	return { x: start, x1, row: 0, text: tool, error: call.error, call: index, afterStop };
}

function userPulse(call: Call, index: number, x: (s: number) => number): Pulse {
	const start = x(call.start);
	const text = call.request.endsWith(' stop') ? 'stop key' : call.answer.split(';')[0].replace(/^you /, '');
	return { x: start, x1: start + 14, row: 0, text, error: false, call: index, afterStop: 'moot' };
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
