// The demo recordings. A recording is `public/demo/<name>.json` (the prompt and the calls,
// timed from the start of the video), `public/demo/<name>.png` (a still of the desktop with
// the caption band `scripts/demo-encode.py` draws under it) and `public/clips/<name>.mp4`,
// the desktop alone at 1280×720, which the home page plays. Replacing those files replaces
// the recording on the site.
import type { ImageMetadata } from 'astro';
import { getImage } from 'astro:assets';
import type { Run } from './timing';

export interface Clip extends Run {
	name: string;
	/** The desktop's still without the caption band, AVIF, by width. */
	poster: { src: string; srcset: string };
	mp4: string;
}

const runs = import.meta.glob<Run>('../../public/demo/*.json', { eager: true, import: 'default' });
const stills = import.meta.glob<ImageMetadata>(['../../public/demo/*.png', '!../../public/demo/og.png'], {
	eager: true,
	import: 'default',
});

/** Widths the poster is made at. */
const POSTER_WIDTHS = [480, 800, 1280];

export async function loadClip(name: string): Promise<Clip> {
	const run = runs[`../../public/demo/${name}.json`];
	const still = stills[`../../public/demo/${name}.png`];
	if (!run || !still) throw new Error(`public/demo/${name}.json or .png is missing`);
	// The desktop is 16:9 at the top of the still; the caption band is cut off.
	const posters = await Promise.all(
		POSTER_WIDTHS.map((width) =>
			getImage({
				src: still,
				width,
				height: Math.round((width * 9) / 16),
				fit: 'cover',
				position: 'top',
				format: 'avif',
				quality: 55,
			}),
		),
	);
	const base = import.meta.env.BASE_URL.replace(/\/?$/, '/');
	return {
		...run,
		name,
		poster: {
			src: posters[1].src,
			srcset: posters.map((poster, index) => `${poster.src} ${POSTER_WIDTHS[index]}w`).join(', '),
		},
		mp4: `${base}clips/${name}.mp4`,
	};
}
