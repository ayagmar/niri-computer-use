import type { APIRoute } from 'astro';
import { home } from '../lib/home';
import { homeMarkdownUrl, markdownUrl, pages, siteRoot, text } from '../lib/markdown';

// Pages by who needs them first: an agent about to use the tools, then whoever installs
// the server. Pages named nowhere here are listed under "More", in sidebar order, so a new
// page can't drop out.
const using = ['tools/overview', 'reference/errors'];
const installing = [
	'start/install',
	'start/install-script',
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
	const line = (page: (typeof all)[number]) => `- [${page.data.title}](${markdownUrl(page)}): ${page.data.description}`;
	const list = (ids: string[]) =>
		ids.map((id) => {
			const page = all.find((candidate) => candidate.id === id);
			if (!page) throw new Error(`llms.txt names ${id}, which is no page`);
			return line(page);
		});
	const named = new Set([...using, ...installing, ...optional]);
	const more = [
		`- [Home page](${homeMarkdownUrl()}): what the server does, five recorded sessions, what an action answers, the checks before an action, what it doesn't protect, requirements, and install in three steps`,
		...all.filter((page) => !named.has(page.id)).map(line),
	];
	const skillLines = skill.map(
		([file, what]) => `- [skill/${file}](${new URL(`skill/${file}`, siteRoot()).href}): ${what}`,
	);
	const body = [
		`# niri-computer-use`,
		`> ${home.description}`,
		`It runs only on Linux, under niri 26.04 on Wayland, from a release build for x86_64 or aarch64 or built from source; with another major or minor release of niri it only looks. Every page is also plain Markdown at its address with \`.md\` in place of the trailing slash. [llms-full.txt](${new URL('llms-full.txt', siteRoot()).href}) has all of them in one file.`,
		`If the user turned on \`confirm_lease\`, \`acquire_desktop\` waits up to a minute for them to choose Allow on the desktop, and fails with \`lease_declined\` if they don't. To read small text, call \`screenshot\` with target \`region\`, an earlier screenshot's \`screenshot_ref\`, and \`region\` in that image's pixels. A \`save_path\` saves the screenshot as a full-resolution PNG under \`~/Pictures/niri-computer-use\`, or the user's \`capture_dir\`.`,
		`## Using the tools (for agents operating a desktop)\n\n${[...skillLines, ...list(using)].join('\n')}`,
		`## Installing for a user\n\n${list(installing).join('\n')}`,
		`## More\n\n${more.join('\n')}`,
		`## Optional\n\n${list(optional).join('\n')}`,
	];
	return text(`${body.join('\n\n')}\n`, 'text/plain');
};
