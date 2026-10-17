// less's keys, status line and search, niri's columns on the home page, and the example's
// replay. Every page reads and works without this: it only adds to what is there.

import { callState } from '../lib/replay';

const root = document.documentElement;
const help = document.getElementById('less-help') as HTMLDialogElement;
const pager = document.querySelector<HTMLElement>('.less');
const msg = pager?.querySelector<HTMLElement>('.msg');
const form = pager?.querySelector<HTMLFormElement>('.find');
const input = form?.querySelector('input');
const strip = document.querySelector<HTMLElement>('.strip');
const columns = strip ? ([...strip.children] as HTMLElement[]) : [];
const wide = matchMedia('(min-width: 75rem)');

// Whether the keys are on, kept across pages; a browser that refuses storage keeps them on.
const keys = (off?: boolean): boolean => {
	try {
		if (off !== undefined) localStorage.ncuKeys = off ? 'off' : 'on';
		return localStorage.ncuKeys != 'off';
	} catch {
		return true;
	}
};
const toggle = help.querySelector<HTMLButtonElement>('[data-keys]')!;
const label = () => (toggle.textContent = keys() ? 'Turn these keys off' : 'Turn these keys on');
toggle.onclick = () => (keys(keys()), label());
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
// The keys scroll the focused EXAMPLE column when it scrolls on its own, else the page.
const pane = () => (root.dataset.column == '1' && wide.matches ? columns[1] : null);
// less's window is the screen less one line; ours keeps one more for the pager line.
const screen_ = () => (pane()?.clientHeight ?? innerHeight) - 2 * line();
const down = (y: () => number) => () => (pane() ?? window).scrollBy(0, y());

// The pager line says (END) at the bottom, as less does.
const end = () => pager?.toggleAttribute('data-end', innerHeight + scrollY >= root.scrollHeight - 2);
addEventListener('scroll', end, { passive: true });
end();

// Search: matches in reverse video, the first below the top line brought up to it, then
// n and N step through the rest.
let pattern = '';
let hits: Range[] = [];
let at = -1;
const scope = document.querySelector('[data-find]');
function search() {
	hits = [];
	const p = pattern.toLowerCase();
	const walk = document.createTreeWalker(scope!, NodeFilter.SHOW_TEXT);
	for (let n = walk.nextNode() as Text; n; n = walk.nextNode() as Text) {
		if (n.parentElement!.closest('.sr')) continue;
		const text = n.data.toLowerCase();
		for (let i = text.indexOf(p); i >= 0; i = text.indexOf(p, i + 1)) {
			const r = new Range();
			r.setStart(n, i);
			r.setEnd(n, i + p.length);
			if (r.getClientRects().length) hits.push(r);
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
		pattern = input.value;
		input.blur();
		if (pattern) search();
		find(1);
	};
	input.onkeydown = (e) => {
		if (e.key == 'Escape') input.blur();
	};
	input.onblur = () => {
		form.hidden = true;
	};
}

// niri's columns: the focused one wears the ring, and the workspace scrolls to keep it in
// view, whether a key, a click or Tab moved focus into it. `data-column` overrides an
// #example in the address, which CSS also follows.
function focusColumn(i: number, move = true) {
	if (!wide.matches || !columns[i]) return;
	root.dataset.column = String(i);
	if (move) columns[i].focus({ preventScroll: true });
}
const columnOf = (node: EventTarget | null) => columns.findIndex((c) => c.contains(node as Node));
strip?.addEventListener('focusin', (e) => focusColumn(columnOf(e.target), false));
strip?.addEventListener(
	'wheel',
	(e) => Math.abs(e.deltaX) > Math.abs(e.deltaY) + 4 && focusColumn(e.deltaX > 0 ? 1 : 0),
	{ passive: true },
);
for (const a of document.querySelectorAll<HTMLAnchorElement>('a[href="#example"]'))
	a.onclick = (e) => {
		if (wide.matches) e.preventDefault(), focusColumn(1);
	};

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
function track(video: HTMLVideoElement) {
	const t = video.currentTime;
	for (const row of rows) {
		row.dataset.state = callState(t, +row.dataset.start!, +row.dataset.ms!);
	}
	if (!video.paused) requestAnimationFrame(() => track(video));
}
for (const a of document.querySelectorAll<HTMLAnchorElement>('[data-play]'))
	a.onclick = (e) => {
		e.preventDefault();
		focusColumn(1);
		let video = document.querySelector('video');
		if (!video) {
			const still = document.querySelector('.still')!;
			video = Object.assign(document.createElement('video'), {
				src: a.href,
				poster: still.querySelector('img')!.currentSrc,
				controls: true,
				muted: true,
				playsInline: true,
			});
			video.setAttribute('aria-label', 'The session, playing');
			const v = video;
			v.onplay = v.onseeked = v.onended = () => track(v);
			still.replaceWith(video);
		}
		video.play();
	};

const actions: Record<string, () => unknown> = {
	j: down(line),
	k: down(() => -line()),
	f: down(screen_),
	b: down(() => -screen_()),
	d: down(() => screen_() / 2),
	u: down(() => -screen_() / 2),
	g: () => (pane() ?? window).scrollTo(0, 0),
	G: () => (pane() ?? window).scrollTo(0, (pane() ?? root).scrollHeight),
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
if (form && input) {
	actions['/'] = () => ((form.hidden = false), (input.value = ''), input.focus({ preventScroll: true }));
	actions.n = () => find(1);
	actions.N = () => find(-1);
}
if (strip) {
	actions.ArrowRight = () => focusColumn(1);
	actions.ArrowLeft = () => focusColumn(0);
}

addEventListener('keydown', (e) => {
	const act = actions[e.key];
	if (!act || e.ctrlKey || e.altKey || e.metaKey || e.defaultPrevented || !keys()) return;
	if ((e.target as Element).closest?.('input, textarea, select, video, [contenteditable]')) return;
	if (help.open && e.key != 'q') return;
	if (e.key.startsWith('Arrow') && !wide.matches) return;
	e.preventDefault();
	say('');
	act();
});
