/**
 * The licence of the fonts of the formula library (KaTeX).
 *
 * The package says "MIT", which is the licence of its code. Its fonts are
 * under the SIL Open Font License 1.1, and the package says so only inside
 * the font files themselves. So this reads it from there: every font the
 * app's style sheet names (src/lib/editor/katex.css) has to state that
 * licence in its name table, or the build fails. The notice is the
 * copyright and reserved names as the fonts give them, followed by the text
 * of the licence (OFL-1.1.txt in this folder, from the SPDX licence list).
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { readName } from '../fonts/sfnt.ts';

import type { FontLicense } from './notices.ts';
import { packageDirectory } from './packages.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');
const STYLE_SHEET = 'src/lib/editor/katex.css';
const LICENSE_TEXT = path.join(import.meta.dirname, 'OFL-1.1.txt');
const STATEMENT = 'This Font Software is licensed under the SIL Open Font License, Version 1.1.';

/** Name identifiers of an OpenType font: copyright notice, licence description. */
const COPYRIGHT = 0;
const LICENSE = 13;

/** The fonts the style sheet names, without extension. */
export function katexFontNames(css: string): string[] {
  return [...css.matchAll(/katex\/dist\/fonts\/([\w-]+)\.woff2/g)].map((match) => match[1] ?? '');
}

/** The notice for the fonts, checked against every one of them. */
export function katexFontLicense(): FontLicense {
  const directory = packageDirectory('katex');
  const { version } = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8')) as {
    version: string;
  };
  const names = katexFontNames(readFileSync(path.join(WEB_ROOT, STYLE_SHEET), 'utf8'));
  if (names.length === 0) throw new Error(`${STYLE_SHEET} names no fonts`);

  const copyrights = new Set<string>();
  const reserved: string[] = [];
  for (const name of names) {
    // The TrueType file of the same font: the WOFF2 file is that font, compressed.
    const font = readFileSync(path.join(directory, 'dist/fonts', `${name}.ttf`));
    const license = readName(font, LICENSE) ?? '';
    if (!license.includes(STATEMENT)) {
      throw new Error(`katex ${version}: the font ${name} does not state the OFL-1.1 licence`);
    }
    copyrights.add((readName(font, COPYRIGHT) ?? '').trim());
    const family = /with Reserved Font Name ([\w-]+)/.exec(license)?.[1];
    if (family && !reserved.includes(family)) reserved.push(family);
  }
  const header = [
    ...copyrights,
    `with Reserved Font Names ${reserved.join(', ')}.`,
    '',
    STATEMENT,
    'This license is copied below, and is also available with a FAQ at: scripts.sil.org/OFL',
    '',
    '-----------------------------------------------------------',
    '',
  ].join('\n');
  return {
    family: `KaTeX fonts (${names.length} files)`,
    version,
    identifier: 'OFL-1.1',
    text: header + readFileSync(LICENSE_TEXT, 'utf8').trim(),
  };
}
