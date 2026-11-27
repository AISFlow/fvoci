/**
 * Fails when a file under `src` or an HTML entry point writes a raw colour
 * outside the homes of `colors.ts`.
 *
 *   node tools/lint/check-colors.ts
 *
 * Part of `pnpm run check`. Takes no arguments and writes nothing.
 */
import path from 'node:path';

import { checkColors } from './colors.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

if (process.argv.length > 2) {
  console.error('usage: node tools/lint/check-colors.ts (no arguments)');
  process.exit(2);
}

const { files, problems } = checkColors(WEB_ROOT);
if (problems.length > 0) {
  for (const problem of problems) console.error(problem);
  console.error(`colors: ${problems.length} raw colour(s) outside the theme's tokens`);
  process.exit(1);
}
console.log(`colors: ${files.length} files use the theme's tokens`);
