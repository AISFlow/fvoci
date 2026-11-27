/**
 * The parts of a workbook's archive, unpacked one at a time as the reader
 * asks for them, in the spreadsheet worker. The archive is the one the
 * frame wrote of the parts it counted (`zip-guard.ts`): each part's sizes
 * are its true ones and its name the directory's, so this reads them as
 * they are and stops at a part that unpacks otherwise.
 *
 * Part names are found as the package's rules have them: without case, and
 * resolved from the part that names them (`../worksheets/sheet1.xml`).
 */
import { zipEntries, type ZipEntry } from '$lib/file-viewer/zip-directory.ts';

import { Unreadable } from './xlsx-xml.ts';

const LOCAL_SIZE = 30;
const STORED = 0;
const DEFLATED = 8;

/** A part's name resolved against the folder of the part that names it, without a leading "/". */
export function resolvePart(from: string, target: string): string {
  const base = target.startsWith('/') ? [] : from.split('/').slice(0, -1);
  for (const segment of target.split('/')) {
    if (segment === '..') base.pop();
    else if (segment !== '.' && segment !== '') base.push(segment);
  }
  return base.join('/');
}

/** The relationships part of a part: `xl/workbook.xml` has `xl/_rels/workbook.xml.rels`. */
export function relationshipsOf(part: string): string {
  const slash = part.lastIndexOf('/');
  return `${part.slice(0, slash + 1)}_rels/${part.slice(slash + 1)}.rels`;
}

function decode(bytes: Uint8Array): string {
  // A part's text is UTF-8 unless its byte order mark says UTF-16.
  const [first, second] = bytes;
  const encoding =
    first === 0xff && second === 0xfe
      ? 'utf-16le'
      : first === 0xfe && second === 0xff
        ? 'utf-16be'
        : 'utf-8';
  return new TextDecoder(encoding).decode(bytes);
}

export class Archive {
  readonly #bytes: Uint8Array<ArrayBuffer>;
  readonly #entries = new Map<string, ZipEntry>();

  constructor(buffer: ArrayBuffer) {
    this.#bytes = new Uint8Array(buffer);
    const entries = zipEntries(this.#bytes);
    if (!entries) throw new Unreadable('no archive');
    for (const entry of entries) this.#entries.set(entry.name.toLowerCase(), entry);
  }

  has(name: string): boolean {
    return this.#entries.has(name.toLowerCase());
  }

  /** A part's text; undefined when the archive has no such part. */
  async text(name: string): Promise<string | undefined> {
    const entry = this.#entries.get(name.toLowerCase());
    if (!entry) return undefined;
    return decode(await this.#unpack(entry));
  }

  async #unpack(entry: ZipEntry): Promise<Uint8Array> {
    const view = new DataView(this.#bytes.buffer);
    const at = entry.offset;
    if (at + LOCAL_SIZE > this.#bytes.length) throw new Unreadable('a part out of place');
    const start = at + LOCAL_SIZE + view.getUint16(at + 26, true) + view.getUint16(at + 28, true);
    const packed = this.#bytes.subarray(start, start + entry.compressedSize);
    if (entry.method === STORED) return packed;
    if (entry.method !== DEFLATED) throw new Unreadable('a part packed otherwise');
    const stream = new Blob([packed]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
    let unpacked: Uint8Array;
    try {
      unpacked = new Uint8Array(await new Response(stream).arrayBuffer());
    } catch {
      throw new Unreadable('a part that does not unpack');
    }
    if (unpacked.length !== entry.size) throw new Unreadable('a part of another size');
    return unpacked;
  }
}
