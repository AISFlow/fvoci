/**
 * The build's compressed files and the choice between them.
 */
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { brotliDecompressSync, constants, gunzipSync, zstdDecompressSync } from 'node:zlib';

import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { acceptsEncoding, chooseEncoding, ENCODINGS } from '../../tools/build/encodings.ts';
import {
  compressedForms,
  MAX_ZSTD_WINDOW,
  outputSizes,
  precompressDirectory,
  ZSTD_LEVEL,
  zstdWindowSize,
} from '../../tools/build/output.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

/** Text that compresses: a style sheet's worth of rules that repeat. */
const sheet = Array.from(
  { length: 400 },
  (_, index) => `.rule-${index} { color: var(--tone-${index % 7}); margin: ${index % 5}px; }`,
).join('\n');

describe('the compressed forms of a build', () => {
  let root = '';
  let written: string[] = [];

  beforeAll(async () => {
    const cache = path.join(WEB_ROOT, 'node_modules/.cache');
    mkdirSync(cache, { recursive: true });
    root = mkdtempSync(path.join(cache, 'build-precompress-'));
    mkdirSync(path.join(root, 'deeper'));
    writeFileSync(path.join(root, 'sheet-0123456789abcdef.css'), sheet);
    writeFileSync(path.join(root, 'deeper/script-0123456789abcdef.js'), `// ${sheet}`);
    writeFileSync(path.join(root, 'tiny.txt'), 'a');
    writeFileSync(path.join(root, 'font-0123456789abcdef.woff2'), sheet);
    written = await precompressDirectory(root);
  });

  afterAll(() => {
    if (root !== '') rmSync(root, { recursive: true, force: true });
  });

  it('are three files next to each text file', () => {
    for (const file of ['sheet-0123456789abcdef.css', 'deeper/script-0123456789abcdef.js']) {
      for (const extension of ['.br', '.zst', '.gz']) {
        expect(existsSync(path.join(root, file + extension)), file + extension).toBe(true);
      }
    }
    expect(written).toHaveLength(6);
  });

  it('give back the file, byte for byte, when read with each decoder', () => {
    for (const file of ['sheet-0123456789abcdef.css', 'deeper/script-0123456789abcdef.js']) {
      const source = readFileSync(path.join(root, file));
      const read = (extension: string): Buffer => readFileSync(path.join(root, file + extension));
      expect(brotliDecompressSync(read('.br')).equals(source), `${file}.br`).toBe(true);
      expect(zstdDecompressSync(read('.zst')).equals(source), `${file}.zst`).toBe(true);
      expect(gunzipSync(read('.gz')).equals(source), `${file}.gz`).toBe(true);
      for (const extension of ['.br', '.zst', '.gz']) {
        expect(read(extension).length, file + extension).toBeLessThan(source.length);
      }
    }
  });

  it('are not written where they would not be smaller, nor for a format that is compressed already', () => {
    for (const extension of ['.br', '.zst', '.gz']) {
      expect(existsSync(path.join(root, `tiny.txt${extension}`))).toBe(false);
      expect(existsSync(path.join(root, `font-0123456789abcdef.woff2${extension}`))).toBe(false);
    }
  });

  it('are the same bytes when the build runs again', async () => {
    const before = written.map((file) => readFileSync(file));
    expect(await precompressDirectory(root)).toEqual(written);
    expect(written.map((file) => readFileSync(file))).toEqual(before);
  });

  it('are listed with their sizes, each file once', async () => {
    const sizes = await outputSizes(root);
    expect(sizes.map(({ file }) => file)).toEqual([
      'deeper/script-0123456789abcdef.js',
      'font-0123456789abcdef.woff2',
      'sheet-0123456789abcdef.css',
      'tiny.txt',
    ]);
    const css = sizes.find(({ file }) => file.endsWith('.css'));
    expect(css?.bytes).toBe(Buffer.byteLength(sheet));
    expect(Object.keys(css?.compressed ?? {}).sort()).toEqual(['br', 'gzip', 'zstd']);
    expect(sizes.find(({ file }) => file === 'tiny.txt')?.compressed).toEqual({});
  });
});

describe('a zstd frame of the build', () => {
  it('says how long its content is and asks for no more memory than that', async () => {
    const source = Buffer.from(sheet);
    const { '.zst': frame } = await compressedForms(source);
    // Bit 5 of the descriptor: the content is one segment, as long as the header says.
    expect(frame.readUInt8(4) & 0x20).toBe(0x20);
    expect(zstdWindowSize(frame)).toBe(source.length);
    expect(zstdWindowSize(frame)).toBeLessThanOrEqual(MAX_ZSTD_WINDOW);
  });

  it('is written at the highest level that is not an "ultra" one', () => {
    expect(ZSTD_LEVEL).toBe(19);
    expect(constants.ZSTD_c_compressionLevel).toBeTypeOf('number');
  });

  it('has its window read from the header, with and without a window descriptor', () => {
    const magic = [0x28, 0xb5, 0x2f, 0xfd];
    // One segment, content size in one byte: 200 bytes.
    expect(zstdWindowSize(Uint8Array.from([...magic, 0x20, 200]))).toBe(200);
    // One segment, content size in two bytes, stored less 256: 1000 bytes.
    expect(zstdWindowSize(Uint8Array.from([...magic, 0x60, 0xe8, 0x02]))).toBe(1000);
    // A window descriptor: exponent 13 (8 MB), mantissa 0.
    expect(zstdWindowSize(Uint8Array.from([...magic, 0x00, 13 << 3]))).toBe(MAX_ZSTD_WINDOW);
    // Exponent 13, mantissa 1: an eighth more than 8 MB.
    expect(zstdWindowSize(Uint8Array.from([...magic, 0x00, (13 << 3) | 1]))).toBe(
      MAX_ZSTD_WINDOW * 1.125,
    );
    expect(() => zstdWindowSize(Uint8Array.from([0x1f, 0x8b, 8, 0, 0, 0]))).toThrow(/not a zstd/);
  });
});

describe('the encoding a client is sent', () => {
  const all = (): boolean => true;

  it('is one the header accepts', () => {
    expect(acceptsEncoding('gzip, deflate, br, zstd', 'zstd')).toBe(true);
    expect(acceptsEncoding('gzip, deflate', 'zstd')).toBe(false);
    expect(acceptsEncoding(undefined, 'gzip')).toBe(false);
    expect(acceptsEncoding('', 'gzip')).toBe(false);
    expect(acceptsEncoding('GZIP', 'gzip')).toBe(true);
    expect(acceptsEncoding(['gzip', 'zstd'], 'zstd')).toBe(true);
    // `br` is not a part of another name.
    expect(acceptsEncoding('brotli-x, x-br', 'br')).toBe(false);
  });

  it('is not one the header refuses with a weight of zero', () => {
    expect(acceptsEncoding('zstd;q=0, gzip', 'zstd')).toBe(false);
    expect(acceptsEncoding('zstd; q=0.0, gzip', 'zstd')).toBe(false);
    expect(acceptsEncoding('zstd;q=0.5', 'zstd')).toBe(true);
    expect(acceptsEncoding('*', 'zstd')).toBe(true);
    expect(acceptsEncoding('*;q=0', 'zstd')).toBe(false);
    expect(acceptsEncoding('*, zstd;q=0', 'zstd')).toBe(false);
    expect(acceptsEncoding('*;q=0, zstd', 'zstd')).toBe(true);
  });

  it('is zstd for a client that accepts zstd and not brotli', () => {
    expect(chooseEncoding('gzip, zstd', all)).toEqual({ name: 'zstd', extension: '.zst' });
    expect(chooseEncoding('zstd', all)?.extension).toBe('.zst');
  });

  it('is brotli before zstd, and zstd before gzip, when the client accepts several', () => {
    expect(ENCODINGS.map((encoding) => encoding.name)).toEqual(['br', 'zstd', 'gzip']);
    expect(chooseEncoding('gzip, deflate, br, zstd', all)?.name).toBe('br');
    expect(chooseEncoding('gzip, deflate, br;q=0, zstd', all)?.name).toBe('zstd');
    expect(chooseEncoding('gzip, deflate', all)?.name).toBe('gzip');
  });

  it('is only one the build wrote a file for', () => {
    const only =
      (...extensions: string[]) =>
      (extension: string): boolean =>
        extensions.includes(extension);
    expect(chooseEncoding('gzip, br, zstd', only('.zst', '.gz'))?.name).toBe('zstd');
    expect(chooseEncoding('gzip, br, zstd', only('.gz'))?.name).toBe('gzip');
    expect(chooseEncoding('gzip, br, zstd', only())).toBeUndefined();
    expect(chooseEncoding('identity', all)).toBeUndefined();
    expect(chooseEncoding(undefined, all)).toBeUndefined();
  });
});
