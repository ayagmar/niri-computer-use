// The page's choices about when its demos run, kept out of the components so
// `control.test.ts` can check them.

/** Tickets for a start that settles later, such as a video's play(). Taking a ticket or
 * cancelling makes every earlier ticket stale, so a start a pause or a newer start overtook
 * does nothing when it settles. */
export function tickets() {
	let latest = 0;
	return {
		take: () => ++latest,
		cancel: () => void ++latest,
		current: (ticket: number) => ticket === latest,
	};
}

/** A demo that plays once when first seen, and its follow-up after it, unless the reader has
 * picked something by then: from that pick on, neither runs. */
export function demo() {
	let picked = false;
	let seen = false;
	return {
		pick: () => void (picked = true),
		/** Whether this sighting should start the demo: only the first, and only unpicked. */
		see: () => {
			const first = !seen;
			seen = true;
			return first && !picked;
		},
		/** Whether the demo's follow-up should still run. */
		follow: () => !picked,
	};
}
