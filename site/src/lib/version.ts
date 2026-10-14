// The server's version, from the crate's `Cargo.toml`, so the page names the release the
// docs describe without anyone editing it.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

// The build runs in `site/`.
const manifest = readFileSync(resolve(process.cwd(), '../Cargo.toml'), 'utf8');
const found = manifest.match(/^\[package\][^[]*?^version = "([^"]+)"/m);
if (!found) throw new Error('no [package] version in ../Cargo.toml');

export const version = found[1];
export const releaseLink = `https://github.com/ayagmar/niri-computer-use/releases/tag/v${version}`;
