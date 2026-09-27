import { Unzip, UnzipInflate, zipSync, type Unzipped } from "fflate";

/** Largest PPTX the browser viewer downloads; bigger files stay download-only. */
export const PPTX_MAX_BYTES = 64 * 1024 * 1024;

/** Total inflated size of every package entry, checked before the renderer runs. */
export const PPTX_MAX_EXPANDED_BYTES = 128 * 1024 * 1024;

/**
 * Inflated size of the entries the loader could parse as XML (see
 * `startsLikeMarkup`; SVG pictures count too), within the total above.
 * The loader's element tree costs about 31× the XML it parses (independent
 * review, Node 22: 16 MiB of slide XML retained ≈ 0.5 GiB of heap), so markup
 * has a much smaller budget than pictures and media, which stay as bytes.
 * This bounds the parsed package, not the rendered slides (see
 * `PPTX_MAX_SLIDE_SVG_BYTES`).
 */
export const PPTX_MAX_MARKUP_BYTES = 16 * 1024 * 1024;

/** Package entry count cap (a real deck has tens to a few thousand parts). */
export const PPTX_MAX_ENTRIES = 10_000;

/** Slide count cap for one deck. */
export const PPTX_MAX_SLIDES = 1_000;

/**
 * Rendered SVG cap for one slide, in UTF-8 bytes. Embedded pictures are
 * inlined as base64 `data:` URLs, so this bounds the images one slide carries.
 */
export const PPTX_MAX_SLIDE_SVG_BYTES = 64 * 1024 * 1024;

/** Wall-clock bound for opening a deck in the worker (download excluded). */
export const PPTX_OPEN_TIMEOUT_MS = 20_000;

/** Wall-clock bound for laying out one slide in the worker. */
export const PPTX_RENDER_TIMEOUT_MS = 10_000;

/**
 * Compressed input handed to the streaming reader per step. At DEFLATE's
 * ~1032:1 limit one step inflates ≤ ~4 MiB (≈ 20 ms measured in Node).
 */
const INPUT_CHUNK = 4 * 1024;

/** Reading yields to the event loop once a slice has run this long. */
const SLICE_MS = 16;

export type PptxRepack =
  | { status: "ok"; bytes: Uint8Array; entries: number; expanded: number; markup: number }
  | { status: "tooLarge" }
  | { status: "invalid" };

/**
 * Whether a part could be markup for the loader, from its leading bytes.
 * `@office-kit/pptx` decodes parts as UTF-8 and its XML reader requires `<`
 * after an optional BOM and XML whitespace (space, tab, CR, LF); anything
 * else fails as "expected root element". So a byte other than those, or the
 * BOM bytes EF BB BF, before the first `<` means the loader cannot parse the
 * part. Returns `undefined` while only such leading bytes have been seen.
 */
export function startsLikeMarkup(data: Uint8Array): boolean | undefined {
  for (const byte of data) {
    if (byte === 0x3c) return true;
    if (byte === 0x20 || byte === 0x09 || byte === 0x0a || byte === 0x0d) continue;
    if (byte === 0xef || byte === 0xbb || byte === 0xbf) continue;
    return false;
  }
  return undefined;
}

const yieldToEventLoop = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** Always a fresh copy: stored entries arrive as views into the input buffer. */
function concat(chunks: Uint8Array[], size: number): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.byteLength;
  }
  return out;
}

/**
 * `@office-kit/pptx`'s `loadPresentation` inflates every central-directory
 * record at once with fflate's `unzipSync` and no bound: each record gets a
 * buffer of its *declared* size, and inflation runs to the end of the DEFLATE
 * stream whatever that size says, so a small file can cost unbounded memory
 * (declared sizes) or CPU (real stream length, repeated by overlapping
 * records). The loader takes only bytes, so the guard cannot hand it parts.
 *
 * Instead the package is read once with fflate's public streaming `Unzip`
 * (the same library, local-header order), fed `INPUT_CHUNK` bytes at a time,
 * and the inflated bytes are counted as they appear — declared sizes are not
 * trusted. It stops at the first chunk past `maxExpanded` or `maxMarkup`
 * (overshoot ≤ one chunk's output, ≤ ~4 MiB at DEFLATE's ~1032:1 limit) or
 * past `maxEntries`. An entry's bytes count as markup unless its leading
 * bytes rule that out (`startsLikeMarkup`), whatever its name: the loader
 * follows relationships, not extensions, so a slide can live at `media/x.png`.
 * The read yields to the event loop every `SLICE_MS`, so the page stays
 * responsive and a cancelled load stops at the next step.
 * Duplicate names (compared case-insensitively, as OPC part names are),
 * unknown compression methods, truncated or malformed data fail the check.
 *
 * The checked entries are then re-packed as a STORE-only ZIP with fflate's
 * `zipSync`, and only those bytes go to the renderer. Its `unzipSync` view is
 * therefore exactly the entries counted here: no hidden, shadowed or
 * overlapping record, no inflation, true sizes. A central directory that
 * disagrees with the local headers cannot smuggle anything past the check; a
 * package that only such a directory describes simply renders differently or
 * fails. Directory entries are dropped, as the loader skips them anyway.
 *
 * The byte copies here (parts, re-packed ZIP, the loader's copies) cost a
 * few times the inflated size; the loader's parsed XML costs far more per
 * byte, hence `maxMarkup`. These caps bound inflated bytes and entry count,
 * not CPU: parsing and slide layout run in a worker with a wall-clock bound
 * (`pptx-client.ts`).
 *
 * Cancellation (`isAlive() === false`) is honoured between input chunks.
 */
export async function repackPptx(
  bytes: Uint8Array,
  isAlive: () => boolean,
  maxExpanded: number = PPTX_MAX_EXPANDED_BYTES,
  maxEntries: number = PPTX_MAX_ENTRIES,
  maxMarkup: number = PPTX_MAX_MARKUP_BYTES,
): Promise<PptxRepack> {
  if (!isAlive() || bytes.byteLength === 0) return { status: "invalid" };
  const files: Unzipped = {};
  const seen = new Set<string>();
  // Assigned from the reader callbacks; the cast keeps TS from narrowing it to `null`.
  let failure = null as "tooLarge" | "invalid" | null;
  let entries = 0;
  let pending = 0;
  let expanded = 0;
  let markup = 0;

  const unzip = new Unzip((file) => {
    if (failure) return;
    entries += 1;
    if (entries > maxEntries) {
      failure = "tooLarge";
      return;
    }
    const key = file.name.toLowerCase();
    if (seen.has(key) || (file.compression !== 0 && file.compression !== 8)) {
      failure = "invalid";
      return;
    }
    seen.add(key);
    const directory = file.name.endsWith("/");
    const chunks: Uint8Array[] = [];
    let size = 0;
    let isMarkup: boolean | undefined;
    pending += 1;
    file.ondata = (error, data, final) => {
      if (failure) return;
      if (error) {
        failure = "invalid";
        return;
      }
      size += data.byteLength;
      expanded += data.byteLength;
      if (isMarkup === undefined) isMarkup = startsLikeMarkup(data);
      if (isMarkup !== false) markup += data.byteLength;
      if (expanded > maxExpanded || markup > maxMarkup) {
        failure = "tooLarge";
        return;
      }
      if (data.byteLength > 0) chunks.push(data);
      if (final) {
        pending -= 1;
        if (!directory) files[file.name] = concat(chunks, size);
      }
    };
    file.start();
  });
  unzip.register(UnzipInflate);

  try {
    let slice = performance.now();
    for (let at = 0; at < bytes.byteLength && !failure; at += INPUT_CHUNK) {
      if (performance.now() - slice >= SLICE_MS) {
        await yieldToEventLoop();
        slice = performance.now();
      }
      if (!isAlive()) return { status: "invalid" };
      const end = Math.min(bytes.byteLength, at + INPUT_CHUNK);
      unzip.push(bytes.subarray(at, end), end === bytes.byteLength);
    }
  } catch {
    return { status: "invalid" };
  }
  if (failure) return { status: failure };
  if (!isAlive() || entries === 0 || pending !== 0) return { status: "invalid" };
  let repacked: Uint8Array;
  try {
    repacked = zipSync(files, { level: 0 });
  } catch {
    return { status: "invalid" };
  }
  return { status: "ok", bytes: repacked, entries, expanded, markup };
}
