# Security

niri-computer-use gives an AI agent your desktop. Its guardrails catch mistakes and keep you in control: the lease, the stop key, the lock gate, the policy file and the audit log. They are not a sandbox. An agent that holds the lease can do anything you can do with a mouse and keyboard, and any connected agent can see your screen, window titles and clipboard. [Safety](https://ayagmar.github.io/niri-computer-use/concepts/safety/) describes what is and isn't enforced.

## Reporting a problem

Report a vulnerability privately through GitHub: on the repository's **Security** tab, choose **Report a vulnerability**. Don't open a public issue for it. Include the version or commit, your niri version, and the steps that show the problem.

## In scope

- A way to act without the lease, or while another server holds it.
- Input that goes out after the stop key, while the screen is locked or its lock state is unknown, or while the input-dirty marker is set.
- A preset rule, the panel allowlist or `capture_dir`'s path rules letting through what they are documented to refuse.
- `paste` replacing the clipboard without saving it whole first.
- Typed or pasted text, clipboard contents, window titles or image data written to the audit log.
- Input left held after the server ends, despite the [crash guardian](https://ayagmar.github.io/niri-computer-use/concepts/safety/#the-crash-guardian).
- The server reaching a different niri or Wayland display than the one in `NIRI_SOCKET`.

## Out of scope

- What an agent does with the access you gave it. A lease holder can click and type anything you could.
- Anything the [Safety](https://ayagmar.github.io/niri-computer-use/concepts/safety/#what-isnt-a-boundary) page lists as not a boundary: what reading tools show without a lease, apps choosing their own `app_id`, a click landing on a window that isn't focused, a wrapper script getting past the preset rules, and other programs running as your user.
- Bugs in niri, Noctalia, `wtype`, `grim`, wl-clipboard or the apps being driven, unless niri-computer-use makes them reachable in a way it shouldn't.
