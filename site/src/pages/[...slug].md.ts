import type { APIRoute, GetStaticPaths } from 'astro';
import { markdown, pages, text } from '../lib/markdown';

export const getStaticPaths: GetStaticPaths = async () =>
	(await pages()).map((page) => ({ params: { slug: page.id }, props: { page } }));

export const GET: APIRoute = ({ props }) => text(markdown(props.page));
