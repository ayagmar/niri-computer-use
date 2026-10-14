// The home page's words, in one place: `pages/index.astro` sets them and `pages/index.md.ts`
// serves them as Markdown for agents. Links are relative to the site's root.

export const home = {
	title: 'niri-computer-use: let your AI agent use your niri desktop',
	description:
		'An MCP server that lets AI agents see and act on a niri Wayland desktop, with a lease, a stop key and an audit log.',
	running: 'MCP server for niri 26.04 on Linux',
	headline: 'Your agent can use your niri desktop.',
	stopline: 'One key takes it back.',
	lede: 'An MCP server for Claude Code, Codex, Pi and other MCP clients. The agent reads your windows and screenshots; while it holds the lease, it clicks, types and runs niri actions.',
	safety: { text: 'How the stop key and the lease work', link: 'concepts/safety/' },
	sessions: {
		caption: 'Five real sessions: each recording, and under it the calls it made.',
		try: 'Press it, or Esc on your keyboard, while the agent works: the diagram shows what the server would answer from then on.',
		note: 'Recorded on a real niri 26.04 desktop with Claude Code (Opus 5.5) and the installed server; nothing was scripted. Thinking time between calls is cut to 1.2 s; the calls run in real time.',
		items: [
			{
				name: 'stop',
				tab: 'Stop key',
				text: 'You press the stop key between two calls. The lease goes back to you, and the agent’s next click comes back stopped.',
			},
			{
				name: 'takeover',
				tab: 'Focus moved',
				text: 'The agent types a paragraph into nvim, then you move focus to Files. Its next key was meant for nvim, so it is refused.',
			},
			{
				name: 'errand',
				tab: 'Errand',
				text: 'The agent selects two screenshots in Files, makes a folder for them from the right-click menu, names it release-0.1 and gives focus back.',
			},
			{
				name: 'layout',
				tab: 'Layout',
				text: 'The agent sizes Files and the browser to a third and two thirds, then floats its own terminal small in the corner, all with niri’s own actions.',
			},
			{
				name: 'shell',
				tab: 'Noctalia',
				text: 'The agent opens Noctalia’s control center, turns on Do Not Disturb and closes the panel.',
			},
		],
	},
	result: {
		headline: 'What an action answers',
		lede: 'Each action says whether niri, Noctalia or the app took the request, what happened afterwards, and what niri saw change on the desktop.',
		source:
			'What launch {"preset": "notes"} answered in the README’s example run, in a headless nested niri.',
		code: `{
  "accepted": true,
  "observed": "one",
  "windows": [4],
  "focused_window": 4,
  "changes": {
    "focus": {"from": 3, "to": 4},
    "opened": [{"app_id": "notes", "id": 4,
                "title": "harness window", "workspace_id": 1}]
  }
}`,
		fields: [
			{
				name: 'accepted',
				lines: [2, 2],
				text: 'Whether niri, Noctalia or, for an element action, the app took the request. False when nothing was sent, null when the request went out but the reply was lost.',
			},
			{
				name: 'observed',
				lines: [3, 3],
				text: 'What happened afterwards, such as focused, opened or closed in niri’s event stream; for input, sent once niri has handled it; for set_element_text, matched or differs by the app’s own count.',
			},
			{
				name: 'changes',
				lines: [6, 10],
				text: 'What niri saw change: windows opened and closed, focus and workspace moving, a title, size or floating state. Left out when niri saw nothing.',
			},
			{
				name: 'error',
				text: 'When an action is refused or fails, in place of the above: a stable name such as focus_mismatch, and a detail that keeps niri’s, Noctalia’s or the program’s own message. An app’s own messages from element actions are left out, since they can hold a field’s text. Wrong arguments get a plain-text error instead.',
			},
		],
		more: { text: 'Every tool and its result', link: 'tools/overview/' },
	},
	gates: {
		headline: 'Before anything reaches niri',
		lede: 'Every action passes the first three checks, in this order; the last two belong to the tools. The first check that fails answers the agent, and nothing goes out.',
		pass: {
			text: 'All checks pass',
			answer: 'accepted: true · observed: sent, for a type_text call',
		},
		items: [
			{
				gate: 'Stop flag',
				when: 'You pressed the stop key.',
				covers: 'every action',
				error: 'stopped',
				detail: 'the stop flag is set; the user clears it with `niri-computer-use resume`',
			},
			{
				gate: 'Lease',
				when: 'This agent didn’t take the lease, or another one holds it.',
				covers: 'every action',
				error: 'lease_required',
				detail: "this session doesn't hold the lease; call acquire_desktop first",
			},
			{
				gate: 'Lock state',
				when: 'The screen is locked, or neither logind nor Noctalia says whether it is.',
				covers: 'every action',
				error: 'screen_locked',
				detail: 'the screen is locked',
			},
			{
				gate: 'Focus',
				when: 'The window the agent named in expect, or an element action’s window, doesn’t have focus. Typing checks again before each key.',
				covers: 'typing, keys and pastes, checked first; element actions, checked after the deny list',
				error: 'focus_mismatch',
				detail: 'expected window_id 19; window 11 with app_id Some("org.gnome.Nautilus")',
			},
			{
				gate: 'Deny list',
				when: 'An app on your deny list has focus, or owns the window an element action or a close aims at.',
				covers: 'pointer, keyboard and element actions, and closing a window',
				error: 'app_denied',
				detail: `the focused window's app_id "org.keepassxc.KeePassXC" is on the policy's deny list`,
			},
		],
		note: 'The details are the server’s own messages; the app_id and window ids are examples. Two more checks sit between these, for stuck input and for niri’s version; Safety lists them all.',
	},
	limits: {
		headline: 'Guardrails, not a sandbox',
		enforces: {
			title: 'What it enforces',
			items: [
				'One agent at a time holds the lease, and only that agent acts. Looking needs no lease, except reading the clipboard.',
				'The stop key cancels the running action and takes the lease back. An agent can’t press it.',
				'With confirm_lease on, you allow each lease from a desktop notification. It is off by default.',
				'Element actions refuse fields the toolkit marks as password fields.',
				'The launch tool starts only presets you write, and niri actions that run programs need unrestricted on.',
				'An audit log of every call that reaches a tool, without typed text, the clipboard, window titles or images. A failed write shows in status.',
			],
		},
		misses: {
			title: 'What it doesn’t stop',
			items: [
				'Any connected agent sees your screen and window titles.',
				'A click can land on a denied window that doesn’t have focus.',
				'Typing and clicking don’t check for password fields.',
				'A lock that comes after the last read isn’t caught.',
				'An agent that can run shell commands can answer the confirm_lease notification itself.',
				'With the lease, a click or a key can do anything you could do.',
			],
		},
		more: { text: 'Read Safety before letting an agent act', link: 'concepts/safety/' },
	},
	requirements: {
		headline: 'What it needs',
		rows: [
			['niri', '26.04. A 26.04 patch release works with a warning; any other release is read-only.'],
			['System', 'Linux on x86_64 or aarch64. Release builds need glibc 2.39 or newer; cargo needs Rust 1.99.'],
			['At run time', 'grim, loginctl and libxkbcommon; notify-send if you turn on confirm_lease.'],
			['Agents', 'Claude Code, Codex, Pi or any MCP client that starts a server over stdio.'],
			['Pointer tools', 'One monitor at transform Normal. The keyboard, screenshots and the rest work with any.'],
			['Elements', 'GTK 4 works. GTK 3 and Qt apps that draw their own title bar give no element boxes.'],
			['Noctalia', 'Optional. A session without Noctalia or an accessibility bus has 17 of the 22 tools.'],
		],
	},
	install: {
		headline: 'Install',
		lede: 'Three steps, from a shell inside your niri session.',
		steps: [
			{
				text: 'Download the install script, read it, run it. It installs the binary and the skill that tells your agents how to use the server, then offers to register the server in each of Claude Code, Pi and Codex it finds.',
				lang: 'sh',
				code: 'curl -fsSLO https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh\nless install.sh\nsh install.sh',
				more: { text: 'What the script does', link: 'start/install-script/' },
				or: {
					text: 'Or build the crate with cargo; then add the skill.',
					lang: 'sh',
					code: 'cargo install --locked niri-computer-use',
					more: { text: 'The skill', link: 'start/install/#the-skill' },
				},
			},
			{
				text: 'If you said no, or built with cargo, register it yourself, here in Claude Code with the binary’s path (~/.cargo/bin after cargo).',
				lang: 'sh',
				code: 'claude mcp add --scope user niri-computer-use -- ~/.local/bin/niri-computer-use serve',
				more: { text: 'Codex, Pi and other clients', link: 'start/clients/' },
			},
			{
				text: 'Bind the stop key in niri’s config.kdl.',
				lang: 'kdl',
				code: 'Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true { spawn "/home/you/.local/bin/niri-computer-use" "stop"; }',
			},
		],
		more: { text: 'Full install guide', link: 'start/install/' },
	},
	agents: { text: 'For agents: llms.txt', link: 'llms.txt' },
};
