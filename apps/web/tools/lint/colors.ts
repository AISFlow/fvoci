/**
 * Static check: a colour is a token of the theme. Raw colour literals (`#fff`,
 * `rgb(…)`, `hsl(…)`, `oklch(…)` and the like) and the classes of Tailwind's own
 * palette (`bg-red-500`, `text-white`) are refused under `src` and in the HTML
 * entry points, but in the files of `COLOR_HOMES`, each with its reason. The list
 * is this constant, not a file of exceptions: a new home is a change of this code.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { viewportFiles } from './viewport.ts';

/** The files where colours are written as values, and why. */
export const COLOR_HOMES: readonly { file: string; reason: string }[] = [
  { file: 'src/styles/app.css', reason: 'the tokens of the theme, light and dark' },
  {
    file: 'src/lib/logic/display.ts',
    reason: "the browser's bar (`theme-color`), which no style of the page reaches",
  },
  { file: 'index.html', reason: "the same bar, set before the app's script runs" },
];

/** A hex colour where a value stands: in quotes, brackets, a list, or after a colon. */
const HEX = /(?<=['"`[(,]|:\s*)#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{3,4})(?![\w-])/gi;
/**
 * A hex colour among the other values of a declaration (`1px solid #ccc`, `0 0 0 1px #000`,
 * `linear-gradient(90deg, #fff, #000)`): after a space that follows another value, past the
 * colon of a property (one right after the colon is `HEX`'s). Not in a comment, where `#123`
 * is a card's number.
 */
const SPACED_HEX =
  /(?<=[\w-]\s*:[^;{}\n]*[^\s;{}:]\s+)#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{3,4})(?![\w-])/gi;
/** A line that is a comment, or goes on one. */
const COMMENT = /^\s*(?:\/\/|\/?\*|<!--)/;
/** A colour written as a function of CSS. */
const FUNCTION = /\b(?:rgba?|hsla?|hwb|oklch|oklab|lab|lch)\(/gi;
const HUES =
  'red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone';
const UTILITIES =
  'bg|text|border|ring|fill|stroke|outline|decoration|from|via|to|divide|shadow|accent|caret|placeholder';
/** A class of Tailwind's palette, in place of a token. */
const PALETTE = new RegExp(
  `(?<![\\w-])(?:${UTILITIES})-(?:(?:${HUES})-\\d{2,3}|black|white)(?![\\w-])`,
  'g',
);

/** The raw colours of `source`, one line each: `file:line: what`. */
export function findRawColors(file: string, source: string): string[] {
  if (COLOR_HOMES.some((home) => home.file === file)) return [];
  const found: string[] = [];
  source.split(/\r\n|\r|\n/).forEach((line, index) => {
    const patterns = COMMENT.test(line)
      ? [HEX, FUNCTION, PALETTE]
      : [HEX, SPACED_HEX, FUNCTION, PALETTE];
    for (const pattern of patterns) {
      for (const match of line.matchAll(pattern)) {
        found.push(
          `${file}:${index + 1}: \`${match[0]}\` is a raw colour; use a token of the theme`,
        );
      }
    }
  });
  return found;
}

/** Every checked file under `root`, and the raw colours found in them. */
export function checkColors(root: string): { files: string[]; problems: string[] } {
  const files = viewportFiles(root);
  const problems = files.flatMap((file) =>
    findRawColors(file, readFileSync(path.join(root, file), 'utf8')),
  );
  return { files, problems };
}
