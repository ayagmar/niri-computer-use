// The home page's words, in one place: `pages/index.astro` sets them and `pages/index.md.ts`
// serves them as Markdown for agents. Links are relative to the site's root.

export const home = {
	title: 'niri-computer-use: let your AI agent use your niri desktop',
	description:
		'An MCP server that lets AI agents see and act on a niri Wayland desktop, with a lease, a stop key and an audit log.',
	tagline: 'Lets Claude Code, Codex, Pi or any MCP agent see and drive your niri desktop.',
	stop: 'You hold a stop key it can’t press.',
	only: 'niri 26.04 on Linux only.',
	clip: {
		name: 'errand',
		alt: 'Play the clip: Files with two screenshots selected and its right-click menu open, New Folder with Selection at the top.',
		caption: 'Claude Code, halfway through filing two screenshots in a new folder.',
		note: 'A real session, thinking time cut.',
	},
	glance: [
		['Sees', 'windows, workspaces, screenshots, buttons by name'],
		['Does', 'focus, launch, click, type, drag, scroll, niri actions, Noctalia panels'],
		['Returns', 'accepted, observed and changes, for each action'],
		['Needs', 'glibc 2.39+, grim, loginctl; one monitor for the pointer'],
	],
	install: [
		{
			label: 'Get it',
			lang: 'sh',
			code: 'curl -fsSLO https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh',
			read: 'less install.sh',
			run: 'sh install.sh',
			orNote: 'or build it, and use ~/.cargo/bin in the steps below:',
			or: 'cargo install --locked niri-computer-use',
		},
		{
			label: 'Register it',
			note: 'The script offers to.',
			more: { text: 'Codex, Pi, others', link: 'start/clients/' },
			lang: 'sh',
			code: 'claude mcp add --scope user niri-computer-use -- ~/.local/bin/niri-computer-use serve',
		},
		{
			label: 'Bind the stop key',
			note: 'In niri’s config.kdl.',
			lang: 'kdl',
			code: 'Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true { spawn "/home/you/.local/bin/niri-computer-use" "stop"; }',
		},
	],
	safety: {
		enforced: [
			'One lease holder acts',
			'Stop key takes the lease back',
			'Locked screen: no actions',
			'Typing checks focus, unless told not to',
			'No input while a denied app has focus',
			'Audit log, no typed text',
		],
		missed: [
			'Agents see your screen',
			'Clicks can hit a denied window',
			'No password-field check on typing',
			'While it holds the lease, it can do anything you can',
		],
		more: { text: 'Read Safety first', link: 'concepts/safety/' },
	},
};
