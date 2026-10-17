// The home page's words, in one place: `pages/index.astro` sets them as a man page and
// `pages/index.md.ts` serves them as Markdown for agents. Links are relative to the site's
// root.

export const home = {
	title: 'niri-computer-use(1): let your AI agent use your niri desktop',
	description:
		'An MCP server that lets AI agents see and act on a niri Wayland desktop, with a lease, a stop key and an audit log.',
	name: 'let an MCP agent see and drive your niri desktop',
	commands: ['serve', 'status', 'stop', 'resume', 'recover', 'watch'],
	about: [
		'An MCP server for Claude Code, Codex, Pi or any stdio client. The agent reads windows, workspaces and screenshots, and finds buttons by name. While it holds the lease, it clicks, types, drags and runs niri actions, and each action returns accepted, observed and changes.',
		'niri 26.04 on Linux only.',
	],
	install: {
		get: 'curl -fsSLO https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh',
		read: 'less install.sh',
		run: 'sh install.sh',
		cargo: 'cargo install --locked niri-computer-use',
		stop: 'Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true {\n    spawn "/home/you/.local/bin/niri-computer-use" "stop";\n}',
	},
	safety: [
		['lease', 'One agent at a time acts.'],
		['stop key', 'Cancels the action and takes the lease back.'],
		['lock', 'Nothing acts while the screen is locked.'],
		['focus', 'Typing checks focus, unless told not to.'],
		['deny list', 'No input while a denied app has focus.'],
		['audit log', 'Every call, without typed text.'],
	],
	sandbox: 'Not a sandbox: agents see your screen, and with the lease they can do anything you can.',
	example: {
		clip: 'errand',
		alt: 'Play the clip: Files with two screenshots selected and its right-click menu open.',
		caption:
			'Claude Code (Opus 5.5) puts two screenshots into a new folder in Files. Six of its eleven calls, with round trips in ms. The clip cuts thinking time.',
		calls: [
			['status', '3'],
			['acquire_desktop', '2'],
			['focus {"window_id": 11, …}', '858'],
			['click {… "button": "right"}', '957'],
			['type_text {"text": "release-0.1", "submit": true, …}', '920'],
			['release_desktop {"restore_focus": true}', '3'],
		],
	},
	bugs: 'With a niri release other than 26.04 it only looks. The pointer tools need one monitor at transform Normal.',
	seeAlso: ['start/install', 'start/clients', 'concepts/safety', 'concepts/configuration', 'tools/overview', 'reference/cli'],
};
