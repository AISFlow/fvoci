/**
 * What the HWP renderer's worker does (hwp-worker.ts), apart from the
 * worker so that it can be tried in Node: checks a file, opens it with rhwp,
 * and draws its pages one at a time; for the editor's frame, it also counts
 * the controls of the file opened, and the pages it lays out on once saved.
 *
 * Before rhwp reads a file it is checked: an archive (HWPX) is unpacked,
 * counted and written anew (zip-guard.ts), and what rhwp parses of it as XML
 * held to the limit (hwpx-package.ts); a compound file (HWP) has its chains
 * and the sizes its streams say checked (compound-guard.ts). rhwp lays the
 * document out with font metrics of its own (it has no `document` in a
 * worker to measure text with), so the layout does not depend on the fonts
 * the reader has; only the glyphs drawn do.
 */
import init, { HwpDocument } from '@rhwp/core';

import { UNPACKED_LIMIT_BYTES } from '$lib/file-viewer/formats.ts';
import { PAGE_LIMIT } from '$lib/file-viewer/protocol.ts';

import { checkCompound } from './compound-guard.ts';
import { tallyControls } from './hwp-controls.ts';
import { checkHwpxPackage } from './hwpx-package.ts';
import type { FromWorker, HwpFormat, PageSize, TextRun, ToWorker } from './hwp-messages.ts';
import { LockedFile, TooLargeFile, UnreadableFile } from './renderer.ts';
import { repack } from './zip-guard.ts';

/**
 * The most text a page's drawing (SVG) may be, in characters. rhwp writes each
 * picture into the page as text, as often as the page shows it; this much is
 * a page with the largest picture a file the viewer takes can hold. Past it,
 * or past `PAGE_ELEMENT_LIMIT`, the page is not handed to the frame, whose
 * thread is the app's: the frame says in its place that it is too large to
 * show, and draws the others.
 */
export const PAGE_SVG_LIMIT = 80 * 1024 * 1024;

/**
 * The most elements a page's drawing and runs of text may be, together. rhwp
 * draws each character apart, and the frame's thread copies and lays out
 * each; a page this full takes it about a second, and a page of a document
 * written in small print about a third of it.
 */
export const PAGE_ELEMENT_LIMIT = 20_000;

/** How many elements an SVG's text opens: each `<` followed by a letter. */
function elementsIn(svg: string): number {
  let count = 0;
  for (let at = svg.indexOf('<'); at >= 0; at = svg.indexOf('<', at + 1)) {
    const next = svg.charCodeAt(at + 1) | 0x20;
    if (next >= 0x61 && next <= 0x7a) count += 1;
  }
  return count;
}

/** What rhwp says of a file it cannot read, and of one locked with a password (0.9.0). */
const INVALID_FILE = '유효하지 않은 파일';
const NEEDS_PASSWORD = '비밀번호가 필요한';

/** Why rhwp did not open a file, from what it says. */
function openFailure(error: unknown): Error {
  if (error instanceof WebAssembly.RuntimeError) return error;
  const said = error instanceof Error ? error.message : String(error);
  if (said.includes(NEEDS_PASSWORD)) return new LockedFile();
  return said.startsWith(INVALID_FILE) ? new UnreadableFile() : new Error(said);
}

const isSize = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value) && value > 0;

function pageSize(document: HwpDocument, index: number): PageSize {
  const info: unknown = JSON.parse(document.getPageInfo(index));
  const { width, height } = (info ?? {}) as { width?: unknown; height?: unknown };
  if (!isSize(width) || !isSize(height)) throw new UnreadableFile();
  return { width, height };
}

/** A page's runs of text, as rhwp lays them out (`getPageTextLayout`). */
function textOf(document: HwpDocument, index: number): TextRun[] {
  const layout: unknown = JSON.parse(document.getPageTextLayout(index));
  const runs = (layout as { runs?: unknown } | null)?.runs;
  if (!Array.isArray(runs)) return [];
  return runs.flatMap((run: Record<string, unknown>) => {
    const { text, x, y, w, h, fontSize } = run;
    const numbers = [x, y, w, h, fontSize];
    if (
      typeof text !== 'string' ||
      !numbers.every((n) => typeof n === 'number' && Number.isFinite(n))
    ) {
      return [];
    }
    const [left, top, width, height, size] = numbers as number[];
    return [{ text, x: left, y: top, width, height, size }] as TextRun[];
  });
}

/** The file as rhwp may read it, once checked as its kind is. */
async function checked(
  message: Extract<ToWorker, { kind: 'open' | 'check' }>,
  unpackLimit: number,
): Promise<ArrayBuffer> {
  if (message.format === 'hwp') return checkCompound(message.bytes, unpackLimit);
  const archive = await repack(message.bytes);
  await checkHwpxPackage(archive);
  return archive;
}

/** What the reader holds a file and its pages to; the constants above unless given. */
export interface ReaderLimits {
  /** The most text a page's drawing may be (`PAGE_SVG_LIMIT`). */
  svg?: number;
  /** The most elements a page's drawing and text may be (`PAGE_ELEMENT_LIMIT`). */
  elements?: number;
  /** What an HWP's streams may unpack to (`UNPACKED_LIMIT_BYTES`). */
  unpacked?: number;
}

/** The worker's document: opened once, then drawn a page at a time. */
export class HwpReader {
  #document: HwpDocument | undefined;
  #format: HwpFormat = 'hwp';
  readonly #limits: Required<ReaderLimits>;

  constructor(limits: ReaderLimits = {}) {
    // A limit given as `undefined` is the constant, not none.
    this.#limits = {
      svg: limits.svg ?? PAGE_SVG_LIMIT,
      elements: limits.elements ?? PAGE_ELEMENT_LIMIT,
      unpacked: limits.unpacked ?? UNPACKED_LIMIT_BYTES,
    };
  }

  async open(message: Extract<ToWorker, { kind: 'open' }>): Promise<FromWorker> {
    if (this.#document) throw new Error('a document is open already');
    const file = await checked(message, this.#limits.unpacked);
    await init({ module_or_path: message.wasm });
    let document: HwpDocument;
    try {
      document = new HwpDocument(new Uint8Array(file));
    } catch (error) {
      throw openFailure(error);
    }
    this.#document = document;
    this.#format = message.format;
    const count = document.pageCount();
    if (!Number.isInteger(count) || count < 1 || count > PAGE_LIMIT) throw new UnreadableFile();
    const pages = Array.from({ length: count }, (_, index) => pageSize(document, index));
    return { kind: 'opened', pages };
  }

  /** The file checked, for the editor to read: rhwp is not started here. */
  async check(message: Extract<ToWorker, { kind: 'check' }>): Promise<FromWorker> {
    return { kind: 'checked', bytes: await checked(message, this.#limits.unpacked) };
  }

  /** How many controls of each kind the document opened holds. */
  controls(): FromWorker {
    const document = this.#document;
    if (!document) throw new Error('no document is open');
    return { kind: 'controls', controls: tallyControls(JSON.parse(document.getControls())) };
  }

  /**
   * The pages the document opened lays out on, and the pages its saved file
   * lays out on, saved as the editor saves it (rhwp's export) and read back
   * as `open` reads a file: checked, then opened. The saved file takes the
   * document's place, which is let go of first: what is asked after is asked
   * of the saved file.
   */
  async resaved(): Promise<FromWorker> {
    const document = this.#document;
    if (!document) throw new Error('no document is open');
    const pages = document.pageCount();
    const exported =
      this.#format === 'hwp' ? document.exportHwpWithReport() : document.exportHwpxWithReport();
    let bytes: Uint8Array;
    try {
      bytes = exported.takeBytes();
    } finally {
      exported.free();
    }
    this.#document = undefined;
    document.free();
    const copy = new ArrayBuffer(bytes.byteLength);
    new Uint8Array(copy).set(bytes);
    const file = await checked(
      { kind: 'check', format: this.#format, bytes: copy },
      this.#limits.unpacked,
    );
    try {
      this.#document = new HwpDocument(new Uint8Array(file));
    } catch (error) {
      throw openFailure(error);
    }
    const saved = this.#document.pageCount();
    // As `open` finds it: a file on no page is not one it reads.
    if (saved < 1) throw new UnreadableFile();
    return { kind: 'resaved', pages, saved };
  }

  /** A page (from 1), drawn. */
  page(number: number): FromWorker {
    const document = this.#document;
    if (!document || !Number.isInteger(number) || number < 1 || number > document.pageCount()) {
      throw new Error('no such page');
    }
    const index = number - 1;
    const svg = document.renderPageSvg(index);
    if (svg.length > this.#limits.svg) throw new TooLargeFile();
    const text = textOf(document, index);
    if (elementsIn(svg) + text.length > this.#limits.elements) throw new TooLargeFile();
    return { kind: 'page', svg, text };
  }

  /**
   * The answer to a message: what was asked, or why it could not be done.
   * Rejects when rhwp trapped (`WebAssembly.RuntimeError`, as when it runs out
   * of the memory it may have): it is left as it was mid-way, and nothing more
   * can be asked of it.
   */
  async answer(message: ToWorker): Promise<FromWorker> {
    try {
      switch (message.kind) {
        case 'open':
          return await this.open(message);
        case 'check':
          return await this.check(message);
        case 'controls':
          return this.controls();
        case 'resaved':
          return await this.resaved();
        case 'page':
          return this.page(message.page);
      }
    } catch (error) {
      if (error instanceof WebAssembly.RuntimeError) throw error;
      return { kind: 'failed', reason: failureOf(error) };
    }
  }
}

function failureOf(error: unknown): Extract<FromWorker, { kind: 'failed' }>['reason'] {
  if (error instanceof LockedFile) return 'locked';
  if (error instanceof TooLargeFile) return 'too-large';
  return error instanceof UnreadableFile ? 'unreadable' : 'renderer';
}
