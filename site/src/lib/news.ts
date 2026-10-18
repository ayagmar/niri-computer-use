// The home page's Releases table, read from CHANGELOG.md when the site is built: each released
// version, newest first, with its date and how many entries it lists under each heading.
// `## Unreleased` and anything before the first release are left out.

export interface Release {
	version: string;
	date: string;
	/** Headings in the order the changelog gives them, lowercased, with their entry counts. */
	counts: [heading: string, entries: number][];
}

export function releases(changelog: string): Release[] {
	const found: Release[] = [];
	let current: Release | undefined;
	for (const line of changelog.split('\n')) {
		const release = line.match(/^## (\d+\.\d+\.\d+) - (\d{4}-\d{2}-\d{2})\s*$/);
		if (release) {
			current = { version: release[1], date: release[2], counts: [] };
			found.push(current);
		} else if (line.startsWith('## ')) {
			current = undefined;
		} else if (current && line.startsWith('### ')) {
			current.counts.push([line.slice(4).trim().toLowerCase(), 0]);
		} else if (current && line.startsWith('- ') && current.counts.length > 0) {
			current.counts[current.counts.length - 1][1] += 1;
		}
	}
	return found;
}

/** Keep a Changelog's kinds of change; other headings, such as Upgrading, are notes. */
const KINDS = new Set(['added', 'changed', 'deprecated', 'removed', 'fixed', 'security']);

/** A release's sections with their entry counts, such as "Added 4 · Changed 2". */
export function summary(release: Release): string {
	return release.counts
		.filter(([heading]) => KINDS.has(heading))
		.map(([heading, entries]) => `${heading[0].toUpperCase()}${heading.slice(1)} ${entries}`)
		.join(' · ');
}

// The releases the home page's HISTORY lists, and `index.md` with it: the latest three.
export function history(changelog: string): Release[] {
	return releases(changelog).slice(0, 3);
}
