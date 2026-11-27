/**
 * Fails when any file under `src` uses `100vh`, `h-screen`, `min-h-screen`,
 * or `max-h-screen` instead of `100dvh` / `h-dvh` / `min-h-dvh` / `max-h-dvh`.
 *
 *   node tools/lint/check-viewport.ts
 *
 * Part of `pnpm run check`. Takes no arguments and writes nothing.
 */
import path from 'node:path';

import { checkViewport } from './viewport.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

if (process.argv.length > 2) {
  console.error('usage: node tools/lint/check-viewport.ts (no arguments)');
  process.exit(2);
}

try {
  const { files, problems } = checkViewport(WEB_ROOT);
  if (problems.length > 0) {
    for (const problem of problems) console.error(problem);
    console.error(
      `viewport: ${problems.length} static viewport-height occurrence(s); replace 100vh with 100dvh and h-screen / min-h-screen / max-h-screen with h-dvh / min-h-dvh / max-h-dvh.`,
    );
    process.exit(1);
  }
  console.log(`viewport: ${files.length} files free of 100vh and h-screen`);
} catch (error) {
  console.error(`viewport: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
