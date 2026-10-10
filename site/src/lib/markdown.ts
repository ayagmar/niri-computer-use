// Plain-Markdown versions of the docs pages, for agents: the raw `.md` URLs, llms.txt and
// llms-full.txt.
import { getCollection, type CollectionEntry } from 'astro:content';
import { groups } from './groups.mjs';

type Page = CollectionEntry<'docs'>;

// The site's root URL, with a trailing slash.
export function siteRoot(): string {
	return new URL(import.meta.env.BASE_URL.replace(/\/?$/, '/'), import.meta.env.SITE).href;
}

// Every page, in sidebar order: the home page, then each group by `sidebar.order`.
export async function pages(): Promise<Page[]> {
	const all = await getCollection('docs');
	const rank = (page: Page) => {
		if (page.id === 'index') return -1;
		return groups.findIndex((group) => page.id.startsWith(`${group.directory}/`));
	};
	const unplaced = all.filter((page) => rank(page) === -1 && page.id !== 'index');
	if (unplaced.length > 0) {
		throw new Error(`pages outside every sidebar group: ${unplaced.map((p) => p.id).join(', ')}`);
	}
	return all.sort(
		(a, b) =>
			rank(a) - rank(b) ||
			(a.data.sidebar.order ?? Infinity) - (b.data.sidebar.order ?? Infinity) ||
			a.id.localeCompare(b.id),
	);
}

// The page's HTML address, absolute.
export function pageUrl(page: Page): string {
	return page.id === 'index' ? siteRoot() : new URL(`${page.id}/`, siteRoot()).href;
}

// The page's Markdown address, absolute.
export function markdownUrl(page: Page): string {
	return new URL(`${page.id}.md`, siteRoot()).href;
}

// The page as Markdown: its title and description, then its body, with relative links
// made absolute so they still work outside the site, and images reduced to their alt
// text.
export function markdown(page: Page): string {
	const base = pageUrl(page);
	const body = (page.body ?? '')
		.replace(/!\[([^\]]*)\]\([^)]*\)/g, '[Image: $1]')
		.replace(/\]\((?!https?:|mailto:|#)([^)\s]+)\)/g, (_, link: string) => `](${new URL(link, base).href})`);
	return `# ${page.data.title}\n\n> ${page.data.description}\n\n${body.trim()}\n`;
}

export function text(body: string, type = 'text/markdown'): Response {
	return new Response(body, { headers: { 'Content-Type': `${type}; charset=utf-8` } });
}
