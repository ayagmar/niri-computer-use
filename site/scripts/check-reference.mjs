// Checks the error and configuration pages against the Rust source, so they can't drift:
// every `ErrorName` variant in src/error.rs has a `### \`name\`` heading on the error
// page and no heading names another, and every key of `Policy` and `Preset` in
// src/policy.rs is documented on the configuration page and no other key is. Exits 1 on
// any difference.
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');
const failures = [];

// The body of `<kind> <name> { … }`, up to the first line that is just `}`.
function block(source, kind, name) {
	const start = source.indexOf(`pub(crate) ${kind} ${name} {`);
	if (start === -1) throw new Error(`no ${kind} ${name} in the source`);
	return source.slice(start, source.indexOf('\n}', start));
}

function compare(what, source, page) {
	for (const name of source) if (!page.has(name)) failures.push(`${what}: ${name} is in the source but not documented`);
	for (const name of page) if (!source.has(name)) failures.push(`${what}: ${name} is documented but not in the source`);
}

const snake = (name) => name.replace(/[A-Z]/g, (c, i) => (i ? '_' : '') + c.toLowerCase());

// Errors: `ErrorName` serializes its variants in snake case.
const errors = block(read('../../src/error.rs'), 'enum', 'ErrorName');
const variants = new Set([...errors.matchAll(/^ {4}([A-Z][A-Za-z]+),$/gm)].map((m) => snake(m[1])));
const errorPage = read('../src/content/docs/reference/errors.md');
const headings = new Set([...errorPage.matchAll(/^### `([a-z_]+)`$/gm)].map((m) => m[1]));
if (variants.size === 0) failures.push('errors: found no ErrorName variants in src/error.rs');
compare('errors', variants, headings);

// Policy keys: serde's field names, after any `rename`.
function fields(body) {
	const keys = new Set();
	let rename = null;
	for (const line of body.split('\n')) {
		const renamed = line.match(/#\[serde\(.*rename = "([^"]+)"/);
		if (renamed) rename = renamed[1];
		const field = line.match(/^ {4}pub\(crate\) ([a-z_]+):/);
		if (field) {
			keys.add(rename ?? field[1]);
			rename = null;
		}
	}
	return keys;
}
const policy = read('../../src/policy.rs');
const config = read('../src/content/docs/concepts/configuration.md');
const keysSection = config.slice(config.indexOf('\n## Keys'), config.indexOf('\n## ', config.indexOf('\n## Keys') + 1));
const topKeys = new Set(
	[...keysSection.matchAll(/^### `(?:\[\[)?([a-z_]+)(?:\]\])?`$/gm)].map((m) => m[1]),
);
compare('policy keys', fields(block(policy, 'struct', 'Policy')), topKeys);
const presetSection = keysSection.slice(keysSection.indexOf('### `[[preset]]`'));
const presetTable = presetSection.slice(0, presetSection.indexOf('\n\n', presetSection.indexOf('| Key |')));
const presetKeys = new Set([...presetTable.matchAll(/^\| `([a-z_]+)` \|/gm)].map((m) => m[1]));
compare('preset keys', fields(block(policy, 'struct', 'Preset')), presetKeys);

if (failures.length > 0) {
	console.error(failures.join('\n'));
	console.error(`check-reference: ${failures.length} failure(s)`);
	process.exit(1);
}
console.log(`check-reference: ${variants.size} errors, ${topKeys.size} policy keys, ${presetKeys.size} preset keys: ok`);
