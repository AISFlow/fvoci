/**
 * Deploys the public Storybook, or makes a Preview of it, with the pinned
 * Wrangler of tools/storybook/wrangler/.
 *
 *   node tools/storybook/wrangler.ts deploy|preview [<argument> ...]
 *
 * Wrangler is not a dependency of the web app: it brings workerd and sharp,
 * some 200 MB that no other install needs. It has a workspace of its own
 * there, installed here from its lockfile, with no install scripts. Being
 * outside the app's install, it is outside web `check`'s licence check too:
 * it runs in the deploy only and nothing of it is in what the site serves.
 *
 * Nothing goes out unless the folder is the site `storybook:site` makes: a
 * plain `build-storybook` has no `_headers`, and would be served without
 * `noindex` and without its policy.
 */
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';

import { SITE_FOLDER } from './site.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');
const WRANGLER = path.join(import.meta.dirname, 'wrangler');
const SITE_FILES = ['_headers', '_redirects', 'robots.txt'];

/** Runs a program to its end and stops here with its exit code if it fails. */
function run(program: string, args: readonly string[], cwd: string): void {
  const { status, error } = spawnSync(program, args, { cwd, stdio: 'inherit' });
  if (error) throw error;
  if (status !== 0) process.exit(status ?? 1);
}

const [command, ...rest] = process.argv.slice(2);
if (command !== 'deploy' && command !== 'preview') {
  console.error('usage: node tools/storybook/wrangler.ts deploy|preview [<argument> ...]');
  process.exit(2);
}
const missing = SITE_FILES.filter((file) => !existsSync(path.join(WEB_ROOT, SITE_FOLDER, file)));
if (missing.length > 0) {
  console.error(`${SITE_FOLDER} is not the site: no ${missing.join(', ')} (run storybook:site)`);
  process.exit(1);
}
// Under `pnpm run`, the pnpm that runs the script; it may be a script for Node.
const pnpm = process.env.npm_execpath;
const [program = 'pnpm', ...first] =
  pnpm === undefined ? ['pnpm'] : /\.[cm]?js$/.test(pnpm) ? [process.execPath, pnpm] : [pnpm];
run(program, [...first, 'install', '--frozen-lockfile'], WRANGLER);
run(path.join(WRANGLER, 'node_modules/.bin/wrangler'), [command, ...rest], WEB_ROOT);
