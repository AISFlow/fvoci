/**
 * Serves built folders for a person to look at, from another device too.
 *
 *   node tools/serve.ts [--host <address>] [--port <number>] [app:]<name>=<folder> ...
 *
 * Each folder is served under `/<name>/`; `/` lists them. It is for looking
 * at the app and Storybook on a phone on the same network:
 * static files only, no API. A `.br`, `.zst` or `.gz` file next to a file is
 * sent in its place when the client accepts it (build/encodings.ts). The
 * default host is this machine alone; `--host 0.0.0.0` listens on every
 * interface.
 *
 * A folder given as `app:<name>=<folder>` is a build of the product
 * (`dist/`) and is served the way the server serves it under the base path
 * `/<name>`: every address below it that is not a file of `static/` answers
 * with `index.html`, the base path filled in, under the content security
 * policy of the build. The app in that build runs on its in-memory backend,
 * so there is still no API here.
 *
 * A folder with a `_headers` file (the public Storybook, tools/storybook/site.ts)
 * gets the headers of its `/*` block on every file, its policy among them, as
 * the Worker that serves it sends them.
 */
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { networkInterfaces } from 'node:os';
import path from 'node:path';

import { chooseEncoding } from './build/encodings.ts';
import { inMemory } from './build/in-memory.ts';
import { contentSecurityPolicy, staticHeaders } from './build/inline-scripts.ts';

const TYPES: Record<string, string> = {
  '.css': 'text/css; charset=utf-8',
  '.gif': 'image/gif',
  '.html': 'text/html; charset=utf-8',
  '.ico': 'image/x-icon',
  '.jpg': 'image/jpeg',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.map': 'application/json',
  '.mjs': 'text/javascript; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.txt': 'text/plain; charset=utf-8',
  '.webp': 'image/webp',
  '.woff2': 'font/woff2',
};

function option(name: string, fallback: string): string {
  const index = process.argv.indexOf(name);
  return index === -1 ? fallback : (process.argv[index + 1] ?? fallback);
}

const host = option('--host', '127.0.0.1');
const port = Number(option('--port', '4180'));
const mounts = new Map<string, string>();
/** The mounts that are builds of the product. */
const apps = new Set<string>();
for (const argument of process.argv.slice(2)) {
  const match = /^(app:)?([a-z0-9-]+)=(.+)$/.exec(argument);
  if (!match?.[2] || !match[3]) continue;
  mounts.set(match[2], path.resolve(match[3]));
  if (match[1]) apps.add(match[2]);
}
if (mounts.size === 0) {
  console.error(
    'usage: node tools/serve.ts [--host <address>] [--port <number>] [app:]<name>=<folder> ...',
  );
  process.exit(2);
}
for (const [name, folder] of mounts) {
  if (!existsSync(folder)) {
    console.error(`${name}: no folder ${folder} (build it first)`);
    process.exit(2);
  }
}

/** The pages worth a link on the front page: every index.html up to two folders down. */
function pagesOf(name: string, folder: string): string[] {
  const pages: string[] = [];
  const walk = (relative: string, depth: number): void => {
    if (existsSync(path.join(folder, relative, 'index.html'))) {
      pages.push(`/${name}/${relative === '' ? '' : `${relative}/`}`);
    }
    if (depth === 0) return;
    for (const entry of readdirNames(path.join(folder, relative)).sort()) {
      walk(path.join(relative, entry), depth - 1);
    }
  };
  walk('', 2);
  return pages;
}

function readdirNames(folder: string): string[] {
  try {
    return readdirSync(folder, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && !entry.name.startsWith('.'))
      .map((entry) => entry.name);
  } catch {
    return [];
  }
}

/** The headers a folder's `_headers` gives every path: the lines of its `/*` block. */
function everyPathHeaders(folder: string): Record<string, string> {
  const file = path.join(folder, '_headers');
  if (!existsSync(file)) return {};
  const lines = readFileSync(file, 'utf8').split('\n');
  const start = lines.indexOf('/*');
  const headers: Record<string, string> = {};
  if (start === -1) return headers;
  for (const line of lines.slice(start + 1)) {
    if (!line.startsWith('  ')) break;
    const colon = line.indexOf(':');
    // `! Name` detaches a header and `#` starts a comment; neither sets one.
    if (colon === -1 || /^\s*[!#]/.test(line)) continue;
    headers[line.slice(0, colon).trim()] = line.slice(colon + 1).trim();
  }
  return headers;
}

/** The page of a product build, as the server sends it under the base path `/<name>`. */
function appPage(name: string, folder: string): { body: string; policy: string } {
  const index = readFileSync(path.join(folder, 'index.html'), 'utf8');
  const hashesFile = path.join(folder, 'csp.json');
  const hashes = existsSync(hashesFile)
    ? (JSON.parse(readFileSync(hashesFile, 'utf8')) as { 'script-src': string[] })['script-src']
    : [];
  return {
    body: inMemory(index.replaceAll('{{.BaseURL}}', `/${name}`)),
    policy: contentSecurityPolicy(hashes),
  };
}

function frontPage(): string {
  const links = [...mounts]
    .flatMap(([name, folder]) => (apps.has(name) ? [`/${name}/`] : pagesOf(name, folder)))
    .map((page) => `<li><a href="${page}">${page}</a></li>`)
    .join('\n');
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>fvoci: screens to look at</title>
<style>
  body { font: 1rem/1.6 system-ui, sans-serif; margin: 1.5rem; }
  a { display: inline-block; padding: 0.6rem 0; font-size: 1.1rem; }
</style>
</head>
<body>
<h1>fvoci</h1>
<ul>
${links}
</ul>
</body>
</html>
`;
}

createServer((request, response) => {
  const url = new URL(request.url ?? '/', 'http://localhost');
  let pathname: string;
  try {
    pathname = decodeURIComponent(url.pathname);
  } catch {
    response.writeHead(400).end();
    return;
  }
  if (pathname === '/') {
    response
      .writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-cache' })
      .end(frontPage());
    return;
  }
  const [, name = '', ...rest] = pathname.split('/');
  const folder = mounts.get(name);
  if (!folder) {
    response.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }).end('not found');
    return;
  }
  if (apps.has(name) && rest[0] !== 'static') {
    const { body, policy } = appPage(name, folder);
    response
      .writeHead(200, {
        'Content-Type': 'text/html; charset=utf-8',
        'Cache-Control': 'no-cache',
        'Content-Security-Policy': policy,
      })
      .end(body);
    return;
  }
  if (rest.length === 0) {
    // `/name` and `/name/` are different bases for the relative links of a page.
    response.writeHead(308, { Location: `/${name}/${url.search}` }).end();
    return;
  }
  let file = path.join(folder, ...rest);
  if (file !== folder && !file.startsWith(folder + path.sep)) {
    response.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }).end('not found');
    return;
  }
  if (existsSync(file) && statSync(file).isDirectory()) {
    if (!pathname.endsWith('/')) {
      response.writeHead(308, { Location: `${pathname}/${url.search}` }).end();
      return;
    }
    file = path.join(file, 'index.html');
  }
  if (!existsSync(file) || !statSync(file).isFile()) {
    response.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }).end('not found');
    return;
  }
  // A folder built with the product's build has compressed forms next to its files.
  const found = file;
  const encoding = chooseEncoding(request.headers['accept-encoding'], (extension) =>
    existsSync(found + extension),
  );
  response
    .writeHead(200, {
      ...everyPathHeaders(folder),
      'Content-Type': TYPES[path.extname(file)] ?? 'application/octet-stream',
      // Rebuilt in place while someone looks: never serve yesterday's file.
      'Cache-Control': 'no-cache',
      'X-Content-Type-Options': 'nosniff',
      Vary: 'Accept-Encoding',
      // The app's own files, as the server sends them; not the other folders' pages.
      ...(apps.has(name) && rest[0] === 'static' ? staticHeaders(file) : {}),
      ...(encoding ? { 'Content-Encoding': encoding.name } : {}),
    })
    .end(readFileSync(encoding ? file + encoding.extension : file));
}).listen(port, host, () => {
  const addresses =
    host === '0.0.0.0'
      ? Object.values(networkInterfaces())
          .flatMap((list) => list ?? [])
          .filter((address) => address.family === 'IPv4')
          .map((address) => address.address)
      : [host];
  for (const address of addresses) console.log(`serve: http://${address}:${port}/`);
  console.log(`serve: pid ${process.pid}`);
});
