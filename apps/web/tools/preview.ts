/**
 * Serves `dist/` the way the server does, for looking at a build without it.
 *
 *   node tools/preview.ts [port] [base path]
 *
 * Every path that is not under `static/` answers with `index.html`, under a
 * content security policy (tools/build/inline-scripts.ts), with the
 * base path filled into the placeholder, and marked to run on the backend
 * in memory (`src/app/backend.ts`): there is no API here. Files under `static/` are sent with
 * the cache rule the server uses, and a `.br`, `.zst` or `.gz` file next to
 * one is sent in its place when the client accepts it (build/encodings.ts
 * has the order). It is a development aid: the end-to-end tests of the
 * product run against the real server.
 */
import { existsSync, readFileSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import path from 'node:path';

import { chooseEncoding } from './build/encodings.ts';
import { inMemory } from './build/in-memory.ts';
import { contentSecurityPolicy, staticHeaders } from './build/inline-scripts.ts';
import { hasContentHash } from './build/output.ts';

const DIST = path.resolve(import.meta.dirname, '../dist');
const port = Number(process.argv[2] ?? 4173);
const basePath = (process.argv[3] ?? '').replace(/\/$/, '');

const TYPES: Record<string, string> = {
  '.css': 'text/css; charset=utf-8',
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.txt': 'text/plain; charset=utf-8',
  '.woff2': 'font/woff2',
};

/** The policy for the page, with the hashes the build wrote for its inline script. */
function policy(): string {
  const file = path.join(DIST, 'csp.json');
  const hashes = existsSync(file)
    ? (JSON.parse(readFileSync(file, 'utf8')) as { 'script-src': string[] })['script-src']
    : [];
  return contentSecurityPolicy(hashes);
}

createServer((request, response) => {
  const url = new URL(request.url ?? '/', 'http://localhost');
  const pathname = decodeURIComponent(url.pathname);
  const inApp = basePath === '' || pathname === basePath || pathname.startsWith(`${basePath}/`);
  const relative = inApp ? pathname.slice(basePath.length) : pathname;

  if (inApp && relative.startsWith('/static/')) {
    const file = path.join(DIST, relative);
    if (
      !file.startsWith(path.join(DIST, 'static')) ||
      !existsSync(file) ||
      !statSync(file).isFile()
    ) {
      response.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }).end('not found');
      return;
    }
    const encoding = chooseEncoding(request.headers['accept-encoding'], (extension) =>
      existsSync(file + extension),
    );
    const headers: Record<string, string> = {
      'Content-Type': TYPES[path.extname(file)] ?? 'application/octet-stream',
      'Cache-Control': hasContentHash(path.basename(file))
        ? 'public, max-age=31536000, immutable'
        : 'no-cache',
      Vary: 'Accept-Encoding',
      ...staticHeaders(file),
    };
    if (encoding) {
      headers['Content-Encoding'] = encoding.name;
      response.writeHead(200, headers).end(readFileSync(file + encoding.extension));
    } else {
      response.writeHead(200, headers).end(readFileSync(file));
    }
    return;
  }

  const index = readFileSync(path.join(DIST, 'index.html'), 'utf8');
  response
    .writeHead(200, {
      'Content-Type': 'text/html; charset=utf-8',
      'Cache-Control': 'no-cache',
      // The page runs under a policy here as it will on the server, so that
      // the browser tests notice anything the policy would stop.
      'Content-Security-Policy': policy(),
    })
    .end(inMemory(index.replaceAll('{{.BaseURL}}', basePath)));
}).listen(port, '127.0.0.1', () => {
  console.log(`preview: http://127.0.0.1:${port}${basePath}/`);
});
