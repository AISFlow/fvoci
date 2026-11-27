/**
 * Fails when any file (> 500 lines) or function (> 100 lines) under `src`
 * exceeds its limit (zero-exception gate). See `lengths.ts`.
 *
 *   node tools/lint/check-lengths.ts
 */
import path from 'node:path';

import { MAX_FILE_LINES, MAX_FUNCTION_LINES, check } from './lengths.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

if (process.argv.length > 2) {
  console.error('usage: node tools/lint/check-lengths.ts (no arguments)');
  process.exit(2);
}

try {
  const { measured, problems } = await check(WEB_ROOT);
  if (problems.length > 0) {
    for (const problem of problems) console.error(problem);
    console.error(
      `lengths: ${problems.length} violation(s). Split or shorten files over ${MAX_FILE_LINES} ` +
        `lines and functions over ${MAX_FUNCTION_LINES} lines (zero exceptions allowed).`,
    );
    process.exit(1);
  }
  const files = measured.filter((length) => length.kind === 'file').length;
  console.log(
    `lengths: ${files} files within ${MAX_FILE_LINES} lines and ${measured.length - files} functions ` +
      `within ${MAX_FUNCTION_LINES} lines (0 exceptions)`,
  );
} catch (error) {
  console.error(`lengths: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
