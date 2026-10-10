import type { APIRoute } from 'astro';
import { groups } from '../lib/groups.mjs';
import { markdownUrl, pages, siteRoot, text } from '../lib/markdown';

export const GET: APIRoute = async () => {
	const all = await pages();
	const home = all.find((page) => page.id === 'index');
	const line = (page: (typeof all)[number]) =>
		`- [${page.data.title}](${markdownUrl(page)}): ${page.data.description}`;
	const sections = groups.map((group) => {
		const listed = all.filter((page) => page.id.startsWith(`${group.directory}/`));
		return `## ${group.label}\n\n${listed.map(line).join('\n')}`;
	});
	const skill = ['SKILL.md', 'references/tools.md', 'references/errors.md']
		.map((file) => `- [skill/${file}](${new URL(`skill/${file}`, siteRoot()).href})`)
		.join('\n');
	const body = [
		`# niri-computer-use`,
		`> ${home?.data.description ?? ''}`,
		`Every page is also plain Markdown at its address with \`.md\` in place of the trailing slash. [llms-full.txt](${new URL('llms-full.txt', siteRoot()).href}) has all of them in one file.`,
		home ? line(home) : '',
		...sections,
		`## Optional\n\nThe agent skill, as published from the repository:\n\n${skill}`,
	];
	return text(`${body.filter(Boolean).join('\n\n')}\n`, 'text/plain');
};
