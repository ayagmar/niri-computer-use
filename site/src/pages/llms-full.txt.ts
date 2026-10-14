import type { APIRoute } from 'astro';
import { homeMarkdown, homeMarkdownUrl, markdown, markdownUrl, pages, text } from '../lib/markdown';

export const GET: APIRoute = async () => {
	const parts = [
		`<!-- ${homeMarkdownUrl()} -->\n\n${homeMarkdown()}`,
		...(await pages()).map((page) => `<!-- ${markdownUrl(page)} -->\n\n${markdown(page)}`),
	];
	return text(parts.join('\n---\n\n'), 'text/plain');
};
