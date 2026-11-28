/**
 * A ZIP of files stored as they are, without compression (method 0), their
 * names in UTF-8 (bit 11): what any ZIP reader opens. For the in-memory
 * backend's exports, which are small; the server compresses its own.
 */

/** A file of the ZIP: its path inside it, with `/` between folders. */
export interface ZipEntry {
  readonly name: string;
  readonly bytes: Uint8Array;
}

const CRC_TABLE = Uint32Array.from({ length: 256 }, (_, index) => {
  let crc = index;
  for (let bit = 0; bit < 8; bit += 1) crc = crc & 1 ? 0xedb88320 ^ (crc >>> 1) : crc >>> 1;
  return crc >>> 0;
});

/** The CRC-32 of `bytes`, as ZIP has it. */
export function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = (CRC_TABLE[(crc ^ byte) & 0xff] ?? 0) ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

/** Writes little-endian numbers one after the other. */
class Out {
  readonly parts: Uint8Array[] = [];
  length = 0;

  numbers(...values: readonly (readonly [number, 2 | 4])[]): void {
    const bytes = new Uint8Array(values.reduce((sum, [, size]) => sum + size, 0));
    const view = new DataView(bytes.buffer);
    let at = 0;
    for (const [value, size] of values) {
      if (size === 2) view.setUint16(at, value, true);
      else view.setUint32(at, value, true);
      at += size;
    }
    this.bytes(bytes);
  }

  bytes(bytes: Uint8Array): void {
    this.parts.push(bytes);
    this.length += bytes.length;
  }
}

/** The ZIP of `entries`, in their order. */
export function storedZip(entries: readonly ZipEntry[]): Uint8Array<ArrayBuffer> {
  const encoder = new TextEncoder();
  const out = new Out();
  const directory = new Out();
  for (const entry of entries) {
    const name = encoder.encode(entry.name);
    const crc = crc32(entry.bytes);
    const size = entry.bytes.length;
    const offset = out.length;
    // A local header: version 2.0, names in UTF-8, stored, no time, then the file.
    out.numbers([0x04034b50, 4], [20, 2], [0x0800, 2], [0, 2], [0, 2], [0x21, 2]);
    out.numbers([crc, 4], [size, 4], [size, 4], [name.length, 2], [0, 2]);
    out.bytes(name);
    out.bytes(entry.bytes);
    directory.numbers([0x02014b50, 4], [20, 2], [20, 2], [0x0800, 2], [0, 2], [0, 2]);
    directory.numbers([0x21, 2], [crc, 4], [size, 4], [size, 4], [name.length, 2]);
    directory.numbers([0, 2], [0, 2], [0, 2], [0, 2], [0, 4], [offset, 4]);
    directory.bytes(name);
  }
  const start = out.length;
  for (const part of directory.parts) out.bytes(part);
  out.numbers([0x06054b50, 4], [0, 2], [0, 2], [entries.length, 2], [entries.length, 2]);
  out.numbers([directory.length, 4], [start, 4], [0, 2]);
  const zip = new Uint8Array(out.length);
  let at = 0;
  for (const part of out.parts) {
    zip.set(part, at);
    at += part.length;
  }
  return zip;
}
