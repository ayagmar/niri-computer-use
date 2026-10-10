// Checks the built site in dist/: every internal link and image in the HTML pages, and
// every link to this site in the Markdown and llms files, must reach a built file, and a
// #fragment must name an element on that page. Also checks that llms.txt links to every
// page's Markdown and that llms-full.txt contains each of them. Exits 1 on any failure.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const DIST = new URL('../dist/', import.meta.url).pathname;
const BASE = '/niri-computer-use/';
const SITE = `https://ayagmar.github.io${BASE}`;

const files = readdirSync(DIST, { recursive: true })
	.map((file) => join(DIST, file))
	.filter((file) => statSync(file).isFile());
const html = files.filter((file) => file.endsWith('.html'));
const text = files.filter((file) => /\.(md|txt)$/.test(file));
const failures = [];

const isRedirect = (source) => source.includes('http-equiv="refresh"');

// The built file a site path names, or null.
function target(path) {
	if (!path.startsWith(BASE)) return null;
	const rest = decodeURIComponent(path.slice(BASE.length));
	const candidates = rest === '' || rest.endsWith('/') ? [join(rest, 'index.html')] : [rest, join(rest, 'index.html')];
	return candidates.map((file) => join(DIST, file)).find((file) => existsSync(file) && statSync(file).isFile()) ?? null;
}

const ids = new Map();
function hasId(file, id) {
	if (!ids.has(file)) {
		const found = new Set([...readFileSync(file, 'utf8').matchAll(/\sid="([^"]+)"/g)].map((m) => m[1]));
		ids.set(file, found);
	}
	return ids.get(file).has(id);
}

function check(from, link, base) {
	if (/^(mailto:|data:|javascript:)/.test(link)) return;
	const url = new URL(link, base);
	if (url.origin !== new URL(SITE).origin) return;
	const file = target(url.pathname);
	if (!file) {
		failures.push(`${from}: ${link} reaches no built file`);
		return;
	}
	const fragment = decodeURIComponent(url.hash.slice(1));
	if (fragment && file.endsWith('.html') && !hasId(file, fragment)) {
		failures.push(`${from}: ${link} names no #${fragment} on ${relative(DIST, file)}`);
	}
}

for (const file of html) {
	// A canonical link names the page itself, which for 404.html is no built file.
	const source = readFileSync(file, 'utf8').replace(/<link rel="canonical"[^>]*>/g, '');
	const page = `${SITE}${relative(DIST, file).replace(/index\.html$/, '')}`;
	for (const [, link] of source.matchAll(/\s(?:href|src)="([^"]+)"/g)) {
		check(relative(DIST, file), link.replaceAll('&amp;', '&'), page);
	}
}

for (const file of text) {
	const source = readFileSync(file, 'utf8');
	for (const [link] of source.matchAll(/https:\/\/ayagmar\.github\.io\/niri-computer-use\/[^\s)>\]]*/g)) {
		check(relative(DIST, file), link, SITE);
	}
}

// Every content page, as its Markdown address.
const pages = html
	.filter((file) => file.endsWith('index.html') && !isRedirect(readFileSync(file, 'utf8')))
	.map((file) => relative(DIST, file).replace(/\/?index\.html$/, ''))
	.map((path) => `${path || 'index'}.md`);
const llms = readFileSync(join(DIST, 'llms.txt'), 'utf8');
const full = readFileSync(join(DIST, 'llms-full.txt'), 'utf8');
for (const page of pages) {
	if (!existsSync(join(DIST, page))) {
		failures.push(`${page}: no Markdown version`);
		continue;
	}
	if (!llms.includes(`(${SITE}${page})`)) failures.push(`llms.txt doesn't list ${page}`);
	if (!full.includes(readFileSync(join(DIST, page), 'utf8'))) failures.push(`llms-full.txt doesn't contain ${page}`);
}

if (failures.length > 0) {
	console.error(failures.join('\n'));
	console.error(`check-links: ${failures.length} failure(s)`);
	process.exit(1);
}
console.log(`check-links: ${html.length} HTML and ${text.length} text files, ${pages.length} pages in llms.txt: ok`);
