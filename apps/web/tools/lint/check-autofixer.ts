/**
 * Fails when `@sveltejs/mcp`'s `svelteAutofixer` reports an issue in any
 * `.svelte`, `.svelte.ts`, or `.svelte.js` file under `src`.
 *
 *   node tools/lint/check-autofixer.ts
 *
 * Part of `pnpm run check`. Takes no arguments and writes nothing.
 */
import path from 'node:path';

import { checkAutofixer } from './autofixer.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

if (process.argv.length > 2) {
  console.error('usage: node tools/lint/check-autofixer.ts (no arguments)');
  process.exit(2);
}

try {
  const { files, svelteVersion, problems } = await checkAutofixer(WEB_ROOT);
  if (problems.length > 0) {
    for (const problem of problems) console.error(problem);
    console.error(
      `autofixer: ${problems.length} issue(s) across ${files.length} Svelte files (svelte ${svelteVersion})`,
    );
    process.exit(1);
  }
  console.log(
    `autofixer: ${files.length} Svelte files checked with 0 issues (svelte ${svelteVersion})`,
  );
} catch (error) {
  console.error(`autofixer: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
