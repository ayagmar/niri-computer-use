// The page's choices about when its demos run, kept out of the components so
// `control.test.ts` can check them.

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
