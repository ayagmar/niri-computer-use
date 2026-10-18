// What the home page's drawing of the errand shows at `t` seconds into the session: which
// window has focus, where the pointer is, what Files shows and which keys are down. It is
// drawn in the agent's own screenshot pixels (1280 by 720, half the 2560 by 1440 output),
// so a click lands on the call's real coordinates. Every moment comes from a call in
// `home.ts`; what the session's apps did in between (the menu, the dialog, the folder) is
// placed where the recording shows it.

import { home } from './home.ts';

export const width = 1280;
export const height = 720;

type Call = [number, string, number];
const calls = home.example.calls as Call[];

/** The start of the first call whose text contains `needle`. */
function at(needle: string): number {
	const call = calls.find(([, text]) => text.includes(needle));
	if (!call) throw new Error(`no call matches ${needle}`);
	return call[0];
}

/** The x and y a pointer call aimed at, read from its arguments. */
function aim(needle: string): [number, number] {
	const call = calls.find(([, text]) => text.includes(needle));
	const m = call?.[1].match(/"x": (\d+), "y": (\d+)/);
	if (!m) throw new Error(`no pointer call matches ${needle}`);
	return [+m[1], +m[2]];
}

/** When a call's answer came back. */
function back(needle: string): number {
	const call = calls.find(([, text]) => text.includes(needle))!;
	return call[0] + call[2] / 1000;
}

const t = {
	lease: at('acquire_desktop'),
	focus: at('focus'),
	select: at('"x": 979'),
	extend: at('shift+Right'),
	menu: at('"button": "right"'),
	item: at('"x": 1130'),
	all: at('ctrl+a'),
	type: at('type_text'),
	typed: back('type_text'),
	release: at('release_desktop'),
};

export const clicks: [number, number, number][] = [
	[t.select, ...aim('"x": 979')],
	[t.menu, ...aim('"button": "right"')],
	[t.item, ...aim('"x": 1130')],
];

/** The session's last moment, and the loop's length with a rest at the end. */
export const end = Math.max(...calls.map(([s, , ms]) => s + Math.max(ms / 1000, 0.5)));
export const loop = end + 3;

export const folderName = 'release-0.1';

export interface Scene {
	focus: 'terminal' | 'files';
	lease: boolean;
	/** Where the pointer is, or null while the agent isn't using it. */
	pointer: [number, number] | null;
	/** The click that just landed, while its ring grows: where, and how far from 0 to 1. */
	pulse: [number, number, number] | null;
	selected: number;
	menu: 'shut' | 'open' | 'chosen';
	dialog: boolean;
	nameSelected: boolean;
	typed: string;
	folder: boolean;
	keys: string[];
}

const ease = (x: number) => 1 - (1 - x) ** 3;
const travel = 0.5;

function pointerAt(now: number): [number, number] | null {
	if (now < t.focus || now >= t.release) return null;
	// The pointer rests in Files after focus, then reaches each click as it lands.
	let from: [number, number] = [clicks[0][1], 300];
	for (const [when, x, y] of clicks) {
		if (now >= when) {
			from = [x, y];
			continue;
		}
		const k = ease(Math.min(1, Math.max(0, (now - (when - travel)) / travel)));
		return [from[0] + (x - from[0]) * k, from[1] + (y - from[1]) * k];
	}
	return from;
}

function keysAt(now: number): string[] {
	const shown: [number, number, string[]][] = [
		[t.extend, t.extend + 0.5, ['shift', 'Right']],
		[t.extend + 0.5, t.extend + 1.1, ['ctrl', 'alt', 'n']],
		[t.all, t.all + 0.7, ['ctrl', 'a']],
		[t.typed - 0.15, t.typed + 0.45, ['Return']],
	];
	return shown.find(([from, to]) => now >= from && now < to)?.[2] ?? [];
}

export function sceneAt(now: number): Scene {
	const pulse = clicks.find(([when]) => now >= when && now < when + 0.5);
	const folder = now >= t.typed;
	const typing = Math.min(1, Math.max(0, (now - t.type) / (t.typed - t.type - 0.2)));
	return {
		focus: now >= t.focus && now < t.release ? 'files' : 'terminal',
		lease: now >= t.lease && now < t.release,
		pointer: pointerAt(now),
		pulse: pulse ? [pulse[1], pulse[2], (now - pulse[0]) / 0.5] : null,
		selected: folder ? 0 : now >= t.extend ? 2 : now >= t.select ? 1 : 0,
		menu: now < t.menu + 0.15 || now >= t.item + 0.3 ? 'shut' : now >= t.item ? 'chosen' : 'open',
		dialog: now >= t.item + 0.3 && !folder,
		nameSelected: now >= t.all && now < t.type,
		typed: folderName.slice(0, Math.round(typing * folderName.length)),
		folder,
		keys: keysAt(now),
	};
}
