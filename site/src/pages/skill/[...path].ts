// The agent skill from `skills/niri-computer-use/` in the repository, published unchanged.
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import type { APIRoute, GetStaticPaths } from 'astro';
import { text } from '../../lib/markdown';

// The build runs in `site/`.
const SKILL = resolve(process.cwd(), '../skills/niri-computer-use');

export const getStaticPaths: GetStaticPaths = () =>
	readdirSync(SKILL, { recursive: true, withFileTypes: true })
		.filter((entry) => entry.isFile() && entry.name.endsWith('.md'))
		.map((entry) => {
			const file = join(entry.parentPath, entry.name);
			return { params: { path: relative(SKILL, file) }, props: { file } };
		});

export const GET: APIRoute = ({ props }) => text(readFileSync(props.file, 'utf8'));
