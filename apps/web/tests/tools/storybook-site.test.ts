/** The files that make the built Storybook a public site (tools/storybook/site.ts). */
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

import ts from 'typescript';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { scriptHash } from '../../tools/build/inline-scripts.ts';
import {
  REDIRECTS,
  SITE_FOLDER,
  headersFile,
  pageScriptHashes,
  storybookPolicy,
  unhashedAssets,
  writeSite,
} from '../../tools/storybook/site.ts';

/** The header lines of a `_headers` block: the indented lines after the line that is the path. */
function block(file: string, pattern: string): string[] {
  const lines = file.split('\n');
  const start = lines.indexOf(pattern);
  expect(start, `no block for ${pattern}`).toBeGreaterThanOrEqual(0);
  const end = lines.findIndex((line, index) => index > start && !line.startsWith('  '));
  return lines.slice(start + 1, end).map((line) => line.trim());
}

describe('the headers of the public Storybook', () => {
  const file = headersFile(["'sha256-one'", "'sha256-two'"]);

  it('keep every response out of search engines and send its type as it is', () => {
    expect(block(file, '/*')).toEqual(
      expect.arrayContaining([
        'X-Robots-Tag: noindex, nofollow',
        'X-Content-Type-Options: nosniff',
        'Referrer-Policy: no-referrer',
      ]),
    );
  });

  it('send the policy with the hashes of the inline scripts and nothing that runs any script', () => {
    const policy = block(file, '/*').find((line) => line.startsWith('Content-Security-Policy: '));
    expect(policy).toBe(
      `Content-Security-Policy: ${storybookPolicy(["'sha256-one'", "'sha256-two'"])}`,
    );
    const directives = new Map(
      storybookPolicy(["'sha256-one'"])
        .split('; ')
        .map((directive) => [directive.split(' ')[0], directive.split(' ').slice(1)] as const),
    );
    expect(directives.get('script-src')).toEqual(["'self'", "'sha256-one'"]);
    expect(directives.get('frame-ancestors')).toEqual(["'self'"]);
    expect(directives.get('object-src')).toEqual(["'none'"]);
    expect([...directives.values()].flat()).not.toContain("'unsafe-eval'");
    expect(directives.get('script-src')).not.toContain("'unsafe-inline'");
  });

  it('keep the hashed files under assets/ for a year, and only those', () => {
    expect(block(file, '/assets/*')).toEqual([
      'Cache-Control: public, max-age=31536000, immutable',
    ]);
    expect(file.match(/Cache-Control/g)).toHaveLength(1);
  });
});

describe('a built Storybook made a site', () => {
  let folder: string;
  beforeEach(() => {
    folder = mkdtempSync(path.join(tmpdir(), 'storybook-site-'));
    mkdirSync(path.join(folder, 'assets', 'nested'), { recursive: true });
    writeFileSync(path.join(folder, 'assets', 'iframe-DKX11vrD.js'), '');
    writeFileSync(path.join(folder, 'index.html'), '<script>window.A = 1;</script>');
    writeFileSync(path.join(folder, 'iframe.html'), '<script>window.B = 2;</script>');
  });
  afterEach(() => {
    rmSync(folder, { recursive: true, force: true });
  });

  const read = (name: string) => readFileSync(path.join(folder, name), 'utf8');

  it('gets the headers with the hashes of both pages, `/` as the manager, and a robots.txt', () => {
    writeSite(folder);
    expect(read('_headers')).toBe(
      headersFile([scriptHash('window.A = 1;'), scriptHash('window.B = 2;')]),
    );
    expect(read('_redirects')).toBe(REDIRECTS);
    expect(REDIRECTS).toBe('/ /index.html 200\n');
    expect(read('robots.txt')).toBe('User-agent: *\nDisallow: /\n');
  });

  it('is refused when a file under assets/, at any depth, has no hash', () => {
    writeFileSync(path.join(folder, 'assets', 'nested', 'logo.svg'), '');
    expect(() => {
      writeSite(folder);
    }).toThrow(/nested\/logo\.svg/);
    expect(existsSync(path.join(folder, '_headers'))).toBe(false);
  });
});

describe('the Worker that serves the site (wrangler.jsonc)', () => {
  const text = readFileSync(new URL('../../wrangler.jsonc', import.meta.url), 'utf8');
  const { config } = ts.parseConfigFileTextToJson('wrangler.jsonc', text) as {
    config: Record<string, unknown>;
  };

  it('serves the folder storybook:site writes, each file at its own path', () => {
    expect(config.assets).toEqual({
      directory: `./${SITE_FOLDER}`,
      html_handling: 'none',
      not_found_handling: 'none',
    });
  });

  it('has no workers.dev address and leaves the domains attached in the dashboard alone', () => {
    expect(config.workers_dev).toBe(false);
    expect(config.preview_urls).toBe(false);
    expect(config).not.toHaveProperty('routes');
    expect(config).not.toHaveProperty('route');
    expect(config.previews).toEqual({});
  });
});

describe('the build the site is made from', () => {
  it('names the files under assets/ that lack the hash a year in the cache needs', () => {
    expect(
      unhashedAssets([
        'iframe-DKX11vrD.js',
        'Account.stories-V7fputDo.js',
        'noto-sans-kr-1-Bx_9-k2Q.woff2',
        'logo.svg',
      ]),
    ).toEqual(['logo.svg']);
  });

  it('hashes the inline scripts of both pages, each once', () => {
    const manager = '<script>window.A = 1;</script><script type="module" src="./x.js"></script>';
    const frame = '<script>window.A = 1;</script><script type="module">import "./y.js";</script>';
    expect(pageScriptHashes([manager, frame])).toEqual([
      scriptHash('window.A = 1;'),
      scriptHash('import "./y.js";'),
    ]);
  });
});
