import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { hasContentHash } from '../../tools/build/output.ts';
import { styleSheetPackages } from '../../tools/licenses/notices.ts';
import { isAllowed, readAllowedLicenses } from '../../tools/licenses/policy.ts';
import known from '../../tools/licenses/rhwp-notices.json' with { type: 'json' };
import {
  RHWP_ACCEPTED,
  RHWP_CRATE_ACCEPTED,
  rhwpDataNotices,
  rhwpWasmLicenses,
} from '../../tools/licenses/rhwp-wasm.ts';
import { packageDirectoryOf } from '../../tools/licenses/worker-packages.ts';

describe('licence policy', () => {
  const allowed = new Set(readAllowedLicenses());

  it('reads the list from deny.toml', () => {
    expect(allowed.has('MIT')).toBe(true);
    expect(allowed.has('Apache-2.0 WITH LLVM-exception')).toBe(true);
    expect(allowed.size).toBeGreaterThanOrEqual(10);
  });

  it('has no copyleft licence on the list', () => {
    for (const name of allowed) expect(name).not.toMatch(/GPL|MPL|EPL|CDDL|EUPL/);
  });

  it.each([
    ['MIT', true],
    ['(MIT OR CC0-1.0)', true],
    ['MIT OR GPL-3.0-only', true],
    ['MIT AND GPL-3.0-only', false],
    ['(Apache-2.0 WITH LLVM-exception)', true],
    ['Apache-2.0 WITH Classpath-exception-2.0', false],
    ['(MIT AND Zlib) OR GPL-2.0-only', true],
    ['MPL-2.0', false],
    ['GPL-3.0-or-later', false],
    ['SEE LICENSE IN LICENSE.md', false],
    ['', false],
    ['MIT OR', false],
    ['(MIT', false],
  ])('%j is allowed: %s', (expression, expected) => {
    expect(isAllowed(expression, allowed)).toBe(expected);
  });
});

describe('packages named by style sheet imports', () => {
  it('finds bare specifiers and ignores relative ones', () => {
    const css = `@import 'tailwindcss';\n@import "tw-animate-css";\n@import './fonts.css';\n@import '@scope/pkg/file.css';\n@import 'shadcn-svelte/tailwind.css';`;
    expect(styleSheetPackages(css)).toEqual([
      '@scope/pkg',
      'shadcn-svelte',
      'tailwindcss',
      'tw-animate-css',
    ]);
  });
});

describe('content hash in a file name, as the server sees it', () => {
  it.each([
    ['index-5f6bf5c286feeaf1.js', true],
    ['noto-sans-kr-common-0a1b2c3d4e5f6071.woff2', true],
    ['index-DP28MaF1.js', false],
    ['index-abcdefab.js', false],
    ['index-1234567.js', false],
    ['THIRD_PARTY_NOTICES.txt', false],
  ])('%s: %s', (name, expected) => {
    expect(hasContentHash(name)).toBe(expected);
  });
});

describe("the packages of the app's workers", () => {
  it('are found by the path of each module', () => {
    const store = '/w/node_modules/.pnpm/saxes@6.0.0/node_modules';
    expect(packageDirectoryOf(`${store}/saxes/lib/saxes.js`)).toBe(`${store}/saxes`);
    expect(packageDirectoryOf(`${store}/@scope/name/dist/a.js?inline`)).toBe(
      `${store}/@scope/name`,
    );
    expect(packageDirectoryOf('/w/apps/web/src/viewer-frame/pptx-worker.ts')).toBeUndefined();
    expect(packageDirectoryOf('\0vite/preload-helper.js')).toBeUndefined();
  });
});

describe("the notices of the HWP renderer's WebAssembly", () => {
  const allowed = new Set(readAllowedLicenses());

  it('are of the installed version, under licences accepted for it by name alone', () => {
    expect(rhwpWasmLicenses(['/w/src/other.ts'])).toEqual([]);
    const [own, crates, data] = rhwpWasmLicenses(['\0virtual:rhwp-wasm']);
    expect(own?.identifier).toBe('MIT');
    for (const item of [crates, data]) {
      if (!item) throw new Error('an item is missing');
      // The repository's list alone refuses it: what it has more is accepted for it by name.
      expect(isAllowed(item.identifier, allowed)).toBe(false);
      expect(isAllowed(item.identifier, new Set([...allowed, ...(item.accepted ?? [])]))).toBe(
        true,
      );
      expect(item.text.length).toBeGreaterThan(10_000);
    }
    expect(crates?.accepted).toEqual(['IJG']);
    expect(data?.text).toContain('Independent JPEG Group');
    expect(data?.text).toContain('GUST Font License');
    expect(data?.text).toContain('urw-base35-fonts');
    expect(data?.text).toContain('Core 14 AFM Files');
  });

  it("accepts no licence of the repository's list again, and each with its reason", () => {
    for (const { license, reason } of known.accepted) {
      expect(allowed.has(license), license).toBe(false);
      expect(reason.length, license).toBeGreaterThan(40);
    }
    expect(RHWP_ACCEPTED).toEqual(known.accepted.map((item) => item.license));
    for (const license of RHWP_CRATE_ACCEPTED) expect(RHWP_ACCEPTED).toContain(license);
  });

  it("generates the notices of the crates with the repository's list and what a crate has more", () => {
    const file = path.resolve(import.meta.dirname, '../../tools/licenses/rhwp-about.toml');
    const accepted = /^accepted = \[([\s\S]*?)\]/m.exec(readFileSync(file, 'utf8'))?.[1] ?? '';
    const names = [...accepted.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
    expect(names).toEqual([...readAllowedLicenses(), ...RHWP_CRATE_ACCEPTED]);
  });

  it('stops on a notice the list does not name, and on one it names that is gone', () => {
    const folder = mkdtempSync(path.join(tmpdir(), 'fvoci-notices-'));
    try {
      mkdirSync(path.join(folder, 'fonts'));
      writeFileSync(path.join(folder, 'LICENSE'), 'one\n');
      writeFileSync(path.join(folder, 'fonts/list.json'), '{}');
      expect(rhwpDataNotices(folder, ['LICENSE'], ['fonts/list.json'])).toMatch(
        /^=+\nLICENSE\n=+\n\none$/,
      );
      writeFileSync(path.join(folder, 'fonts/NEW-LICENSE.txt'), 'two\n');
      expect(() => rhwpDataNotices(folder, ['LICENSE'], ['fonts/list.json'])).toThrow(
        /new: fonts\/NEW-LICENSE\.txt/,
      );
      expect(() =>
        rhwpDataNotices(
          folder,
          ['LICENSE', 'fonts/NEW-LICENSE.txt', 'NOTICE'],
          ['fonts/list.json'],
        ),
      ).toThrow(/gone: NOTICE/);
    } finally {
      rmSync(folder, { recursive: true, force: true });
    }
  });
});
