// The version in the crate's `Cargo.toml`, read when the site is built, and the GitHub release
// of that version. The site is built from main, which can be ahead of the latest release.
import manifest from '../../../Cargo.toml?raw';

const found = manifest.match(/^\[package\][^[]*?^version = "([^"]+)"/m);
if (!found) throw new Error('no [package] version in Cargo.toml');

export const version = found[1];
export const releaseLink = `https://github.com/ayagmar/niri-computer-use/releases/tag/v${version}`;
