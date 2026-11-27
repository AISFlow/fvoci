/**
 * Draws a Word document with docx-preview into the worker's own DOM
 * (`worker-xml.ts`, `worker-html.ts`) and hands back the steps that build
 * what was drawn: what the Word worker (`docx-worker.ts`) does with a file.
 *
 * docx-preview reads `document`, `Node`, `DOMParser` and `URL.createObjectURL`
 * from the global object; the worker puts its own there before this runs
 * (`worker-scope.ts`). Before drawing, the style sheets docx-preview would
 * write are reckoned (`docx-styles.ts`): they are text, not nodes, and no
 * node count sees them. Pictures come back as blobs, each named in the steps
 * by a key that starts with the frame's token, one key for each picture
 * however often it is shown: the frame makes their addresses (an address
 * made in the worker would end with it).
 */
import { parseAsync, renderDocument } from 'docx-preview';

import { OPTIONS } from './docx-options.ts';

import type { DocxFailure, FromDocxWorker } from './docx-protocol.ts';
import { STYLE_TEXT_LIMIT, styleText } from './docx-styles.ts';
import { Element, flatten, STEPS_TEXT_LIMIT, type Budget, type Node } from './worker-html.ts';
import type { Drawing } from './worker-scope.ts';
import { TooMuch } from './worker-xml.ts';

/** What docx-preview's package is to this: how it finds a part, and unpacks one. */
interface Package {
  get(path: string): unknown;
  load(path: string, type?: string): Promise<unknown>;
}

function packageOf(document: unknown): Package | undefined {
  if (typeof document !== 'object' || document === null || !('_package' in document)) {
    return undefined;
  }
  const found: unknown = document._package;
  if (typeof found !== 'object' || found === null) return undefined;
  return 'get' in found &&
    'load' in found &&
    typeof found.get === 'function' &&
    typeof found.load === 'function'
    ? (found as Package)
    : undefined;
}

/** The kinds of output an unpacked part is kept as: ones nothing changes after. */
const KEPT = new Set(['string', 'blob']);

/**
 * Makes the package unpack each part once for each kind of output, however
 * often the document refers to it (a picture shown in many places). A part
 * is known by the archive's own entry for it.
 */
function unpackOnce(pack: Package): void {
  const load = pack.load.bind(pack);
  const unpacked = new Map<unknown, Map<string, Promise<unknown>>>();
  pack.load = (path, type = 'string') => {
    const entry = pack.get(path);
    if (entry === undefined || !KEPT.has(type)) return load(path, type);
    const kinds = unpacked.get(entry) ?? new Map<string, Promise<unknown>>();
    unpacked.set(entry, kinds);
    const known = kinds.get(type);
    if (known) return known;
    const loading = load(path, type);
    kinds.set(type, loading);
    return loading;
  };
}

/**
 * Makes the document give one address to a picture however often it is
 * shown: docx-preview asks for an address at each place.
 */
function addressOncePerPart(document: object): void {
  const holder = document as { blobToURL?: unknown };
  if (typeof holder.blobToURL !== 'function') return;
  const original = (holder.blobToURL as (blob: unknown, path?: string) => unknown).bind(document);
  const made = new Map<string, unknown>();
  holder.blobToURL = (blob: unknown, path?: string) => {
    if (typeof path !== 'string') return original(blob, path);
    if (!made.has(path)) made.set(path, original(blob, path));
    return made.get(path);
  };
}

/** Why drawing stopped, from what was thrown: too much, or a file the library could not read. */
function failure(error: unknown, budget: Budget): FromDocxWorker {
  // A string or a call stack past the engine's limits is a document too large too.
  const tooMuch = error instanceof TooMuch || error instanceof RangeError || budget.passed;
  const reason: DocxFailure = tooMuch ? 'too-large' : 'unreadable';
  return { kind: 'failed', reason };
}

/** Draws the file and flattens what was drawn: the style sheets apart from the pages. */
export async function draw(
  bytes: ArrayBuffer,
  { budget, images }: Drawing,
): Promise<FromDocxWorker> {
  let parsed: unknown;
  try {
    parsed = await parseAsync(bytes, OPTIONS);
  } catch (error) {
    return failure(error, budget);
  }
  const pack = packageOf(parsed);
  // A version of the library that keeps its package otherwise fails here (as the renderer's
  // failure) rather than draws without the guard.
  if (!pack || typeof parsed !== 'object' || parsed === null) {
    return { kind: 'failed', reason: 'renderer' };
  }
  unpackOnce(pack);
  addressOncePerPart(parsed);
  if (styleText(parsed) > STYLE_TEXT_LIMIT) return { kind: 'failed', reason: 'too-large' };
  try {
    const nodes = (await renderDocument(parsed, OPTIONS)) as unknown as Node[];
    // A ceiling passed where the library caught what was thrown.
    if (budget.passed) throw new TooMuch('elements');
    const isStyle = (node: Node): boolean => node instanceof Element && node.tagName === 'style';
    const room = { left: STEPS_TEXT_LIMIT };
    const styles = flatten(nodes.filter(isStyle), room);
    const body = flatten(
      nodes.filter((node) => !isStyle(node)),
      room,
    );
    return { kind: 'drawn', styles, body, images };
  } catch (error) {
    return failure(error, budget);
  }
}
