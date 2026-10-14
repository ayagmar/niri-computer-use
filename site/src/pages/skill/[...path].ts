// The agent skill from `skills/niri-computer-use/` in the repository, published unchanged.
import type { APIRoute, GetStaticPaths } from 'astro';
import { text } from '../../lib/markdown';

const SKILL = '../../../../skills/niri-computer-use/';
const files = import.meta.glob<string>('../../../../skills/niri-computer-use/**/*.md', {
	query: '?raw',
	import: 'default',
	eager: true,
});

export const getStaticPaths: GetStaticPaths = () =>
	Object.entries(files).map(([file, body]) => ({ params: { path: file.slice(SKILL.length) }, props: { body } }));

export const GET: APIRoute = ({ props }) => text(props.body);
