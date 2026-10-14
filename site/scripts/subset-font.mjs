// Cuts Archivo down to the characters the site's text uses and the widths and weights it sets,
// so the face the headline waits for is a fraction of Fontsource's. It is declared after
// Fontsource's faces under the same name, so any other character falls back to those, which a
// page downloads only when it has such a character.
//
// Writes `src/generated/archivo-text.woff2` and the `@font-face` for it,
// `src/generated/archivo-text.css`.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import subsetFont from 'subset-font';

const source = new URL(
	'../node_modules/@fontsource-variable/archivo/files/archivo-latin-wdth-normal.woff2',
	import.meta.url,
);
const out = new URL('../src/generated/', import.meta.url);

/** Printable ASCII, the no-break space, and the typographer's punctuation the copy uses. */
const ranges = [
	[0x20, 0x7e],
	[0xa0],
	[0xa9],
	[0xb7],
	[0xd7],
	[0x2013, 0x2014],
	[0x2018, 0x2019],
	[0x201c, 0x201d],
	[0x2026],
];

/** The site sets Archivo from 62% to 100% wide; the wider half of the axis is a third of the
 * file. */
const axes = { wdth: { min: 62, max: 100 } };

const hex = (code) => code.toString(16).toUpperCase();
const text = ranges
	.flatMap(([first, last = first]) => Array.from({ length: last - first + 1 }, (_, i) => first + i))
	.map((code) => String.fromCodePoint(code))
	.join('');
const unicodeRange = ranges
	.map(([first, last]) => (last === undefined ? `U+${hex(first)}` : `U+${hex(first)}-${hex(last)}`))
	.join(',');

const font = await subsetFont(await readFile(source), text, { targetFormat: 'woff2', variationAxes: axes });
// The weight and width ranges are Fontsource's, though wider than 100% clamps to 100%: Chrome
// only falls back between faces whose ranges match, and otherwise downloads both.
const css = `@font-face {
	font-family: 'Archivo Variable';
	font-style: normal;
	font-display: swap;
	font-weight: 100 900;
	font-stretch: 62% 125%;
	src: url(./archivo-text.woff2) format('woff2-variations');
	unicode-range: ${unicodeRange};
}
`;

await mkdir(out, { recursive: true });
await writeFile(new URL('archivo-text.woff2', out), font);
await writeFile(new URL('archivo-text.css', out), css);
console.log(`subset-font: Archivo ${Math.round(font.length / 1024)} KB, ${text.length} characters`);
