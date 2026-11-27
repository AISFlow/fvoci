/**
 * The inline scripts of `index.html`, for a content security policy.
 *
 * `index.html` carries one script that has to run before the style sheet
 * paints (it puts the remembered theme and density on the root
 * element). A policy that forbids inline scripts, as it should, has to allow
 * exactly that one, and the way to do so without `unsafe-inline` is its hash.
 * The build writes the hashes to `csp.json` beside `index.html`:
 *
 *   { "script-src": ["'sha256-…'"] }
 *
 * The server reads the file when it starts and adds the values to the
 * `script-src` of the policy it sends with `index.html`. The file is not
 * under `static/` and is not served.
 */
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

import type { Plugin } from 'vite';

/** The text of every inline script of a page: `<script>` elements without a `src`. */
export function inlineScripts(html: string): string[] {
  const scripts: string[] = [];
  for (const match of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)) {
    const [, attributes = '', text = ''] = match;
    if (/\bsrc\s*=/.test(attributes)) continue;
    scripts.push(text);
  }
  return scripts;
}

/** The source expression a policy names a script by: the SHA-256 of its text, exactly as written. */
export function scriptHash(text: string): string {
  return `'sha256-${createHash('sha256').update(text, 'utf8').digest('base64')}'`;
}

/**
 * A content security policy the app runs under, given the hashes of
 * `csp.json`. The preview server sends it, so that the browser tests run
 * under it; it is also the starting point for the policy of the real server.
 *
 * - Scripts: the app's own files and the one hashed inline script. Nothing
 *   else: no `unsafe-inline`, no `unsafe-eval`.
 * - Styles: the app's own files, and inline styles. Inline styles are needed:
 *   the parts that place popovers, the editor and the formula renderer write
 *   `style` attributes.
 * - Images: the app's own origin (files of a board are served from it) and
 *   `blob:` (the in-memory backend hands out files that way).
 * - No plugins, no framing by another site, no `<base>`.
 */
export function contentSecurityPolicy(scriptHashes: readonly string[]): string {
  return [
    "default-src 'self'",
    ["script-src 'self'", ...scriptHashes].join(' '),
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' blob:",
    "font-src 'self'",
    "connect-src 'self'",
    "object-src 'none'",
    "base-uri 'none'",
    "form-action 'self'",
    "frame-ancestors 'none'",
  ].join('; ');
}

/** Writes `csp.json` next to the built `index.html`. */
export function cspHashes(): Plugin {
  return {
    name: 'fvoci:csp-hashes',
    apply: 'build',
    async closeBundle() {
      const { root, build } = this.environment.config;
      const directory = path.resolve(root, build.outDir);
      let html: string;
      try {
        html = await readFile(path.join(directory, 'index.html'), 'utf8');
      } catch {
        // The build failed before it wrote the page; its own error says why.
        return;
      }
      const hashes = inlineScripts(html).map(scriptHash);
      await writeFile(
        path.join(directory, 'csp.json'),
        `${JSON.stringify({ 'script-src': hashes }, null, 2)}\n`,
      );
      this.info(`csp: ${hashes.length} inline script(s) hashed into csp.json`);
    },
  };
}

/**
 * Headers every file under `static/` carries, as the server sends them
 * (crates/server/src/web.rs).
 *
 * - `Access-Control-Allow-Origin: *`: the file viewer's frame has no origin
 *   of its own, so loading the app's scripts, style sheets and fonts is a
 *   cross-origin request for it. The files are the public build; nothing
 *   is sent with them that a stranger could not fetch.
 * - An HTML document there (the viewer's frame) is sandboxed by a header too,
 *   so that opened as a page of its own it has no origin either.
 */
export function staticHeaders(fileName: string): Record<string, string> {
  return {
    'Access-Control-Allow-Origin': '*',
    ...(path.extname(fileName) === '.html'
      ? { 'Content-Security-Policy': 'sandbox allow-scripts' }
      : {}),
  };
}
