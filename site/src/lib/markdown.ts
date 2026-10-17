// Plain-Markdown versions of the docs pages, for agents: the raw `.md` URLs, llms.txt and
// llms-full.txt.
import { getCollection, type CollectionEntry } from 'astro:content';
import { groups } from './groups.mjs';
import { home } from './home';
import { releaseLink, version } from './version';

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
	const { glance, install, safety } = home;
	const list = (items: string[]) => items.map((item) => `- ${item}`).join('\n');
	const code = (lang: string, text: string) => `   \`\`\`${lang}\n${text.replace(/^/gm, '   ')}\n   \`\`\``;
	return [
		`# niri-computer-use`,
		`> ${home.description}`,
		`Version ${version}: [release notes](${releaseLink})`,
		`${home.tagline} ${home.stop} ${home.only}`,
		list(glance.map(([what, value]) => `${what}: ${value}`)),
		`## Install`,
		install
			.map((step, index) => {
				const note = step.note ? ` ${step.note}` : '';
				const more = step.more ? ` [${step.more.text}](${url(step.more.link)})` : '';
				const read = step.read
					? `\n\n   Read it:\n\n${code('sh', step.read)}\n\n   Then run it:\n\n${code('sh', step.run)}`
					: '';
				const or = step.or ? `\n\n   ${step.orNote}\n\n${code('sh', step.or)}` : '';
				return `${index + 1}. ${step.label}.${note}${more}\n\n${code(step.lang, step.code)}${read}${or}`;
			})
			.join('\n\n'),
		`## Safety`,
		`Enforced:\n\n${list(safety.enforced)}\n\nNot covered:\n\n${list(safety.missed)}\n\n[${safety.more.text}](${url(safety.more.link)})`,
		`[Documentation](${url('start/install/')}) · [Tools](${url('tools/overview/')}) · [Configuration](${url('concepts/configuration/')})`,
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
