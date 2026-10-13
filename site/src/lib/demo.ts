// The demo recordings in `public/demo/`. A recording is `<name>.webm`, `<name>.mp4`,
// `<name>.png` (a still, which also gives the video's size) and `<name>.json` (the prompt and
// the calls, timed from the start of the video). Replacing those four files replaces the
// recording on the site.
import type { ImageMetadata } from 'astro';
import { getImage } from 'astro:assets';

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
	/** How the recording was made, shown under its calls. */
	note?: string;
	/** The recorded output, such as `winit · 1280×720 at 0,0`. */
	output?: string;
}

export interface Demo extends Run {
	name: string;
	width: number;
	height: number;
	/** Poster images by width, AVIF. */
	poster: { src: string; srcset: string };
	webm: string;
	mp4: string;
}

const runs = import.meta.glob<Run>('../../public/demo/*.json', { eager: true, import: 'default' });
const stills = import.meta.glob<ImageMetadata>(['../../public/demo/*.png', '!../../public/demo/og.png'], {
	eager: true,
	import: 'default',
});

/** The hero recording's still, by width, and its slot: beside the copy or under it. */
export const HERO_WIDTHS = [480, 720, 960, 1280];
export const HERO_SIZES = '(min-width: 62rem) 52vw, 96vw';

export async function loadDemo(name: string, widths: number[]): Promise<Demo> {
	const run = runs[`../../public/demo/${name}.json`];
	const still = stills[`../../public/demo/${name}.png`];
	if (!run || !still) throw new Error(`public/demo/${name}.json or .png is missing`);
	const posters = await Promise.all(
		widths.map((width) => getImage({ src: still, width, format: 'avif', quality: 50 })),
	);
	const base = import.meta.env.BASE_URL.replace(/\/?$/, '/');
	return {
		...run,
		name,
		width: still.width,
		height: still.height,
		poster: {
			src: posters.at(-1)!.src,
			srcset: posters.map((poster, index) => `${poster.src} ${widths[index]}w`).join(', '),
		},
		webm: `${base}demo/${name}.webm`,
		mp4: `${base}demo/${name}.mp4`,
	};
}

/** Seconds into the recording as m:ss.s, as a player shows them. */
export const at = (seconds: number) =>
	`${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(1).padStart(4, '0')}`;
