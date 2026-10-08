import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
	site: 'https://ayagmar.github.io',
	base: '/niri-computer-use',
	integrations: [
		starlight({
			title: 'niri-computer-use',
			description: 'An MCP server that lets AI agents see a niri Wayland desktop.',
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/ayagmar/niri-computer-use' },
			],
			sidebar: [
				{ label: 'Getting started', slug: 'guides/getting-started' },
				{ label: 'Tools reference', slug: 'reference/tools' },
			],
		}),
	],
});
