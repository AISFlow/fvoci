/**
 * Colours are written in one place: the tokens of styles/app.css. Everywhere
 * else a colour is a token (`bg-background`, `var(--option-blue)`), so that
 * the dark screen follows by itself and the contrast test covers what is
 * drawn.
 *
 * This reads the source of the app and fails on a colour written out: a hex
 * value, a colour function, a class of the framework's own palette
 * (`bg-red-500`, `text-white`), or a colour in an arbitrary value. The few
 * places that have to name a colour are listed with the reason, and the
 * values they name are held to the tokens further down.
 */
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { THEME_COLORS } from '../../src/lib/logic/display.ts';
import { parseOklch, type Color } from '../support/color.ts';

const WEB = path.resolve(import.meta.dirname, '../..');

/** Where a colour may be written out, and why. */
const ALLOWED: Record<string, string> = {
  'src/styles/app.css': 'the tokens are defined here',
  'src/lib/logic/display.ts':
    'theme-color needs a plain colour before and without the style sheet; checked against the tokens below',
  'index.html': 'the script that runs before the style sheet; checked against the tokens below',
};

const SKIPPED = ['src/lib/i18n/paraglide', 'src/styles/fonts'];
const EXTENSIONS = new Set(['.svelte', '.ts', '.js', '.css', '.html']);

function sourceFiles(directory: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(path.join(WEB, directory), { withFileTypes: true })) {
    const relative = path.join(directory, entry.name);
    if (SKIPPED.some((skipped) => relative.startsWith(skipped))) continue;
    if (entry.isDirectory()) files.push(...sourceFiles(relative));
    else if (EXTENSIONS.has(path.extname(entry.name))) files.push(relative);
  }
  return files.sort();
}

const PALETTE =
  'black|white|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone';
const UTILITIES =
  'bg|text|border|ring|ring-offset|fill|stroke|from|to|via|outline|decoration|shadow|accent|caret|divide|placeholder';

const RULES: [string, RegExp][] = [
  ['a hex colour', /(?<![\w&/-])#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![\w-])/g],
  ['a colour function', /\b(?:rgba?|hsla?|hwb|oklch|oklab|lch|lab)\(/g],
  [
    'a class of the palette',
    new RegExp(`(?<![\\w-])(?:${UTILITIES})-(?:${PALETTE})(?:-\\d{2,3})?(?![\\w-])`, 'g'),
  ],
  ['a colour in an arbitrary value', /\[(?:color:|#)[^\]]*\]/g],
];

/** The private names of a file: `#fade` is a member, not a colour. */
function privateNames(source: string): Set<string> {
  const names = new Set<string>();
  for (const match of source.matchAll(
    /(?:\.|^\s*(?:readonly\s+|static\s+|async\s+)*)(#[A-Za-z_]\w*)/gm,
  )) {
    names.add(match[1] ?? '');
  }
  return names;
}

/** The colours written out in a source text: `[line, what, text]`. */
export function rawColours(source: string): [number, string, string][] {
  const found: [number, string, string][] = [];
  const members = privateNames(source);
  source.split('\n').forEach((line, index) => {
    for (const [what, pattern] of RULES) {
      for (const match of line.matchAll(pattern)) {
        if (what === 'a hex colour' && members.has(match[0])) continue;
        found.push([index + 1, what, match[0]]);
      }
    }
  });
  return found;
}

describe('colours written out', () => {
  it('finds each kind, and leaves tokens and private members alone', () => {
    const kinds = (source: string): string[] => rawColours(source).map(([, what]) => what);
    expect(kinds('color: #fff;')).toEqual(['a hex colour']);
    expect(kinds('<div class="bg-[#0a0a0a]">')).toEqual([
      'a hex colour',
      'a colour in an arbitrary value',
    ]);
    expect(kinds('fill: rgb(0 0 0 / 50%)')).toEqual(['a colour function']);
    expect(kinds('--x: oklch(0.5 0.1 20)')).toEqual(['a colour function']);
    expect(kinds('class="text-white border-red-500"')).toEqual([
      'a class of the palette',
      'a class of the palette',
    ]);
    expect(kinds('class="bg-background text-option-blue border-input-border"')).toEqual([]);
    expect(kinds('style:background-color="var(--option-{colour})"')).toEqual([]);
    expect(
      kinds('class Thing {\n  #fade = 1;\n  go() { return this.#fade + this.#bed; }\n}'),
    ).toEqual([]);
    expect(kinds('href="#app" {#each items as item} id="#abc123x"')).toEqual([]);
    expect(kinds('const blue = 1; const white = `${blue}-gray`;')).toEqual([]);
  });

  it('has none in the app outside of the places that define them', () => {
    const offences: string[] = [];
    for (const file of [...sourceFiles('src'), 'index.html']) {
      if (file in ALLOWED) continue;
      for (const [line, what, text] of rawColours(readFileSync(path.join(WEB, file), 'utf8'))) {
        offences.push(`${file}:${line}: ${what}: ${text}`);
      }
    }
    expect(offences).toEqual([]);
  });

  it('lists only places that exist', () => {
    for (const file of Object.keys(ALLOWED)) {
      expect(() => readFileSync(path.join(WEB, file), 'utf8')).not.toThrow();
    }
  });
});

describe('the colours that have to be written out', () => {
  const css = readFileSync(path.join(WEB, 'src/styles/app.css'), 'utf8');
  const background = (selector: string): Color => {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const block = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(css)?.[1] ?? '';
    const value = /--background:\s*(oklch\([^)]*\))/.exec(block)?.[1];
    if (!value) throw new Error(`app.css has no --background for ${selector}`);
    return parseOklch(value);
  };
  /** A colour as `#rrggbb`: the channels are linear and are encoded for the screen first. */
  const hex = (colour: Color): string => {
    const channel = (linear: number): string => {
      const encoded = linear <= 0.0031308 ? 12.92 * linear : 1.055 * linear ** (1 / 2.4) - 0.055;
      return Math.round(Math.min(1, Math.max(0, encoded)) * 255)
        .toString(16)
        .padStart(2, '0');
    };
    return `#${channel(colour.r)}${channel(colour.g)}${channel(colour.b)}`;
  };

  it('are the page colours of the two themes', () => {
    expect(THEME_COLORS.light).toBe(hex(background(':root')));
    expect(THEME_COLORS.dark).toBe(hex(background('.dark')));
  });

  it('are the same in index.html, which cannot ask the style sheet yet', () => {
    const html = readFileSync(path.join(WEB, 'index.html'), 'utf8');
    const written = [...html.matchAll(/#[0-9a-f]{6}\b/g)].map((match) => match[0]).sort();
    expect([...new Set(written)]).toEqual([THEME_COLORS.dark, THEME_COLORS.light].sort());
    expect(html).toContain(`dark ? '${THEME_COLORS.dark}' : '${THEME_COLORS.light}'`);
    expect(html).toContain(`<meta name="theme-color" content="${THEME_COLORS.light}" />`);
  });
});
