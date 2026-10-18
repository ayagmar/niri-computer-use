// less's keys, status line and search, and the home page example's replay. Every page reads and works without this: it only adds to what is there.

import { matches } from '../lib/find';
import { keySwitch } from '../lib/keys';
import { callState } from '../lib/replay';

const root = document.documentElement;
const help = document.getElementById('less-help') as HTMLDialogElement;
const pager = document.querySelector<HTMLElement>('.less');
const msg = pager?.querySelector<HTMLElement>('.msg');
const form = pager?.querySelector<HTMLFormElement>('.find');
const input = form?.querySelector('input');

const keys = keySwitch(() => localStorage);
const toggle = help.querySelector<HTMLButtonElement>('[data-keys]')!;
const label = () => (toggle.textContent = keys.on() ? 'Turn these keys off' : 'Turn these keys on');
toggle.onclick = () => (keys.set(!keys.on()), label());
label();
for (const button of document.querySelectorAll<HTMLButtonElement>('[data-help]'))
	(button.hidden = false), (button.onclick = () => help.showModal());

const say = (text: string) => msg && (msg.textContent = text);

// The 404 names the address it has no page for, as man names the page.
const path = document.querySelector('[data-path]');
if (path) {
	let address = location.pathname;
	try {
		address = decodeURI(address);
	} catch {
		// A malformed escape is shown as it came.
	}
	path.textContent = ` for ${address.replace(import.meta.env.BASE_URL.replace(/\/?$/, '/'), '')}`;
}
const line = () => parseFloat(getComputedStyle(document.body).lineHeight) || 24;
// less's window is the screen less one line; ours keeps one more for the pager line.
const screen_ = () => innerHeight - 2 * line();
const down = (y: () => number) => () => scrollBy(0, y());

// The pager line says (END) at the bottom, as less does.
const end = () => pager?.toggleAttribute('data-end', innerHeight + scrollY >= root.scrollHeight - 2);
addEventListener('scroll', end, { passive: true });
end();

// Search: matches in reverse video, the first below the top line brought up to it, then
// n and N step through the rest.
let pattern = '';
let hits: Range[] = [];
let at = -1;
function search() {
	hits = [];
	for (const scope of document.querySelectorAll('[data-find]')) {
		const walk = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
		for (let n = walk.nextNode() as Text; n; n = walk.nextNode() as Text) {
			if (n.parentElement!.closest('.sr, .copy')) continue;
			for (const [start, end] of matches(n.data, pattern)) {
				const r = new Range();
				r.setStart(n, start);
				r.setEnd(n, end);
				if (r.getClientRects().length) hits.push(r);
			}
		}
	}
	at = -1;
	if (window.Highlight) CSS.highlights.set('find', new Highlight(...hits));
}
function find(dir: number) {
	if (!pattern) return say('No previous search pattern');
	const top = (r: Range) => r.getBoundingClientRect().top;
	const i =
		at >= 0
			? at + dir
			: dir > 0
				? hits.findIndex((r) => top(r) > line())
				: hits.findLastIndex((r) => top(r) < 0);
	if (!hits[i]) return say('Pattern not found');
	at = i;
	scrollBy(0, top(hits[i]) - line());
}
if (form && input) {
	form.onsubmit = (e) => {
		e.preventDefault();
		// An empty pattern repeats the last search, as in less.
		if (input.value) (pattern = input.value), search();
		input.blur();
		find(1);
	};
	input.onkeydown = (e) => {
		if (e.key == 'Escape') input.blur();
	};
	input.onblur = () => {
		form.hidden = true;
	};
}

// Each command gets a copy button, and the pager line says what was copied.
for (const pre of document.querySelectorAll<HTMLElement>('[data-find] pre')) {
	const text = pre.textContent!;
	const words = text.split(/\s+/);
	const short = words.length > 4 ? `${words[0]} ${words[1]} … ${words.slice(-2).join(' ')}` : text;
	const button = Object.assign(document.createElement('button'), { type: 'button', className: 'copy', textContent: 'copy' });
	button.setAttribute('aria-label', `copy ${short}`);
	button.onclick = () =>
		navigator.clipboard.writeText(text).then(
			() => say(`copied: ${short}`),
			() => say('Copy failed: the browser refused the clipboard'),
		);
	pre.append(button);
}

// The example's calls replay at the times they went out, each in reverse video for its
// round trip, once the window is in view and unless motion is reduced. The recording opens
// under them as a second window when asked for, and then the rows follow it instead.
const rows = [...document.querySelectorAll<HTMLElement>('[data-start]')];
const win = document.querySelector<HTMLElement>('[data-replay]');
const motion = matchMedia('(prefers-reduced-motion: no-preference)');
const clock = win?.querySelector<HTMLElement>('.clock');
const again = win?.querySelector<HTMLButtonElement>('[data-again]');
const last = Math.max(...rows.map((row) => +row.dataset.start! + Math.max(+row.dataset.ms! / 1000, 0.5)));
const mark = (t: number) => {
	for (const row of rows) row.dataset.state = callState(t, +row.dataset.start!, +row.dataset.ms!);
	if (clock) clock.textContent = t < last ? `${t.toFixed(1)} s` : '(END)';
};
// One frame loop at a time, for the replay or the recording.
let frame = 0;
function run(time: () => number, going: () => boolean) {
	cancelAnimationFrame(frame);
	const step = () => {
		mark(time());
		if (going()) frame = requestAnimationFrame(step);
	};
	step();
}
function replay() {
	const t0 = performance.now();
	const t = () => (performance.now() - t0) / 1000;
	run(t, () => t() < last);
}
function recording() {
	let video = document.querySelector<HTMLVideoElement>('.win video');
	const a = win?.querySelector<HTMLAnchorElement>('[data-play]');
	if (video || !win || !a) return video;
	video = Object.assign(document.createElement('video'), { src: a.href, controls: true, muted: true, playsInline: true });
	video.setAttribute('aria-label', 'The recording of the session');
	const v = video;
	v.onplay = () => run(() => v.currentTime, () => !v.paused);
	v.onseeked = () => mark(v.currentTime);
	const box = Object.assign(document.createElement('div'), { className: 'win focused' });
	box.append(v);
	win.after(box);
	return video;
}
function play(at: number) {
	const video = recording();
	if (!video) return;
	video.currentTime = at;
	video.play();
	video.focus({ preventScroll: true });
	video.parentElement!.scrollIntoView({ block: 'nearest', behavior: motion.matches ? 'smooth' : 'auto' });
}
for (const a of document.querySelectorAll<HTMLAnchorElement>('[data-play]'))
	a.onclick = (e) => {
		e.preventDefault();
		play(0);
	};
// Each call's time plays the recording from that call.
for (const row of rows) {
	const cell = row.firstElementChild as HTMLElement;
	const seek = Object.assign(document.createElement('button'), { type: 'button', textContent: cell.textContent });
	seek.setAttribute('aria-label', `Play the recording from ${cell.textContent} s`);
	seek.onclick = () => play(+row.dataset.start!);
	cell.replaceChildren(seek);
}
// The replay button shows wherever the replay runs, and stays while it does, so focus
// isn't lost from it.
if (again && motion.matches) {
	again.hidden = false;
	again.onclick = () => {
		const video = document.querySelector<HTMLVideoElement>('.win video');
		if (video) play(0);
		else replay();
	};
}
if (win && rows.length && motion.matches) {
	mark(0);
	const seen = new IntersectionObserver(
		([entry]) => {
			if (!entry.isIntersecting) return;
			seen.disconnect();
			// Not over a replay or recording the visitor already started.
			if (!frame && !document.querySelector('.win video')) replay();
		},
		{ threshold: 0.4 },
	);
	seen.observe(win);
}

const actions: Record<string, () => unknown> = {
	j: down(line),
	k: down(() => -line()),
	f: down(screen_),
	b: down(() => -screen_()),
	d: down(() => screen_() / 2),
	u: down(() => -screen_() / 2),
	g: () => scrollTo(0, 0),
	G: () => scrollTo(0, root.scrollHeight),
	'?': () => help.showModal(),
	q: () =>
		help.open
			? help.close()
			: document.referrer.startsWith(location.origin) && history.length > 1
				? history.back()
				: say('Nothing to go back to'),
};
actions.e = actions.j;
actions.y = actions.k;
actions['<'] = actions.g;
actions['>'] = actions.G;
// On the docs, / opens Starlight's search, which itself only listens for Ctrl+K.
const docsSearch = document.querySelector<HTMLButtonElement>('site-search button[data-open-modal]');
if (docsSearch) actions['/'] = () => docsSearch.click();
if (form && input) {
	actions['/'] = () => ((form.hidden = false), (input.value = ''), input.focus({ preventScroll: true }));
	actions.n = () => find(1);
	actions.N = () => find(-1);
}

addEventListener('keydown', (e) => {
	const act = actions[e.key];
	if (!act || e.ctrlKey || e.altKey || e.metaKey || e.defaultPrevented || !keys.on()) return;
	if ((e.target as Element).closest?.('input, textarea, select, video, [contenteditable]')) return;
	if (help.open && e.key != 'q') return;
	e.preventDefault();
	say('');
	act();
});
