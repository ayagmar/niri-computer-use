// Plays the drawn errand: one frame loop sets the drawing and the call lines from
// `sceneAt` while the stage is in view, unless motion is reduced or the visitor paused it.
// "Watch the recording" plays the real recording in the same frame, and the lines follow
// it. Printing shows the finished errand.

import { callState } from '../lib/replay';
import { end, loop, sceneAt, type Scene } from '../lib/stage';

const stage = document.querySelector<HTMLElement>('[data-stage]');
const screen = stage?.querySelector<HTMLElement>('.screen');
if (stage && screen) play(stage, screen);

const svg = 'http://www.w3.org/2000/svg';

function play(stage: HTMLElement, screen: HTMLElement) {
	const q = <T extends Element>(s: string) => stage.querySelector<T>(s)!;
	const rows = [...stage.querySelectorAll<HTMLElement>('[data-start]')];
	const clock = q<SVGTextElement>('[data-clock]');
	const leaseText = q<SVGTextElement>('[data-lease-text]');
	const typed = q<SVGTextElement>('[data-typed]');
	const keys = q<SVGGElement>('[data-keys]');
	const pointer = q<SVGGElement>('[data-pointer]');
	const again = q<HTMLButtonElement>('[data-replay]');
	const recording = q<HTMLAnchorElement>('[data-recording]');
	const motion = matchMedia('(prefers-reduced-motion: no-preference)');
	let shownKeys = '';

	function setKeys(list: string[]) {
		const id = list.join('+');
		if (id == shownKeys) return;
		shownKeys = id;
		keys.replaceChildren();
		let x = 0;
		const chips = list.map((key) => {
			const w = key.length * 8 + 18;
			const g = document.createElementNS(svg, 'g');
			g.innerHTML = `<rect x="${x}" y="0" width="${w}" height="26" rx="4"/><text x="${x + w / 2}" y="18" text-anchor="middle">${key}</text>`;
			x += w + 8;
			return g;
		});
		keys.append(...chips);
		keys.setAttribute('transform', `translate(${960 - (x - 8) / 2} 650)`);
	}

	function show(s: Scene, t: number) {
		Object.assign(screen.dataset, {
			focus: s.focus,
			lease: String(s.lease),
			selected: String(s.selected),
			menu: s.menu,
			dialog: String(s.dialog),
			nameSelected: String(s.nameSelected),
			folder: String(s.folder),
		});
		leaseText.textContent = s.lease ? 'agent holds the lease' : 'no agent';
		typed.textContent = s.typed;
		setKeys(s.keys);
		pointer.style.display = s.pointer ? '' : 'none';
		if (s.pointer) {
			const [x, y] = s.pointer;
			q('[data-guide-x]').setAttribute('x1', String(x));
			q('[data-guide-x]').setAttribute('x2', String(x));
			q('[data-guide-y]').setAttribute('y1', String(y));
			q('[data-guide-y]').setAttribute('y2', String(y));
			q('[data-dot]').setAttribute('cx', String(x));
			q('[data-dot]').setAttribute('cy', String(y));
			const coords = q<SVGTextElement>('[data-coords]');
			coords.setAttribute('x', String(x + 14));
			coords.setAttribute('y', String(y - 12));
			coords.textContent = `${Math.round(x)}, ${Math.round(y)}`;
		}
		const ring = q('[data-ring]');
		if (s.pulse) {
			const [x, y, k] = s.pulse;
			ring.setAttribute('cx', String(x));
			ring.setAttribute('cy', String(y));
			ring.setAttribute('r', String(6 + 26 * k));
			ring.setAttribute('opacity', String(1 - k));
		} else ring.setAttribute('r', '0');
		for (const row of rows) row.dataset.state = callState(t, +row.dataset.start!, +row.dataset.ms!);
		clock.textContent = t < end ? `${t.toFixed(1)} s` : '(END)';
	}

	// One frame loop, for the drawing or the recording.
	let frame = 0;
	let t0 = 0;
	let running = false;
	function run(time: () => number, again: () => boolean) {
		cancelAnimationFrame(frame);
		const step = () => {
			const t = time();
			show(sceneAt(Math.min(t, end)), t);
			if (again()) frame = requestAnimationFrame(step);
		};
		step();
	}
	function start(from = 0) {
		t0 = performance.now() - from * 1000;
		running = true;
		run(
			() => ((performance.now() - t0) / 1000) % loop,
			() => running,
		);
	}
	function stop() {
		running = false;
		cancelAnimationFrame(frame);
	}

	let held = false;
	const finished = () => show(sceneAt(end), end);
	function label() {
		again.textContent = screen.querySelector('video') ? 'back to the drawing' : running ? 'pause' : 'play';
	}

	recording.onclick = (e) => {
		e.preventDefault();
		stop();
		let video = screen.querySelector('video');
		if (!video) {
			video = Object.assign(document.createElement('video'), { src: recording.href, controls: true, muted: true, playsInline: true });
			video.setAttribute('aria-label', 'The recording of the session');
			const v = video;
			v.onplay = () => run(() => v.currentTime, () => !v.paused);
			v.onseeked = () => show(sceneAt(Math.min(v.currentTime, end)), v.currentTime);
			screen.append(v);
		}
		video.currentTime = 0;
		video.play();
		video.focus({ preventScroll: true });
		recording.hidden = true;
		again.hidden = false;
		label();
	};
	let paused = 0;
	function pause() {
		paused = ((performance.now() - t0) / 1000) % loop;
		stop();
	}
	again.onclick = () => {
		const video = screen.querySelector('video');
		if (video) {
			video.pause();
			video.remove();
			stop();
			recording.hidden = false;
			held = false;
			if (motion.matches) start();
			else {
				again.hidden = true;
				finished();
			}
		} else if (running) {
			held = true;
			pause();
		} else {
			held = false;
			start(paused);
		}
		label();
	};

	let printing = false;
	addEventListener('beforeprint', () => {
		printing = running;
		if (running) pause();
		finished();
	});
	addEventListener('afterprint', () => {
		if (printing) start(paused);
	});

	if (!motion.matches) return;
	again.hidden = false;
	label();
	// Plays while the stage is in view, from where it was, unless the visitor paused it.
	new IntersectionObserver(
		([entry]) => {
			if (held || screen.querySelector('video')) return;
			if (entry.isIntersecting && !running) start(paused);
			else if (!entry.isIntersecting && running) pause();
			label();
		},
		{ threshold: 0.3 },
	).observe(screen);
}
