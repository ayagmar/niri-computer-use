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
	/** The phone's crop of the poster by width, AVIF, when the recording has one. */
	cropped?: string;
	webm: string;
	mp4: string;
}

const runs = import.meta.glob<Run>('../../public/demo/*.json', { eager: true, import: 'default' });
const stills = import.meta.glob<ImageMetadata>(['../../public/demo/*.png', '!../../public/demo/og.png'], {
	eager: true,
	import: 'default',
});

/** Widths the stills are made at. */
export const STILL_WIDTHS = [640, 800, 960, 1280, 1920];

/** Which half of the recording's width a phone shows, so it stays legible: the left, middle
 * or right half. */
export type Crop = 'left' | 'centre' | 'right';

/** How far a crop's left edge sits from the recording's, as a fraction of its width. */
export const CROP_LEFT: Record<Crop, number> = { left: 0, centre: 0.25, right: 0.5 };

/** Widths the phone's crop is made at, up to the half of the recording it shows. */
const CROP_WIDTHS = [480, 800, 960];

export async function loadDemo(name: string, widths: number[], crop?: Crop): Promise<Demo> {
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
		cropped: crop && (await cropped(still, crop)),
		webm: `${base}demo/${name}.webm`,
		mp4: `${base}demo/${name}.mp4`,
	};
}

/** Half the still's width at its full height, cut where `crop` says: the same part of the
 * frame the video shows at twice the width of a phone. */
async function cropped(still: ImageMetadata, crop: Crop): Promise<string> {
	const images = await Promise.all(
		CROP_WIDTHS.map((width) =>
			getImage({
				src: still,
				width,
				height: Math.round((width * still.height * 2) / still.width),
				fit: 'cover',
				position: crop,
				format: 'avif',
				quality: 50,
			}),
		),
	);
	return images.map((image, index) => `${image.src} ${CROP_WIDTHS[index]}w`).join(', ');
}

/** Who holds the lease after each call: taken by `acquire_desktop`, given back by
 * `release_desktop`, taken back by the user's stop command. */
export function leaseAfter(calls: Call[]): ('free' | 'held' | 'stopped')[] {
	let lease: 'free' | 'held' | 'stopped' = 'free';
	return calls.map((call) => {
		if (call.you && call.request.endsWith(' stop')) lease = 'stopped';
		else if (call.request.startsWith('acquire_desktop') && call.answer.startsWith('lease taken')) lease = 'held';
		else if (call.request.startsWith('release_desktop') && call.answer.startsWith('released')) lease = 'free';
		return lease;
	});
}

/** Seconds into the recording as m:ss.s, as a player shows them. */
export const at = (seconds: number) =>
	`${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(1).padStart(4, '0')}`;
