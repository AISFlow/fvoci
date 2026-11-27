/**
 * Build steps that shape the output directory for the server.
 *
 * The server sends `Cache-Control: immutable` for a file under `static/` whose
 * name carries a content hash, and it sends `<file>.br`, `<file>.zst` or
 * `<file>.gz` instead of `<file>` when one lies next to it and the client
 * accepts the encoding. `checkHashedNames` fails the build when a file would
 * miss the first rule, `precompress` writes the files for the second.
 */
import { existsSync } from 'node:fs';
import { readdir, readFile, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { brotliCompress, constants, gzip, zstdCompress } from 'node:zlib';

import type { Plugin } from 'vite';

import { ENCODINGS, type Encoding } from './encodings.ts';

const brotli = promisify(brotliCompress);
const gzipAsync = promisify(gzip);
const zstd = promisify(zstdCompress);

/**
 * The level the `.zst` files are written at: 19, the highest of the regular
 * levels.
 *
 * The files are compressed once, at build time, so time does not decide (all
 * text files of a build take about 0.1 s at this level). Above 19 are the
 * levels zstd calls "ultra". They gain nothing on files this small (the
 * totals at 19, 20 and 22 differ by four bytes), and for a large file they
 * ask for a window the `zstd` content coding does not allow
 * (`MAX_ZSTD_WINDOW`): measured on 17 MB of input, level 19 asks for 8 MB,
 * levels 20 to 22 for all 17.
 */
export const ZSTD_LEVEL = 19;

/**
 * The largest window a frame sent with `Content-Encoding: zstd` may ask a
 * client for: 8 MB (RFC 9659, section 3). A browser refuses a larger one.
 */
export const MAX_ZSTD_WINDOW = 8 * 1024 * 1024;

/**
 * The server's test for "this name contains a content hash": some part
 * between `-`, `.` and `_` is at least eight hexadecimal digits long and has
 * a decimal digit in it.
 */
export function hasContentHash(fileName: string): boolean {
  return fileName
    .split(/[-._]/)
    .some((part) => part.length >= 8 && /^[0-9a-fA-F]+$/.test(part) && /[0-9]/.test(part));
}

/** Files that are meant to be revalidated on every request, by name: `favicon.svg` is linked by its name. */
const UNHASHED_ON_PURPOSE = new Set([
  'THIRD_PARTY_NOTICES.txt',
  'favicon.svg',
  'viewer-frame.html',
  'editor-frame.html',
]);

/** Already compressed formats; compressing them again gains nothing. */
const COMPRESSIBLE = new Set([
  '.wasm',
  '.css',
  '.html',
  '.js',
  '.json',
  '.map',
  '.mjs',
  '.svg',
  '.txt',
  '.webmanifest',
  '.xml',
]);

const COMPRESSED = new Set<string>(ENCODINGS.map((encoding) => encoding.extension));

/** Whether a file is the compressed form of the file next to it. */
const isCompressedForm = (file: string): boolean => COMPRESSED.has(path.extname(file));

async function walk(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files: string[] = [];
  for (const entry of entries) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(full)));
    else if (entry.isFile()) files.push(full);
  }
  return files.sort();
}

function outputDirectory(plugin: {
  environment: { config: { root: string; build: { outDir: string } } };
}): string {
  const { root, build } = plugin.environment.config;
  return path.resolve(root, build.outDir);
}

/** Fails the build when a file under `static/` has no content hash in its name. */
export function checkHashedNames(): Plugin {
  return {
    name: 'fvoci:check-hashed-names',
    apply: 'build',
    async closeBundle() {
      const staticDirectory = path.join(outputDirectory(this), 'static');
      // This hook also runs after a build that stopped before it wrote
      // anything. Failing here then would hide the error that stopped it.
      if (!existsSync(staticDirectory)) return;
      const unhashed = (await walk(staticDirectory))
        .map((file) => path.relative(staticDirectory, file))
        .filter((file) => !isCompressedForm(file))
        .filter((file) => !UNHASHED_ON_PURPOSE.has(file) && !hasContentHash(path.basename(file)));
      if (unhashed.length > 0) {
        this.error(
          `files under static/ without a content hash in the name (the server would not cache them):\n  ${unhashed.join('\n  ')}`,
        );
      }
    },
  };
}

/**
 * The window a zstd frame asks its reader to hold, in bytes, read from the
 * frame header (RFC 8878, section 3.1.1.1). A frame that says how long its
 * content is and fits it in one segment needs exactly that much.
 */
export function zstdWindowSize(frame: Uint8Array): number {
  const view = Buffer.from(frame.buffer, frame.byteOffset, frame.byteLength);
  if (view.length < 6 || view.readUInt32LE(0) !== 0xfd2fb528) throw new Error('not a zstd frame');
  const descriptor = view.readUInt8(4);
  const singleSegment = (descriptor & 0x20) !== 0;
  if (!singleSegment) {
    const window = view.readUInt8(5);
    const base = 2 ** (10 + (window >> 3));
    return base + (base / 8) * (window & 7);
  }
  // No window descriptor: the content size follows the dictionary id.
  const at = 5 + ([0, 1, 2, 4][descriptor & 3] ?? 0);
  switch (descriptor >> 6) {
    case 0:
      return view.readUInt8(at);
    case 1:
      return view.readUInt16LE(at) + 256;
    case 2:
      return view.readUInt32LE(at);
    default:
      return Number(view.readBigUInt64LE(at));
  }
}

/** The compressed forms of one file's bytes, by the extension each is written with. */
export async function compressedForms(
  source: Buffer,
): Promise<Record<Encoding['extension'], Buffer>> {
  const [br, zst, gz] = await Promise.all([
    brotli(source, {
      params: {
        [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
        [constants.BROTLI_PARAM_SIZE_HINT]: source.length,
      },
    }),
    // The pledged size puts the length of the content into the frame header
    // and keeps the window no larger than the content.
    zstd(source, {
      params: { [constants.ZSTD_c_compressionLevel]: ZSTD_LEVEL },
      pledgedSrcSize: source.length,
    }),
    gzipAsync(source, { level: constants.Z_BEST_COMPRESSION }),
  ]);
  if (zstdWindowSize(zst) > MAX_ZSTD_WINDOW) {
    throw new Error(
      `the zstd frame asks for a window of ${zstdWindowSize(zst)} bytes; a browser accepts ${MAX_ZSTD_WINDOW} at most`,
    );
  }
  return { '.br': br, '.zst': zst, '.gz': gz };
}

/**
 * Writes `<file>.br`, `<file>.zst` and `<file>.gz` next to every compressible
 * file under a directory, each unless the compressed form is not smaller.
 * Returns the paths written.
 */
export async function precompressDirectory(directory: string): Promise<string[]> {
  const written: string[] = [];
  for (const file of await walk(directory)) {
    if (!COMPRESSIBLE.has(path.extname(file))) continue;
    const source = await readFile(file);
    const forms = await compressedForms(source);
    for (const { extension } of ENCODINGS) {
      if (forms[extension].length >= source.length) continue;
      await writeFile(file + extension, forms[extension]);
      written.push(file + extension);
    }
  }
  return written;
}

/**
 * Writes the compressed forms of the files under `static/` (see
 * `precompressDirectory`).
 *
 * `index.html` is left alone: the server fills the base URL into it on every
 * request and never sends a file from disk for it.
 */
export function precompress(): Plugin {
  return {
    name: 'fvoci:precompress',
    apply: 'build',
    async closeBundle() {
      const started = performance.now();
      const staticDirectory = path.join(outputDirectory(this), 'static');
      // As above: nothing to do after a build that wrote nothing.
      if (!existsSync(staticDirectory)) return;
      const written = await precompressDirectory(staticDirectory);
      const extensions = ENCODINGS.map((encoding) => encoding.extension).join('/');
      this.info(
        `precompress: wrote ${written.length} ${extensions} files in ${Math.round(performance.now() - started)} ms`,
      );
    },
  };
}

export interface FileSizes {
  /** Path under the directory. */
  file: string;
  bytes: number;
  /** The size of each compressed form that lies next to the file. */
  compressed: Partial<Record<Encoding['name'], number>>;
}

/** The sizes of the files of a build and of their compressed forms, for the report. */
export async function outputSizes(directory: string): Promise<FileSizes[]> {
  const files = await walk(directory);
  const sizes: FileSizes[] = [];
  for (const file of files.filter((candidate) => !isCompressedForm(candidate))) {
    const compressed: FileSizes['compressed'] = {};
    for (const { name, extension } of ENCODINGS) {
      if (files.includes(file + extension)) compressed[name] = (await stat(file + extension)).size;
    }
    sizes.push({
      file: path.relative(directory, file),
      bytes: (await stat(file)).size,
      compressed,
    });
  }
  return sizes;
}
