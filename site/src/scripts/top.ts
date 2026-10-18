// The running head's links on the home page: the repository's star count from GitHub, and
// a switch between light and dark that the docs share, through Starlight's stored theme.
// Without the script the GitHub link stays and the page follows the system.

const root = document.documentElement;
const key = 'starlight-theme';
const system = matchMedia('(prefers-color-scheme: light)');
const toggle = document.querySelector<HTMLButtonElement>('[data-theme-toggle]');

const current = () => root.dataset.theme ?? (system.matches ? 'light' : 'dark');
function label() {
	if (!toggle) return;
	const other = current() == 'light' ? 'dark' : 'light';
	toggle.textContent = other;
	toggle.setAttribute('aria-label', `Switch to the ${other} theme`);
}
if (toggle) {
	toggle.hidden = false;
	toggle.onclick = () => {
		const next = current() == 'light' ? 'dark' : 'light';
		root.dataset.theme = next;
		try {
			localStorage.setItem(key, next);
		} catch {
			// The choice holds for this page only.
		}
		label();
	};
	system.addEventListener('change', label);
	label();
}

// The count is GitHub's own; when GitHub doesn't answer in time, the link shows none.
const stars = document.querySelector('[data-stars]');
if (stars)
	fetch('https://api.github.com/repos/ayagmar/niri-computer-use', { signal: AbortSignal.timeout(4000) })
		.then((r) => (r.ok ? r.json() : null))
		.then((repo: { stargazers_count?: number } | null) => {
			const n = repo?.stargazers_count;
			if (typeof n == 'number') stars.textContent = ` · ${n} ${n == 1 ? 'star' : 'stars'}`;
		})
		.catch(() => {});
