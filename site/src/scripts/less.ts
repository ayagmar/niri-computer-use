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

// The example plays in place, and its calls light up as the session makes them.
const rows = [...document.querySelectorAll<HTMLElement>('[data-start]')];
const mark = (video: HTMLVideoElement) => {
	for (const row of rows) row.dataset.state = callState(video.currentTime, +row.dataset.start!, +row.dataset.ms!);
};
// One frame loop while the clip plays; a seek only marks the rows again.
let frame = 0;
function track(video: HTMLVideoElement) {
	cancelAnimationFrame(frame);
	const step = () => {
		mark(video);
		if (!video.paused) frame = requestAnimationFrame(step);
	};
	step();
}
// The clip, made from the still on first use: muted, with controls, and followed by the
// call table.
function clip() {
	const existing = document.querySelector<HTMLVideoElement>('.win video');
	const still = document.querySelector<HTMLAnchorElement>('.still');
	if (existing || !still) return existing;
	const video = Object.assign(document.createElement('video'), {
		src: still.href,
		poster: still.querySelector('img')!.currentSrc,
		controls: true,
		muted: true,
		playsInline: true,
	});
	video.setAttribute('aria-label', 'The session, playing');
	video.onplay = () => track(video);
	video.onseeked = video.onended = () => mark(video);
	const focused = still.contains(document.activeElement);
	still.replaceWith(video);
	if (focused) video.focus({ preventScroll: true });
	return video;
}
for (const a of document.querySelectorAll<HTMLAnchorElement>('[data-play]'))
	a.onclick = (e) => {
		e.preventDefault();
		clip()?.play();
	};
// Each call's time seeks the clip to it.
for (const row of rows) {
	const cell = row.firstElementChild as HTMLElement;
	const seek = Object.assign(document.createElement('button'), { type: 'button', textContent: cell.textContent });
	seek.setAttribute('aria-label', `Play from ${cell.textContent} s`);
	seek.onclick = () => {
		const video = clip();
		if (!video) return;
		video.currentTime = +row.dataset.start!;
		video.play();
	};
	cell.replaceChildren(seek);
}
// Unless motion is reduced, the clip plays on a loop while the example is in view, and
// pauses when it leaves. A pause while it is in view is the visitor's, and holds until
// they play it again.
const windows = document.querySelector('.windows');
if (windows && matchMedia('(prefers-reduced-motion: no-preference)').matches) {
	let inView = false;
	let held = false;
	new IntersectionObserver(
		([entry]) => {
			inView = entry.isIntersecting;
			const video = inView ? clip() : document.querySelector('video');
			if (!video) return;
			if (!video.loop) {
				video.loop = true;
				video.addEventListener('pause', () => (held = inView && !document.hidden));
				video.addEventListener('play', () => (held = false));
			}
			if (inView && !held) video.play().catch(() => {});
			else if (!inView) video.pause();
		},
		{ threshold: 0.5 },
	).observe(windows);
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
	h: () => help.showModal(),
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
actions['?'] = actions.h;
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
