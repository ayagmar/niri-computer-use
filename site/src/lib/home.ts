// The home page's words, in one place: `pages/index.astro` sets them and `pages/index.md.ts`
// serves them as Markdown for agents. Links are relative to the site's root.

export const home = {
	title: 'niri-computer-use: let your AI agent use your niri desktop',
	description:
		'An MCP server that lets AI agents see and act on a niri Wayland desktop, with a lease, a stop key and an audit log.',
	headline: ['Your agent', 'drives niri.'],
	lede: 'An MCP server that lets Claude Code, Codex or any MCP agent see and use your niri desktop.',
	note: 'A real Claude Code session on niri 26.04. Thinking time is cut to 1.2 s; calls play in real time.',
	stop: {
		headline: ['One key', 'takes it back.'],
		lede: 'Mod+Shift+Escape ends the action and takes the lease back.',
		facts: ['Nothing acts without the lease.', 'Your agent can’t press the stop key.', 'Every action reports what niri saw.'],
		more: { text: 'Guardrails, not a sandbox', link: 'concepts/safety/' },
	},
	clips: {
		headline: 'More real sessions',
		items: [
			{ name: 'layout', text: 'Resizes and floats windows with niri’s own actions.' },
			{ name: 'shell', text: 'Opens Noctalia’s control center and clicks inside it.' },
			{ name: 'takeover', text: 'Stops typing when you click into another window.' },
		],
	},
	install: {
		headline: 'Install',
		lede: 'Needs niri 26.04 on Linux and Rust 1.99.',
		steps: [
			{
				text: 'Build it.',
				lang: 'sh',
				code: 'git clone https://github.com/ayagmar/niri-computer-use.git\ncd niri-computer-use\ncargo install --locked --path .',
			},
			{
				text: 'Add it to Claude Code.',
				lang: 'sh',
				code: 'claude mcp add --scope user niri-computer-use -- ~/.cargo/bin/niri-computer-use serve',
				more: { text: 'Codex, Pi and other clients', link: 'start/clients/' },
			},
			{
				text: 'Bind the stop key in niri’s config.kdl.',
				lang: 'kdl',
				code: 'Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true { spawn "/home/you/.cargo/bin/niri-computer-use" "stop"; }',
			},
		],
		more: { text: 'Full install guide', link: 'start/install/' },
	},
	agents: { text: 'For agents: llms.txt', link: 'llms.txt' },
};
