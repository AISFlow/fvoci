/**
 * Runs `@sveltejs/mcp`'s `svelteAutofixer` over every Svelte component and
 * rune module under `src` (`.svelte`, `.svelte.ts`, `.svelte.js`), generated
 * UI primitives in `src/lib/ui` included.
 *
 * `svelteAutofixer` combines Svelte compiler diagnostics, AST rune checks
 * (such as reading a `$state` value with a `$` store prefix), and Svelte 5
 * lint errors. Loading `@sveltejs/mcp` once per worker thread keeps the whole
 * `src/` sweep within a couple of seconds without spawning `npx` per file.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { availableParallelism } from 'node:os';
import path from 'node:path';
import { Worker } from 'node:worker_threads';

import svelteConfig from '../../svelte.config.js';
import { testCpus } from '../test-cpus.ts';

/** Folder checked, relative to the web app's root. */
export const CHECKED_DIR = 'src';

/** Extensions handled by `@sveltejs/mcp`'s `svelteAutofixer`. */
export const SVELTE_EXTENSIONS = ['.svelte', '.svelte.ts', '.svelte.js'] as const;

/** Tool-generated outputs under `src` that are not Svelte sources. */
export const GENERATED = ['src/lib/i18n/paraglide/', 'src/lib/api/contract.ts'] as const;

const MAX_WORKERS = 8;
const MIN_FILES_PER_WORKER = 8;

const ASYNC_SVELTE = Boolean(
  (svelteConfig as { compilerOptions?: { experimental?: { async?: boolean } } }).compilerOptions
    ?.experimental?.async,
);

type SvelteAutofixerFn = (input: {
  code: string;
  desired_svelte_version: string | number;
  async?: boolean;
  filename?: string;
}) => Promise<{
  issues: string[];
  suggestions: string[];
  require_another_tool_call_after_fixing: boolean;
}>;

let autofixerPromise: Promise<SvelteAutofixerFn> | undefined;

/**
 * Loads `svelteAutofixer` from `@sveltejs/mcp` without letting its top-level
 * MCP resource setup reach out to `https://svelte.dev/docs/...` over the
 * network (which would otherwise open an unawaited 10s timeout timer).
 */
async function loadSvelteAutofixer(): Promise<SvelteAutofixerFn> {
  autofixerPromise ??= (async () => {
    const originalFetch = globalThis.fetch;
    const stubbedFetch: typeof fetch = async (input, init) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
      if (url.startsWith('https://svelte.dev/')) {
        return new Response('{}', {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      return originalFetch(input, init);
    };
    globalThis.fetch = stubbedFetch;
    try {
      const mod = await import('@sveltejs/mcp');
      return mod.svelteAutofixer;
    } catch (error) {
      autofixerPromise = undefined;
      throw error;
    } finally {
      if (globalThis.fetch === stubbedFetch) {
        globalThis.fetch = originalFetch;
      }
    }
  })();
  return autofixerPromise;
}

/**
 * `@sveltejs/mcp` runs its own embedded ESLint with `eslint-plugin-svelte`
 * only. Project-specific `eslint-disable` comments for `@typescript-eslint/*`
 * or `fvoci/*` are already validated by `eslint .`, so a missing-definition
 * report for those two namespaces inside the standalone autofixer is ignored.
 */
function isProjectPluginRuleMissing(issue: string): boolean {
  return /^Definition for rule '(?:@typescript-eslint|fvoci)\/[^']+' was not found\./.test(issue);
}

/** Reads the pinned Svelte version from `<root>/package.json`. */
export function readSvelteVersion(root: string): string {
  const manifest = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')) as {
    dependencies?: Record<string, string>;
    devDependencies?: Record<string, string>;
  };
  const version = manifest.devDependencies?.svelte ?? manifest.dependencies?.svelte;
  if (!version) throw new Error('package.json does not declare a `svelte` version');
  return version;
}

/**
 * Every `.svelte`, `.svelte.ts`, and `.svelte.js` file under `<root>/src`,
 * sorted by `/`-separated path from `root`. Symbolic links are refused.
 */
export function svelteFiles(root: string): string[] {
  const files: string[] = [];
  const walk = (relative: string): void => {
    const entries = readdirSync(path.join(root, relative), { withFileTypes: true });
    for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const child = `${relative}/${entry.name}`;
      if (GENERATED.some((made) => child === made || `${child}/` === made)) continue;
      if (entry.isSymbolicLink()) throw new Error(`${child} is a symbolic link`);
      if (entry.isDirectory()) walk(child);
      else if (SVELTE_EXTENSIONS.some((ext) => entry.name.endsWith(ext))) files.push(child);
    }
  };
  walk(CHECKED_DIR);
  return files;
}

/** Checks one Svelte source text and returns formatted `<file>: <issue>` entries. */
export async function checkSvelteCode(
  file: string,
  code: string,
  svelteVersion: string,
): Promise<string[]> {
  const svelteAutofixer = await loadSvelteAutofixer();
  const result = await svelteAutofixer({
    code,
    desired_svelte_version: svelteVersion,
    async: ASYNC_SVELTE,
    filename: path.basename(file),
  });
  return result.issues
    .filter((issue) => !isProjectPluginRuleMissing(issue))
    .map((issue) => `${file}: ${issue}`);
}

function workerCount(fileCount: number, requested?: number): number {
  if (fileCount === 0) return 1;
  if (requested !== undefined) {
    return Math.max(1, Math.min(MAX_WORKERS, requested, fileCount));
  }
  if (fileCount <= MIN_FILES_PER_WORKER) return 1;
  const available = testCpus() ?? availableParallelism();
  return Math.max(1, Math.min(MAX_WORKERS, available, Math.ceil(fileCount / MIN_FILES_PER_WORKER)));
}

function spawnAutofixerWorker(
  root: string,
  svelteVersion: string,
  files: readonly string[],
): { worker: Worker; promise: Promise<string[]> } {
  const worker = new Worker(new URL('./autofixer-worker.ts', import.meta.url), {
    workerData: { root, svelteVersion, files },
  });
  const promise = new Promise<string[]>((resolve, reject) => {
    let result: string[] | undefined;
    worker.once('message', (message: { problems: string[] }) => {
      result = message.problems;
    });
    worker.once('error', reject);
    worker.once('exit', (code) => {
      if (code !== 0) reject(new Error(`autofixer worker exited with code ${code}`));
      else if (result === undefined) {
        reject(new Error('autofixer worker exited without reporting results'));
      } else {
        resolve(result);
      }
    });
  });
  return { worker, promise };
}

export interface AutofixerCheckResult {
  files: string[];
  svelteVersion: string;
  problems: string[];
}

/** Runs `svelteAutofixer` across all Svelte files under `<root>/src`. */
export async function checkAutofixer(
  root: string,
  options?: { svelteVersion?: string; workers?: number },
): Promise<AutofixerCheckResult> {
  const files = svelteFiles(root);
  const svelteVersion = options?.svelteVersion ?? readSvelteVersion(root);
  const count = workerCount(files.length, options?.workers);

  if (count <= 1) {
    const problems: string[] = [];
    for (const file of files) {
      const code = readFileSync(path.join(root, file), 'utf8');
      problems.push(...(await checkSvelteCode(file, code, svelteVersion)));
    }
    return { files, svelteVersion, problems };
  }

  const buckets = Array.from({ length: count }, (): string[] => []);
  for (const [index, file] of files.entries()) {
    buckets[index % count]?.push(file);
  }
  const active = buckets
    .filter((bucket) => bucket.length > 0)
    .map((bucket) => spawnAutofixerWorker(root, svelteVersion, bucket));

  try {
    const batches = await Promise.all(active.map((entry) => entry.promise));
    const problems = batches.flat().sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
    return { files, svelteVersion, problems };
  } finally {
    await Promise.allSettled(active.map((entry) => entry.worker.terminate()));
  }
}
