/**
 * What the frame does with a file that is a ZIP archive before its renderer
 * gets it: unpacks every part, counts what it unpacks to, and writes a new
 * archive of those parts alone, which is what the renderer is handed.
 *
 * The app holds an archive to limits on what its directory says the parts
 * unpack to (`$lib/file-viewer/formats.ts`). But a directory can say less
 * than a part holds, and the renderers' unpacker (JSZip) can read the same
 * bytes otherwise than `zip-directory.ts` does: more entries than the count,
 * the offsets shifted, a ZIP64 directory elsewhere, a local name not the
 * directory's. Checking the parts and then handing on the original bytes
 * would check one archive and draw another. So here:
 *
 * - parts whose packed bytes overlap stop the file (one region counted many
 *   times over would make the new archive many times the old);
 * - each part is unpacked as a stream, and stopped once it runs past what is
 *   said for it, or past what is left of the limits;
 * - XML is told by what a part holds (its first character past blanks is
 *   "<", read as the renderer will read it), not by its name;
 * - the new archive has each part's name as the directory's bytes, its true
 *   size, and nothing that was not counted: no extra fields, no comments, no
 *   gaps.
 *
 * The frame's thread is the app's, and the unpacking resolves as promises
 * that do not let a timer in: it gives the thread back every `BREATH`
 * milliseconds, between parts and between the slices of one, as it reads
 * them for blanks as well as as it unpacks them.
 */
import {
  MARKUP_LIMIT_BYTES,
  UNPACKED_LIMIT_BYTES,
  unpacksTooLarge,
} from '$lib/file-viewer/formats.ts';
import { zipEntries, type ZipEntry } from '$lib/file-viewer/zip-directory.ts';

import { UnreadableFile } from './renderer.ts';

const LOCAL_SIGNATURE = 0x04034b50;
const ENTRY_SIGNATURE = 0x02014b50;
const END_SIGNATURE = 0x06054b50;
const LOCAL_SIZE = 30;
const ENTRY_SIZE = 46;
const END_SIZE = 22;
const STORED = 0;
const DEFLATED = 8;
const ENCRYPTED = 0x0001;
const BACKSLASH = 0x5c;
/** The flag that says a name is UTF-8, as the zip-directory reader reads every name. */
const UTF8_NAMES = 0x0800;
const VERSION = 20;
const MOST_32 = 0xffffffff;
const MOST_16 = 0xffff;

/**
 * The code points that may come before a document's first tag and are not
 * one: white space, a byte order mark, and NUL (the other half of UTF-16).
 */
const BLANK = new Set([0x20, 0x09, 0x0a, 0x0d, 0x00, 0xfeff]);
/** How much of a part's packed bytes the unpacker is fed at a time: little, so one feed cannot unpack to much. */
const SLICE = 16 * 1024;
/** The longest the frame's thread is held while an archive is counted, in milliseconds. */
const BREATH = 16;

/** Gives the thread back, when it has been held `BREATH` milliseconds since it last was. */
type Breathe = () => Promise<void>;

/** A task of its own, after those waiting: a message to itself, which no browser delays as it does timers. */
function nextTask(): Promise<void> {
  const { port1, port2 } = new MessageChannel();
  return new Promise((resolve) => {
    port1.onmessage = () => {
      port1.close();
      resolve();
    };
    port2.postMessage(null);
  });
}

function breather(): Breathe {
  let since = performance.now();
  return async () => {
    if (performance.now() - since < BREATH) return;
    await nextTask();
    since = performance.now();
  };
}

/** Feeds the sniffer a slice at a time, giving the thread back between slices (it reads blanks one by one). */
async function sniff(sniffer: MarkupSniffer, bytes: Uint8Array, breathe: Breathe): Promise<void> {
  for (let at = 0; at < bytes.length && sniffer.verdict === undefined; at += SLICE) {
    sniffer.feed(bytes.subarray(at, at + SLICE));
    await breathe();
  }
}

/** A part as counted: what goes into the new archive. */
interface Part {
  name: Uint8Array;
  method: number;
  crc: number;
  packed: Uint8Array<ArrayBuffer>;
  /** What it unpacked to, in bytes. */
  size: number;
}

/** What the parts counted so far have used of the limits. */
interface Used {
  total: number;
  markup: number;
}

/**
 * Tells, from a part's bytes as they come, whether it is XML (or other text
 * that starts with a tag): its first character past any number of blanks
 * is "<" (a parser skips the blanks too). Undecided while it has seen only
 * blanks.
 *
 * The bytes are read as JSZip reads a part as text in a browser (its
 * `buf2string`, from pako), which is lenient: it takes overlong forms and
 * does not check continuation bytes, so `C0 BC` is "<" to it. Reading them
 * any stricter would let XML pass here as something else.
 */
export class MarkupSniffer {
  /** The code point being joined, and how many continuation bytes it still takes. */
  #code = 0;
  #need = 0;
  /** Bytes of a five- or six-byte form still to skip. */
  #skip = 0;
  #verdict: boolean | undefined;

  /** True when it is XML, false when not, undefined while it cannot tell. */
  get verdict(): boolean | undefined {
    return this.#verdict;
  }

  feed(chunk: Uint8Array): void {
    for (let at = 0; at < chunk.length && this.#verdict === undefined; at += 1) {
      this.#verdict = this.#byte(chunk[at] ?? 0);
    }
  }

  #byte(byte: number): boolean | undefined {
    if (this.#skip > 0) {
      this.#skip -= 1;
      return undefined;
    }
    if (this.#need > 0) {
      this.#code = (this.#code << 6) | (byte & 0x3f);
      this.#need -= 1;
      return this.#need > 0 ? undefined : this.#character(this.#code);
    }
    if (byte < 0x80) return this.#character(byte);
    // buf2string's table of lengths, with its quirk: 0xFE is 1, 0xFF stays 6.
    const length =
      byte === 0xfe
        ? 1
        : byte >= 252
          ? 6
          : byte >= 248
            ? 5
            : byte >= 240
              ? 4
              : byte >= 224
                ? 3
                : byte >= 192
                  ? 2
                  : 1;
    if (length > 4) {
      this.#skip = length - 1;
      return this.#character(0xfffd);
    }
    this.#code = byte & (length === 2 ? 0x1f : length === 3 ? 0x0f : 0x07);
    this.#need = length - 1;
    return this.#need > 0 ? undefined : this.#character(this.#code);
  }

  #character(code: number): boolean | undefined {
    return BLANK.has(code) ? undefined : code === 0x3c;
  }
}

/** Where a part's local header starts and its packed bytes end. */
function span(bytes: Uint8Array, entry: ZipEntry): { start: number; data: number; end: number } {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const at = entry.offset;
  if (at + LOCAL_SIZE > bytes.byteLength || view.getUint32(at, true) !== LOCAL_SIGNATURE) {
    throw new UnreadableFile();
  }
  const data = at + LOCAL_SIZE + view.getUint16(at + 26, true) + view.getUint16(at + 28, true);
  const end = data + entry.compressedSize;
  if (end > bytes.byteLength) throw new UnreadableFile();
  return { start: at, data, end };
}

/** The packed bytes of each part, or `UnreadableFile` when two parts share any. */
function packedParts(
  bytes: Uint8Array<ArrayBuffer>,
  entries: readonly ZipEntry[],
): Uint8Array<ArrayBuffer>[] {
  const spans = entries.map((entry) => span(bytes, entry));
  const ordered = [...spans].sort((a, b) => a.start - b.start);
  for (let index = 1; index < ordered.length; index += 1) {
    const [before, after] = [ordered[index - 1], ordered[index]];
    if (before && after && after.start < before.end) throw new UnreadableFile();
  }
  return spans.map(({ data, end }) => bytes.subarray(data, end));
}

/**
 * How many times docx-preview may parse a part, at most, as it finds it
 * under its two spellings (with and without a leading "/"): twice, and a
 * part's relationships (`_rels/….rels`) four times. The XML limit holds for
 * what is parsed, so a part counts toward it that many times over, halved.
 */
function readings(name: string): number {
  return /(?:^|\/)_rels\/[^/]*\.rels$/i.test(name) ? 2 : 1;
}

/** Whether a part of `size` bytes so far, sniffed so, is past what the parts before left of the limits. */
function pastLimits(size: number, weight: number, sniffer: MarkupSniffer, used: Used): boolean {
  if (used.total + size > UNPACKED_LIMIT_BYTES) return true;
  // Until it is known not to be XML, it is counted as XML.
  return sniffer.verdict !== false && used.markup + weight * size > MARKUP_LIMIT_BYTES;
}

/** Unpacks deflated bytes, counting and sniffing them; stops once past `most` or the limits. */
async function inflate(
  packed: Uint8Array<ArrayBuffer>,
  most: number,
  sniffer: MarkupSniffer,
  past: (size: number) => boolean,
  breathe: Breathe,
): Promise<number> {
  let at = 0;
  const slices = new ReadableStream<Uint8Array<ArrayBuffer>>({
    pull(controller) {
      if (at >= packed.length) {
        controller.close();
        return;
      }
      controller.enqueue(packed.slice(at, (at += SLICE)));
    },
  });
  const reader = slices.pipeThrough(new DecompressionStream('deflate-raw')).getReader();
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return size;
      size += value.byteLength;
      await sniff(sniffer, value, breathe);
      if (size > most || past(size)) throw new UnreadableFile();
      await breathe();
    }
  } catch {
    // Past what was said or the limits, or not deflated data.
    void reader.cancel().catch(() => undefined);
    throw new UnreadableFile();
  }
}

/** A part unpacked and counted into `used`; or `UnreadableFile`. */
async function count(
  entry: ZipEntry,
  packed: Uint8Array<ArrayBuffer>,
  used: Used,
  breathe: Breathe,
): Promise<Part> {
  if (entry.flags & ENCRYPTED) throw new UnreadableFile();
  // A backslash in a name: docx-preview finds such a part under any mix of it and "/", and
  // would parse it once for each spelling (a package's part names have none).
  if (entry.rawName.includes(BACKSLASH)) throw new UnreadableFile();
  const weight = readings(entry.name);
  const sniffer = new MarkupSniffer();
  const past = (size: number): boolean => pastLimits(size, weight, sniffer, used);
  let size: number;
  if (entry.method === STORED) {
    size = packed.byteLength;
    if (size > entry.size) throw new UnreadableFile();
    await sniff(sniffer, packed, breathe);
    if (past(size)) throw new UnreadableFile();
  } else if (entry.method === DEFLATED) {
    size = await inflate(packed, entry.size, sniffer, past, breathe);
  } else {
    throw new UnreadableFile();
  }
  used.total += size;
  // A part of blanks only is no XML, but is counted as one all the same.
  if (sniffer.verdict !== false) used.markup += weight * size;
  return { name: entry.rawName, method: entry.method, crc: entry.crc, packed, size };
}

/** A ZIP archive of the parts, with nothing else in it; `UnreadableFile` when they do not fit one without ZIP64. */
export function writeArchive(parts: readonly Part[]): ArrayBuffer {
  let length = END_SIZE;
  for (const part of parts) {
    if (part.name.length > MOST_16 || part.size > MOST_32) throw new UnreadableFile();
    length += LOCAL_SIZE + ENTRY_SIZE + 2 * part.name.length + part.packed.length;
  }
  if (parts.length > MOST_16 || length > MOST_32) throw new UnreadableFile();
  const out = new Uint8Array(length);
  const view = new DataView(out.buffer);
  const offsets: number[] = [];
  let at = 0;
  const header = (signature: number, part: Part, fields: number): void => {
    view.setUint32(at, signature, true);
    // The fields of the local and the directory header are the same, the
    // directory's after "version made by"; `fields` is where they start.
    view.setUint16(at + fields, VERSION, true);
    view.setUint16(at + fields + 2, UTF8_NAMES, true);
    view.setUint16(at + fields + 4, part.method, true);
    view.setUint32(at + fields + 10, part.crc, true);
    view.setUint32(at + fields + 14, part.packed.length, true);
    view.setUint32(at + fields + 18, part.size, true);
    view.setUint16(at + fields + 22, part.name.length, true);
  };
  for (const part of parts) {
    offsets.push(at);
    header(LOCAL_SIGNATURE, part, 4);
    out.set(part.name, at + LOCAL_SIZE);
    out.set(part.packed, at + LOCAL_SIZE + part.name.length);
    at += LOCAL_SIZE + part.name.length + part.packed.length;
  }
  const directory = at;
  parts.forEach((part, index) => {
    header(ENTRY_SIGNATURE, part, 6);
    view.setUint16(at + 4, VERSION, true);
    view.setUint32(at + 42, offsets[index] ?? 0, true);
    out.set(part.name, at + ENTRY_SIZE);
    at += ENTRY_SIZE + part.name.length;
  });
  view.setUint32(at, END_SIGNATURE, true);
  view.setUint16(at + 8, parts.length, true);
  view.setUint16(at + 10, parts.length, true);
  view.setUint32(at + 12, at - directory, true);
  view.setUint32(at + 16, directory, true);
  return out.buffer;
}

/**
 * The archive anew, of its parts as counted; throws `UnreadableFile` when the
 * bytes are no archive the viewer reads, a part unpacks to more than its
 * directory says, or the parts to more than the limits.
 */
export async function repack(buffer: ArrayBuffer): Promise<ArrayBuffer> {
  // A browser without it cannot count, and says so as a renderer's failure, not a damaged file's.
  if (typeof DecompressionStream !== 'function') throw new Error('No DecompressionStream');
  const bytes = new Uint8Array(buffer);
  const entries = zipEntries(bytes);
  // What the directory says is held to the limits here as well as in the app.
  if (!entries || unpacksTooLarge(entries)) throw new UnreadableFile();
  const packed = packedParts(bytes, entries);
  const used: Used = { total: 0, markup: 0 };
  const parts: Part[] = [];
  const breathe = breather();
  for (const [index, entry] of entries.entries()) {
    parts.push(await count(entry, packed[index] ?? new Uint8Array(), used, breathe));
    await breathe();
  }
  return writeArchive(parts);
}
