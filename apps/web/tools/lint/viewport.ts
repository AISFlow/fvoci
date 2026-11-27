/**
 * Static check forbidding static/large viewport-height units and classes
 * (`100vh`, `100lvh`, `h-screen`, `min-h-screen`, `max-h-screen`, `h-lvh`,
 * `min-h-lvh`, `max-h-lvh`, and `calc`/`min`/`max`/`clamp` expressions using
 * `vh` or `lvh`) under `src` and the app's HTML entry points (`index.html`,
 * `static/*.html`).
 *
 * On mobile browsers `vh` and `lvh` include the area behind dynamic browser
 * toolbars, clipping controls at the bottom or introducing unwanted scrollbars.
 * Use the dynamic viewport equivalents (`100dvh`, `h-dvh`, `min-h-dvh`, `max-h-dvh`).
 */
import { existsSync, lstatSync, readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

export const CHECKED_DIR = 'src';
export const VIEWPORT_EXTENSIONS = ['.svelte', '.ts', '.js', '.css', '.html'] as const;
export const GENERATED = [
  'src/lib/i18n/paraglide/',
  'src/styles/fonts/',
  'src/lib/api/contract.ts',
] as const;

const STATIC_VH = /(?<![A-Za-z0-9.])100(?:\.0+)?(l?vh)(?![A-Za-z0-9])/gi;
const SCREEN_HEIGHT_CLASS = /(?<![\w-])((?:min-|max-)?h-(?:screen|lvh))(?![\w-])/g;
const MATH_FN = /\b(?:calc|min|max|clamp)\s*\(/gi;
const INNER_STATIC_VH = /(?<![A-Za-z0-9.])(\d+(?:\.\d+)?)(l?vh)(?![A-Za-z0-9])/gi;

const CLASS_REPLACEMENTS: Readonly<Record<string, string>> = {
  'h-screen': 'h-dvh',
  'min-h-screen': 'min-h-dvh',
  'max-h-screen': 'max-h-dvh',
  'h-lvh': 'h-dvh',
  'min-h-lvh': 'min-h-dvh',
  'max-h-lvh': 'max-h-dvh',
};

/**
 * Every source, style, and HTML entry file under `<root>` checked for static
 * viewport-height units, sorted by `/`-separated path from `root`.
 */
export function viewportFiles(root: string): string[] {
  const files: string[] = [];

  const indexHtml = path.join(root, 'index.html');
  if (existsSync(indexHtml)) {
    const stat = lstatSync(indexHtml);
    if (stat.isSymbolicLink()) throw new Error('index.html is a symbolic link');
    if (stat.isFile()) files.push('index.html');
  }

  const staticDir = path.join(root, 'static');
  if (existsSync(staticDir)) {
    const stat = lstatSync(staticDir);
    if (stat.isSymbolicLink()) throw new Error('static is a symbolic link');
    if (stat.isDirectory()) {
      for (const entry of readdirSync(staticDir, { withFileTypes: true }).sort((a, b) =>
        a.name < b.name ? -1 : 1,
      )) {
        const child = `static/${entry.name}`;
        if (entry.isSymbolicLink()) throw new Error(`${child} is a symbolic link`);
        if (entry.isFile() && entry.name.endsWith('.html')) files.push(child);
      }
    }
  }

  const walk = (relative: string): void => {
    const entries = readdirSync(path.join(root, relative), { withFileTypes: true });
    for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const child = `${relative}/${entry.name}`;
      if (GENERATED.some((made) => child === made || `${child}/` === made)) continue;
      if (entry.isSymbolicLink()) throw new Error(`${child} is a symbolic link`);
      if (entry.isDirectory()) walk(child);
      else if (VIEWPORT_EXTENSIONS.some((ext) => entry.name.endsWith(ext))) files.push(child);
    }
  };
  walk(CHECKED_DIR);
  return files.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
}

/** Ranges of `calc(...)` / `min(...)` / `max(...)` / `clamp(...)` calls on a line. */
function mathCallRanges(line: string): [number, number][] {
  const ranges: [number, number][] = [];
  for (const match of line.matchAll(MATH_FN)) {
    const openAt = line.indexOf('(', match.index);
    if (openAt === -1) continue;
    let depth = 0;
    for (let i = openAt; i < line.length; i += 1) {
      if (line.charAt(i) === '(') depth += 1;
      else if (line.charAt(i) === ')') {
        depth -= 1;
        if (depth === 0) {
          ranges.push([match.index, i + 1]);
          break;
        }
      }
    }
  }
  return ranges;
}

/** Finds static `vh` / `lvh` units, classes, and CSS math expressions in `source`. */
export function findViewportViolations(file: string, source: string): string[] {
  const violations: string[] = [];
  const lines = source.split(/\r\n|\r|\n/);

  for (const [index, line] of lines.entries()) {
    const hits: { column: number; message: string }[] = [];
    const seenColumns = new Set<number>();

    for (const match of line.matchAll(STATIC_VH)) {
      seenColumns.add(match.index);
      hits.push({
        column: match.index,
        message: `${file}:${index + 1}: \`${match[0]}\` uses the static viewport height; use \`100dvh\` (or \`h-dvh\`) instead`,
      });
    }

    for (const [start, end] of mathCallRanges(line)) {
      const slice = line.slice(start, end);
      for (const match of slice.matchAll(INNER_STATIC_VH)) {
        const column = start + match.index;
        if (seenColumns.has(column)) continue;
        seenColumns.add(column);
        const amount = match[1] ?? '';
        hits.push({
          column,
          message: `${file}:${index + 1}: \`${match[0]}\` uses a static viewport height inside a CSS math expression; use \`${amount}dvh\` instead`,
        });
      }
    }

    for (const match of line.matchAll(SCREEN_HEIGHT_CLASS)) {
      const token = match[1] ?? match[0];
      const replacement = CLASS_REPLACEMENTS[token] ?? 'h-dvh';
      const mapped = token.endsWith('-lvh') ? '100lvh' : '100vh';
      hits.push({
        column: match.index,
        message: `${file}:${index + 1}: \`${token}\` maps to \`${mapped}\`; use \`${replacement}\` (\`100dvh\`) instead`,
      });
    }

    hits.sort((a, b) => a.column - b.column);
    for (const hit of hits) violations.push(hit.message);
  }

  return violations;
}

/** Scans all checked files under `<root>` for viewport-height anti-patterns. */
export function checkViewport(root: string): { files: string[]; problems: string[] } {
  const files = viewportFiles(root);
  const problems: string[] = [];
  for (const file of files) {
    const source = readFileSync(path.join(root, file), 'utf8');
    problems.push(...findViewportViolations(file, source));
  }
  return { files, problems };
}
