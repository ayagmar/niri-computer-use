// Where `pattern` occurs in `text`, ignoring case, as offsets into `text` itself. Matching
// the text as it is, rather than a lowercased copy, keeps the offsets right where
// lowercasing changes a string's length, as it does for `İ`.
export function matches(text: string, pattern: string): [number, number][] {
	if (!pattern) return [];
	const literal = new RegExp(pattern.replace(/[\\^$.*+?()[\]{}|/]/g, '\\$&'), 'giu');
	return [...text.matchAll(literal)].map((m) => [m.index, m.index + m[0].length]);
}
