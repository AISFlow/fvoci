/**
 * The HWP editor's source as the repository holds it (vendor/rhwp-studio):
 * of one version with the engine, changed only where the package says, and
 * brought by an import that takes no file it was not told of.
 */
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import {
  agreed,
  changedFromUpstream,
  namedAsPatched,
  rhwpVersions,
  studioDirectory,
  studioVersion,
} from '../../tools/build/rhwp-studio.ts';
import { readAllowedLicenses } from '../../tools/licenses/policy.ts';
import { rhwpStudioLicenses } from '../../tools/licenses/rhwp-studio.ts';

/** The import's choice of files (vendor/rhwp-studio/import.mjs), which has no types of its own. */
type Chosen = (source: string, assets: Record<string, unknown>) => string[];
const importer = path.join(studioDirectory(), 'import.mjs');
const chosen = ((await import(importer)) as { chosen: Chosen }).chosen;

describe("rhwp's engine and editor", () => {
  it('are of one version, everywhere it is said', () => {
    const versions = rhwpVersions();
    expect(Object.keys(versions)).toHaveLength(5);
    expect(new Set(Object.values(versions)).size).toBe(1);
    expect(studioVersion()).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it('stop the build when one of them moved alone', () => {
    const versions = rhwpVersions();
    for (const who of Object.keys(versions)) {
      expect(() => agreed({ ...versions, [who]: '0.8.7' }), who).toThrow(/move only together/);
      expect(() => agreed({ ...versions, [who]: '' }), who).toThrow(/\(none\)/);
    }
    // A range is not a version: the engine is installed at exactly the editor's.
    const ranged = Object.fromEntries(Object.keys(versions).map((who) => [who, '^0.9.0']));
    expect(() => agreed(ranged)).toThrow(/move only together/);
  });
});

describe("the editor's source", () => {
  it('differs from what rhwp has at the tag only in the files the package names', () => {
    const named = namedAsPatched();
    expect(named.length).toBeGreaterThan(0);
    expect(changedFromUpstream()).toEqual(named);
  });

  it('is listed in the notices under rhwp’s licence when it is in the bundle', () => {
    expect(rhwpStudioLicenses(['/w/apps/web/src/main.ts'])).toEqual([]);
    const module = path.join(studioDirectory(), 'upstream/rhwp-studio/src/main.ts');
    const [item] = rhwpStudioLicenses([module]);
    expect(item?.identifier).toBe('MIT');
    expect(readAllowedLicenses()).toContain(item?.identifier);
    expect(item?.name).toContain('github.com/edwardkim/rhwp');
    expect(item?.version).toBe(studioVersion());
    expect(item?.text).toContain('MIT License');
  });
});

describe('rhwp’s import of PDF, which this editor does not have', () => {
  const upstream = path.join(studioDirectory(), 'upstream');
  const sources = (folder: string): string[] =>
    readdirSync(folder, { withFileTypes: true }).flatMap((entry) => {
      const at = path.join(folder, entry.name);
      if (entry.isDirectory()) return sources(at);
      return /\.(ts|js)$/.test(entry.name) ? [at] : [];
    });

  it('is not in the repository: neither its session nor its worker, and nothing imports them', () => {
    expect(existsSync(path.join(upstream, 'rhwp-shared/pdf'))).toBe(false);
    const importing = sources(upstream).filter((file) =>
      /from\s+['"][^'"]*rhwp-shared\/pdf\//.test(
        readFileSync(file, 'utf8').replace(/^import type .*$/gm, ''),
      ),
    );
    expect(importing).toEqual([]);
  });

  it('is refused by the one function the editor’s source reaches it through', async () => {
    const opening = path.join(upstream, 'rhwp-studio/src/core/pdf-open.ts');
    const { preparePdfForEditor } = (await import(opening)) as {
      preparePdfForEditor: (...given: unknown[]) => Promise<unknown>;
    };
    await expect(preparePdfForEditor()).rejects.toMatchObject({ code: 'PDF_IMPORT_DISABLED' });
  });

  it('is taken off the engine before the editor has it', async () => {
    const engine = await import('@rhwp/core');
    expect(typeof (engine.HwpDocument as unknown as Record<string, unknown>).importPdf).toBe(
      'function',
    );
    const given = await import('../../src/editor-frame/rhwp.ts');
    for (const name of given.PDF_IMPORT_STATICS) {
      expect(name in given.HwpDocument, name).toBe(false);
    }
    expect(given.PdfEditorReopen).toBeUndefined();
    expect(given.PdfImportRequest).toBeUndefined();
    // Nothing else the editor is handed names the import.
    const named = Object.keys(given).filter((name) => /pdf/i.test(name));
    expect(named.sort()).toEqual(['PDF_IMPORT_STATICS', 'PdfEditorReopen', 'PdfImportRequest']);
  });
});

describe('the import of the editor’s source', () => {
  const folders: string[] = [];
  afterEach(() => {
    for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true });
  });

  /** A checkout with what the import brings, each file a line of its own. */
  function checkout(more: Record<string, string> = {}): string {
    const root = mkdtempSync(path.join(tmpdir(), 'rhwp-import-'));
    folders.push(root);
    const files: Record<string, string> = {
      LICENSE: 'MIT License',
      'rhwp-studio/index.html': '<html></html>',
      'rhwp-studio/public/theme-init.js': '',
      'rhwp-studio/public/locale-init.js': '',
      'rhwp-shared/forms/action-runtime.ts': '',
      'assets/fonts/enclosed-pua-metrics.json': '{}',
      'rhwp-studio/src/main.ts': '',
      'rhwp-studio/src/style.css': '',
      'rhwp-studio/src/main.test.ts': '',
      'rhwp-studio/src/NOTES.md': '',
      ...more,
    };
    for (const [file, text] of Object.entries(files)) {
      mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
      writeFileSync(path.join(root, file), text);
    }
    return root;
  }
  const NAMED = { origin: 'rhwp’s own data', license: 'MIT' };
  const ASSETS = { 'assets/fonts/enclosed-pua-metrics.json': NAMED };

  it('takes code by its kind and the assets it was told of, and leaves tests and documents', () => {
    expect(chosen(checkout(), ASSETS)).toEqual([
      'LICENSE',
      'assets/fonts/enclosed-pua-metrics.json',
      'rhwp-shared/forms/action-runtime.ts',
      'rhwp-studio/index.html',
      'rhwp-studio/public/locale-init.js',
      'rhwp-studio/public/theme-init.js',
      'rhwp-studio/src/main.ts',
      'rhwp-studio/src/style.css',
    ]);
  });

  it.each([
    ['a font', 'rhwp-studio/src/fonts/Some.woff2'],
    ['a picture', 'rhwp-studio/src/images/icons.svg'],
    ['data', 'rhwp-studio/src/core/table.json'],
    ['WebAssembly', 'rhwp-studio/src/engine/extra.wasm'],
    ['a file of no kind', 'rhwp-studio/src/core/NOTICE'],
  ])('stops on %s nobody named', (_, file) => {
    expect(() => chosen(checkout({ [file]: 'x' }), ASSETS)).toThrow(
      new RegExp(`assets\\.json does not name[\\s\\S]*${file.replaceAll('.', '\\.')}`),
    );
    // Named with its origin and licence, it is brought; named without either, it is not.
    expect(chosen(checkout({ [file]: 'x' }), { ...ASSETS, [file]: NAMED })).toContain(file);
    for (const bare of [
      {},
      { origin: 'somewhere' },
      { license: 'MIT' },
      { ...NAMED, license: ' ' },
    ]) {
      expect(() => chosen(checkout({ [file]: 'x' }), { ...ASSETS, [file]: bare })).toThrow(
        /without their origin or licence/,
      );
    }
  });

  it('stops when an asset it was told of is not there, and when the list of assets is not used', () => {
    expect(() => chosen(checkout(), {})).toThrow(/does not name/);
    expect(() => chosen(checkout(), { ...ASSETS, 'assets/fonts/gone.json': NAMED })).toThrow(
      /names files that are not brought: assets\/fonts\/gone\.json/,
    );
  });
});

describe('a symbolic link in rhwp’s source', () => {
  it('is not followed: the import ends there', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'rhwp-import-'));
    try {
      mkdirSync(path.join(root, 'rhwp-studio/src'), { recursive: true });
      symlinkSync('/etc/hostname', path.join(root, 'rhwp-studio/src/link.ts'));
      // The import ends its process on a link; here it is asked in one of its own.
      const { spawnSync } = await import('node:child_process');
      const script = `import(${JSON.stringify(importer)}).then((m) => m.chosen(${JSON.stringify(root)}, {}))`;
      const ended = spawnSync(process.execPath, ['-e', script], { encoding: 'utf8' });
      expect(ended.status).toBe(1);
      expect(ended.stderr).toContain('rhwp-studio/src/link.ts is a symbolic link');
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
