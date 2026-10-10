import type { APIRoute } from 'astro';
import { markdown, markdownUrl, pages, text } from '../lib/markdown';

export const GET: APIRoute = async () => {
	const all = await pages();
	const parts = all.map((page) => `<!-- ${markdownUrl(page)} -->\n\n${markdown(page)}`);
	return text(parts.join('\n---\n\n'), 'text/plain');
};
