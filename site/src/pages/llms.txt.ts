import type { APIRoute } from 'astro';
import { markdownUrl, pages, siteRoot, text } from '../lib/markdown';

// Pages by who needs them first: an agent about to use the tools, then whoever installs
// the server. Pages named nowhere here are listed under "More", in sidebar order, so a new
// page can't drop out.
const using = ['tools/overview', 'reference/errors'];
const installing = [
	'start/install',
	'start/clients',
	'start/first-session',
	'reference/troubleshooting',
	'concepts/configuration',
];
const optional = ['tools/elements', 'tools/keyboard', 'tools/acting', 'reference/cli', 'concepts/noctalia'];
// The agent skill, as published from the repository.
const skill = [
	['SKILL.md', 'load this first: how to look, take the lease, act one call at a time and hand back'],
	['references/tools.md', 'every tool, its arguments and what its result means'],
	['references/errors.md', 'each error name and what to do next'],
];

export const GET: APIRoute = async () => {
	const all = await pages();
	const home = all.find((page) => page.id === 'index');
	const line = (page: (typeof all)[number]) =>
		page.id === 'index'
			? `- [Home page](${markdownUrl(page)}): what the server does, what it needs, its limits, and install in five steps`
			: `- [${page.data.title}](${markdownUrl(page)}): ${page.data.description}`;
	const list = (ids: string[]) =>
		ids.map((id) => {
			const page = all.find((candidate) => candidate.id === id);
			if (!page) throw new Error(`llms.txt names ${id}, which is no page`);
			return line(page);
		});
	const named = new Set([...using, ...installing, ...optional]);
	const more = all.filter((page) => !named.has(page.id)).map(line);
	const skillLines = skill.map(
		([file, what]) => `- [skill/${file}](${new URL(`skill/${file}`, siteRoot()).href}): ${what}`,
	);
	const body = [
		`# niri-computer-use`,
		`> ${home?.data.description ?? ''}`,
		`It runs only on Linux, under niri 26.04 on Wayland, built from source; with any other niri version it only looks. Every page is also plain Markdown at its address with \`.md\` in place of the trailing slash. [llms-full.txt](${new URL('llms-full.txt', siteRoot()).href}) has all of them in one file.`,
		`## Using the tools (for agents operating a desktop)\n\n${[...skillLines, ...list(using)].join('\n')}`,
		`## Installing for a user\n\n${list(installing).join('\n')}`,
		`## More\n\n${more.join('\n')}`,
		`## Optional\n\n${list(optional).join('\n')}`,
	];
	return text(`${body.join('\n\n')}\n`, 'text/plain');
};
