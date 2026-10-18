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
	// Inline the stylesheets, so the first paint waits on no request after the page's own.
	build: { inlineStylesheets: 'always' },
	integrations: [
		starlight({
			title: 'niri-computer-use',
			// The 404 is `src/pages/404.astro`, as man answers a page it doesn't have.
			disable404Route: true,
			description:
				'An MCP server that lets AI agents see and act on a niri Wayland desktop, with a lease, a stop key and an audit log.',
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/ayagmar/niri-computer-use' },
			],
			customCss: [
				'@fontsource-variable/jetbrains-mono/wght.css',
				'./src/styles/tokens.css',
				'./src/styles/theme.css',
			],
			components: {
				Head: './src/components/Head.astro',
				PageTitle: './src/components/PageTitle.astro',
				Footer: './src/components/Footer.astro',
			},
			// Code on the plate, in the page's theme: no window frame, and lines wrapped to the
			// width.
			expressiveCode: {
				themes: ['github-dark', 'github-light'],
				defaultProps: { frame: 'none', wrap: true },
				styleOverrides: {
					borderWidth: '0px',
					borderRadius: '2px',
					codeBackground: 'var(--ncu-plate)',
					frames: { frameBoxShadowCssValue: 'none' },
				},
			},
			// The card link previews show, from the home page's recording.
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
