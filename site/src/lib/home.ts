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
		script: 'https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh',
		piped: 'curl -fsSL https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh | sh',
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
	// Every call in the clip: when it went out, in seconds into the clip, and its round trip
	// in ms. From the recording's timeline, which `scripts/demo-timeline.py` wrote.
	example: {
		clip: 'errand',
		caption:
			'Claude Code (Opus 5.5) puts two screenshots into a new folder in Files. Its eleven calls, with when each went out and its round trip, from the recording, which cuts thinking time.',
		calls: [
			[1.2, 'status', 3],
			[1.2, 'desktop_state', 3],
			[2.4, 'acquire_desktop', 2],
			[3.56, 'focus {"window_id": 11, …}', 858],
			[5.62, 'click {"x": 979, "y": 88, …}', 252],
			[7.07, 'key {"keys": ["shift+Right", "ctrl+alt+n"], …}', 469],
			[8.74, 'click {"x": 1061, "y": 88, "button": "right", …}', 957],
			[10.89, 'click {"x": 1130, "y": 100, …}', 856],
			[12.95, 'key {"keys": ["ctrl+a"], …}', 253],
			[14.4, 'type_text {"text": "release-0.1", "submit": true, …}', 920],
			[16.49, 'release_desktop {"restore_focus": true}', 3],
		] as [number, string, number][],
	},
	bugs: 'With a niri release other than 26.04 it only looks. The pointer tools need one monitor at transform Normal.',
	seeAlso: ['start/install', 'start/clients', 'concepts/safety', 'concepts/configuration', 'tools/overview', 'reference/cli'],
};
