// Where a call in the replayed clip stands at a moment `t` seconds in: still to come, in
// flight, or made. A call shows as in flight for at least half a second, so one of a few
// milliseconds can still be seen; its row gives the real round trip.
export type CallState = 'next' | 'now' | 'done';

export function callState(t: number, start: number, ms: number): CallState {
	if (t < start) return 'next';
	return t < start + Math.max(ms / 1000, 0.5) ? 'now' : 'done';
}
