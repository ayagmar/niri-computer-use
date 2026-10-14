// Plain-Markdown versions of the docs pages, for agents: the raw `.md` URLs, llms.txt and
// llms-full.txt.
import { getCollection, type CollectionEntry } from 'astro:content';
import { groups } from './groups.mjs';
import { home } from './home';

type Page = CollectionEntry<'docs'>;

// The site's root URL, with a trailing slash.
export function siteRoot(): string {
	return new URL(import.meta.env.BASE_URL.replace(/\/?$/, '/'), import.meta.env.SITE).href;
}

// Every docs page, in sidebar order: each group by `sidebar.order`.
export async function pages(): Promise<Page[]> {
	const all = await getCollection('docs');
	const rank = (page: Page) => groups.findIndex((group) => page.id.startsWith(`${group.directory}/`));
	const unplaced = all.filter((page) => rank(page) === -1);
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
	return new URL(`${page.id}/`, siteRoot()).href;
}

// The home page's Markdown address, absolute.
export function homeMarkdownUrl(): string {
	return new URL('index.md', siteRoot()).href;
}

// The home page as Markdown, from the same words the page sets.
export function homeMarkdown(): string {
	const url = (link: string) => new URL(link, siteRoot()).href;
	const { stop, clips, install } = home;
	return [
		`# niri-computer-use`,
		`> ${home.description}`,
		`${home.headline.join(' ')} ${home.lede}`,
		`The recordings on the page are real sessions. ${home.note}`,
		`## ${stop.headline.join(' ')}`,
		`${stop.lede}\n\n${stop.facts.map((fact) => `- ${fact}`).join('\n')}\n\n[${stop.more.text}](${url(stop.more.link)})`,
		`## ${clips.headline}`,
		clips.items.map((clip) => `- ${clip.text}`).join('\n'),
		`## ${install.headline}`,
		install.lede,
		install.steps
			.map((step, index) => {
				const more = step.more ? ` [${step.more.text}](${url(step.more.link)})` : '';
				return `${index + 1}. ${step.text}${more}\n\n   \`\`\`${step.lang}\n${step.code.replace(/^/gm, '   ')}\n   \`\`\``;
			})
			.join('\n\n'),
		`[${install.more.text}](${url(install.more.link)})`,
	].join('\n\n') + '\n';
}

// The page's Markdown address, absolute.
export function markdownUrl(page: Page): string {
	return new URL(`${page.id}.md`, siteRoot()).href;
}

// The page as Markdown: its title and description, then its body, with relative links
// made absolute so they still work outside the site, images reduced to their alt text,
// and an MDX page's imports and component tags left out, keeping what they wrap.
export function markdown(page: Page): string {
	const base = pageUrl(page);
	const source = page.filePath?.endsWith('.mdx') ? withoutMdx(page.body ?? '') : (page.body ?? '');
	const body = source
		.replace(/!\[([^\]]*)\]\([^)]*\)/g, '[Image: $1]')
		.replace(/\]\((?!https?:|mailto:|#)([^)\s]+)\)/g, (_, link: string) => `](${new URL(link, base).href})`);
	return `# ${page.data.title}\n\n> ${page.data.description}\n\n${body.trim()}\n`;
}

// MDX without its imports and its lines that hold only a component's opening or closing
// tag.
function withoutMdx(body: string): string {
	return body
		.replace(/^import .*\n/gm, '')
		.replace(/^[ \t]*<\/?[A-Za-z][^>]*>[ \t]*(\n|$)/gm, '')
		.replace(/\n{3,}/g, '\n\n');
}

export function text(body: string, type = 'text/markdown'): Response {
	return new Response(body, { headers: { 'Content-Type': `${type}; charset=utf-8` } });
}
