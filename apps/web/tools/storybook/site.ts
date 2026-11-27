/**
 * Makes the built Storybook a public site: writes `_headers`, `_redirects` and
 * `robots.txt` into it, for the Worker `fvoci-web-storybook` (wrangler.jsonc) that serves
 * the folder as static files.
 *
 *   node tools/storybook/site.ts [<folder>]   (default: storybook-static)
 *
 * The site is open to anyone and is kept out of search engines: every
 * response says `noindex`, and `robots.txt` asks crawlers to stay away. The
 * policy lets the pages run only the site's own scripts and the inline
 * scripts of its two pages, by their hashes; those scripts change with the
 * configuration of Storybook, so the hashes are read from this build.
 *
 * It fails when a file under `assets/` has no hash in its name: those files
 * are cached for a year without asking again, which is right only when a
 * changed file gets a new name.
 */
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { inlineScripts, scriptHash } from '../build/inline-scripts.ts';

/** The folder `storybook build` writes and the Worker serves (`assets.directory` of wrangler.jsonc). */
export const SITE_FOLDER = 'storybook-static';

/** The pages Storybook writes with inline scripts: the manager, and the frame a story renders in. */
const PAGES = ['index.html', 'iframe.html'];

/**
 * The names Vite gives to what it writes under `assets/`: `<name>-<hash>.<extension>`,
 * the hash eight characters of base64url. A name of that shape without a hash
 * (`my-filename.js`) passes too: the check catches a file copied in under its
 * own name, not every name.
 */
const HASHED = /-[\w-]{8}\.[a-z0-9]+$/i;

/**
 * The policy of every page and file of the site.
 *
 * - Scripts: the site's own files and the inline scripts of its pages, by hash.
 *   No `unsafe-inline`, no `unsafe-eval`.
 * - Styles: the site's own files and inline styles. Storybook's manager writes
 *   its styles into `<style>` elements, and the app's parts write `style`
 *   attributes (tools/build/inline-scripts.ts says which).
 * - Images and fonts: the site's own files, `data:` and `blob:` (stories hand
 *   out pictures and files that way, as the in-memory backend does).
 * - Frames: the manager frames its own `iframe.html`; nothing else frames the
 *   site, and it frames nothing from elsewhere.
 * - No plugins, no `<base>` pointing elsewhere, forms only to the site.
 */
export function storybookPolicy(scriptHashes: readonly string[]): string {
  return [
    "default-src 'self'",
    ["script-src 'self'", ...scriptHashes].join(' '),
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "font-src 'self' data:",
    "connect-src 'self'",
    "worker-src 'self' blob:",
    "frame-src 'self'",
    "frame-ancestors 'self'",
    "object-src 'none'",
    "base-uri 'self'",
    "form-action 'self'",
  ].join('; ');
}

/**
 * The `_headers` of the site (https://developers.cloudflare.com/workers/static-assets/headers/).
 *
 * Every response: not to be indexed, its type as sent, no referrer (a
 * preview's address names its branch), and the policy. Files under `assets/`
 * have their hash in their name and are kept for a year. Everything else,
 * the two pages, `index.json` and the files of Storybook's own folders whose
 * names do not change, keeps the default of Workers: kept, but asked about
 * again before each use (`max-age=0, must-revalidate` with an `ETag`).
 */
export function headersFile(scriptHashes: readonly string[]): string {
  return [
    '/*',
    '  X-Robots-Tag: noindex, nofollow',
    '  X-Content-Type-Options: nosniff',
    '  Referrer-Policy: no-referrer',
    `  Content-Security-Policy: ${storybookPolicy(scriptHashes)}`,
    '',
    '/assets/*',
    '  Cache-Control: public, max-age=31536000, immutable',
    '',
  ].join('\n');
}

/**
 * The `_redirects` of the site: `/` answers with the manager without a
 * redirect. Every other path is served as exactly the file it names
 * (`html_handling: "none"` in wrangler.jsonc), so `iframe.html` is not sent on
 * to `/iframe` first, as the default handling of HTML would.
 */
export const REDIRECTS = '/ /index.html 200\n';

/** Asks every crawler to stay away from the whole site. */
export const ROBOTS = 'User-agent: *\nDisallow: /\n';

/** The files under `assets/` whose names carry no hash. */
export function unhashedAssets(names: readonly string[]): string[] {
  return names.filter((name) => !HASHED.test(name));
}

/** The hashes of the inline scripts of the site's pages, each once. */
export function pageScriptHashes(pages: readonly string[]): string[] {
  return [...new Set(pages.flatMap((html) => inlineScripts(html).map(scriptHash)))];
}

/** The files under a folder, at any depth, by their paths from it. */
function filesUnder(folder: string): string[] {
  return readdirSync(folder, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => path.relative(folder, path.join(entry.parentPath, entry.name)));
}

/** Writes the three files into the built Storybook in `folder`. */
export function writeSite(folder: string): void {
  const unhashed = unhashedAssets(filesUnder(path.join(folder, 'assets')));
  if (unhashed.length > 0) {
    throw new Error(`assets/ has files without a hash in their name: ${unhashed.join(', ')}`);
  }
  const hashes = pageScriptHashes(
    PAGES.map((page) => readFileSync(path.join(folder, page), 'utf8')),
  );
  writeFileSync(path.join(folder, '_headers'), headersFile(hashes));
  writeFileSync(path.join(folder, '_redirects'), REDIRECTS);
  writeFileSync(path.join(folder, 'robots.txt'), ROBOTS);
  console.warn(
    `storybook site: _headers (${hashes.length} inline script hashes), _redirects, robots.txt`,
  );
}

if (import.meta.main) {
  writeSite(path.resolve(process.argv[2] ?? SITE_FOLDER));
}
