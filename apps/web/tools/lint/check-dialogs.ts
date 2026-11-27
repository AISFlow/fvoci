/**
 * Out-of-band check refusing bare `eslint-disable` comments or inline
 * suppression of the browser-dialog rules under `src/**`.
 *
 *   node tools/lint/check-dialogs.ts
 */
import path from 'node:path';

import { checkDialogs } from './dialogs.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

if (process.argv.length > 2) {
  console.error('usage: node tools/lint/check-dialogs.ts (no arguments)');
  process.exit(2);
}

try {
  const { files, problems } = checkDialogs(WEB_ROOT);
  if (problems.length > 0) {
    for (const problem of problems) console.error(problem);
    console.error(`dialogs: ${problems.length} forbidden eslint-disable directive(s) found`);
    process.exit(1);
  }
  console.log(`dialogs: ${files.length} files clean`);
} catch (error) {
  console.error(`dialogs: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
