/**
 * The colour tokens of styles/app.css, read from the file itself, against the
 * contrast they have to keep on both themes:
 *
 * - text on its surface: 4.5 to 1
 * - the outline of a form field and the focus indicator, on every surface
 *   they can appear on: 3 to 1
 * - the line of somebody else's caret in a body (editor/carets.css), in each
 *   colour a person can have, on the surfaces a body is drawn on: 3 to 1
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { AVATAR_COLORS } from '$lib/components/avatar/avatar.ts';
import { TOKEN_KINDS } from '$lib/editor/code-tokens.ts';

import { contrast, over, parseOklch, type Color } from '../support/color.ts';

const css = readFileSync(path.resolve(import.meta.dirname, '../../src/styles/app.css'), 'utf8');

/** The colour declarations of the first rule with this selector. */
function tokensOf(selector: string): Map<string, Color> {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const block = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(css)?.[1];
  if (block === undefined) throw new Error(`app.css has no rule for ${selector}`);
  const tokens = new Map<string, Color>();
  for (const match of block.matchAll(/--([\w-]+):\s*(oklch\([^)]*\))\s*;/g)) {
    tokens.set(match[1] ?? '', parseOklch(match[2] ?? ''));
  }
  return tokens;
}

const caretCss = readFileSync(
  path.resolve(import.meta.dirname, '../../src/lib/editor/carets.css'),
  'utf8',
);
/** The share of the person's colour in the line of a caret; the rest is the text colour. */
const caretShare =
  Number(
    /--caret-line:\s*color-mix\(\s*in srgb,\s*var\(--caret[^%]*\)\s+(\d+)%,\s*var\(--foreground\)\s*\)/.exec(
      caretCss,
    )?.[1],
  ) / 100;

const themes = {
  light: tokensOf(':root'),
  dark: tokensOf('.dark'),
};

/** [text, surface]. The text is drawn on the surface, the surface on the page. */
const TEXT_PAIRS: [string, string][] = [
  ['foreground', 'background'],
  ['card-foreground', 'card'],
  ['popover-foreground', 'popover'],
  ['primary-foreground', 'primary'],
  ['secondary-foreground', 'secondary'],
  ['accent-foreground', 'accent'],
  ['muted-foreground', 'background'],
  ['muted-foreground', 'card'],
  ['muted-foreground', 'muted'],
  ['destructive', 'background'],
  ['destructive', 'card'],
  ['sidebar-foreground', 'sidebar'],
  ['sidebar-primary-foreground', 'sidebar-primary'],
  ['sidebar-accent-foreground', 'sidebar-accent'],
  ['muted-foreground', 'sidebar'],
  // The label of a select option: its text is the page's text colour.
  ['foreground', 'option-default'],
  ['foreground', 'option-gray'],
  ['foreground', 'option-brown'],
  ['foreground', 'option-orange'],
  ['foreground', 'option-yellow'],
  ['foreground', 'option-green'],
  ['foreground', 'option-blue'],
  ['foreground', 'option-purple'],
  ['foreground', 'option-pink'],
  ['foreground', 'option-red'],
  // The colours of code, on the surface of a code block (editor/code-block.css).
  ...TOKEN_KINDS.map((kind): [string, string] => [`code-${kind}`, 'muted']),
  // What a comparison of two bodies marks (editor/diff.css): text and code on it.
  ['foreground', 'diff-added'],
  ['foreground', 'diff-removed'],
  // A link on it (diff.css); a mention has its own surface.
  ['diff-link', 'diff-added'],
  ['diff-link', 'diff-removed'],
  ['diff-link', 'background'],
  ...TOKEN_KINDS.flatMap((kind): [string, string][] => [
    [`code-${kind}`, 'diff-added'],
    [`code-${kind}`, 'diff-removed'],
  ]),
];

/** [indicator, surface] */
const INDICATOR_PAIRS: [string, string][] = [
  ['input-border', 'background'],
  ['input-border', 'card'],
  ['input-border', 'popover'],
  ['input-border', 'muted'],
  ['input-border', 'sidebar'],
  ['ring', 'background'],
  ['ring', 'card'],
  ['ring', 'popover'],
  ['ring', 'muted'],
  ['sidebar-ring', 'sidebar'],
  ['sidebar-ring', 'sidebar-accent'],
  // The line beside a block a comparison says was added or removed whole.
  ['diff-added-line', 'popover'],
  ['diff-removed-line', 'popover'],
  ['diff-added-line', 'background'],
  ['diff-removed-line', 'background'],
];

describe.each(Object.entries(themes))('%s theme', (name, tokens) => {
  const token = (name: string): Color => {
    const value = tokens.get(name);
    if (!value) throw new Error(`token --${name} is not defined`);
    return value;
  };
  const page = token('background');
  const ratio = (foreground: string, surface: string): number => {
    const painted = over(token(surface), page);
    return contrast(over(token(foreground), painted), painted);
  };

  it.each(TEXT_PAIRS)('text %s on %s is at least 4.5:1', (foreground, surface) => {
    expect(ratio(foreground, surface)).toBeGreaterThanOrEqual(4.5);
  });

  it.each(INDICATOR_PAIRS)('%s on %s is at least 3:1', (indicator, surface) => {
    expect(ratio(indicator, surface)).toBeGreaterThanOrEqual(3);
  });

  it('destructive text on the tinted destructive button is at least 4.5:1', () => {
    // The button is `bg-destructive/10`, `/20` under the pointer; on the dark
    // theme `/20` and `/30`. Its label is `text-destructive`.
    const opacities = name === 'light' ? [0.1, 0.2] : [0.2, 0.3];
    for (const opacity of opacities) {
      const tinted = over(token('destructive'), page, opacity);
      expect(contrast(token('destructive'), tinted)).toBeGreaterThanOrEqual(4.5);
    }
  });

  it.each(AVATAR_COLORS)("the line of a caret in a person's %s is at least 3:1", (color) => {
    expect(caretShare).toBeGreaterThan(0);
    for (const surface of ['background', 'card', 'popover']) {
      const painted = over(token(surface), page);
      const line = over(token('foreground'), token(`option-${color}`), 1 - caretShare);
      expect(contrast(line, painted), `on ${surface}`).toBeGreaterThanOrEqual(3);
      // The name is the text colour on the person's colour.
      expect(contrast(token('foreground'), token(`option-${color}`))).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('defines every token the other theme defines', () => {
    expect([...tokens.keys()].sort()).toEqual([...themes.light.keys()].sort());
  });
});

describe('colour arithmetic', () => {
  it('gives black on white the maximum ratio', () => {
    expect(contrast(parseOklch('oklch(0 0 0)'), parseOklch('oklch(1 0 0)'))).toBeCloseTo(21, 1);
  });

  it('matches a known pair: #767676 on white is 4.54:1', () => {
    const grey = 0x76 / 255;
    const linear = ((grey + 0.055) / 1.055) ** 2.4;
    const color: Color = { r: linear, g: linear, b: linear, alpha: 1 };
    expect(contrast(color, parseOklch('oklch(1 0 0)'))).toBeCloseTo(4.54, 2);
  });

  it('composites a translucent colour in encoded sRGB', () => {
    // White at 50% over black is #808080 on screen, luminance 0.2159.
    const half = over(parseOklch('oklch(1 0 0 / 50%)'), parseOklch('oklch(0 0 0)'));
    expect(half.r).toBeCloseTo(0.214, 3);
  });
});
