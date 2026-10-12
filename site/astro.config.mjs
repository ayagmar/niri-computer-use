import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { groups } from './src/lib/groups.mjs';

export default defineConfig({
	site: 'https://ayagmar.github.io',
	base: '/niri-computer-use',
	// The pages these addresses had before the site was split by topic.
	redirects: {
		'/guides/getting-started': '/niri-computer-use/start/install/',
		'/reference/tools': '/niri-computer-use/tools/overview/',
	},
	integrations: [
		starlight({
			title: 'niri-computer-use',
			description:
				'An MCP server that lets AI agents see and act on a niri Wayland desktop, with a lease, a stop key and an audit log.',
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/ayagmar/niri-computer-use' },
			],
			customCss: [
				'@fontsource-variable/archivo/wght.css',
				'@fontsource-variable/jetbrains-mono/wght.css',
				'./src/styles/theme.css',
			],
			components: { Hero: './src/components/Hero.astro' },
			// The card link previews show, from the hero demo's recording.
			head: [
				{
					tag: 'meta',
					attrs: { property: 'og:image', content: 'https://ayagmar.github.io/niri-computer-use/demo/og.png' },
				},
				{ tag: 'meta', attrs: { property: 'og:image:width', content: '1280' } },
				{ tag: 'meta', attrs: { property: 'og:image:height', content: '640' } },
				{ tag: 'meta', attrs: { name: 'twitter:card', content: 'summary_large_image' } },
			],
			sidebar: groups.map(({ label, directory }) => ({ label, items: [{ autogenerate: { directory } }] })),
		}),
	],
});
