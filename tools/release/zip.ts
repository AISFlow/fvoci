// The ZIP subset the documents smoke needs: write a deflated archive for the
// Markdown import, and read an exported DOCX/PPTX fail-closed. ZIP64,
// multi-disk, encryption and methods other than stored/deflate are refused.
// JSZip (in the tree) is not used for reading: it deliberately accepts a local
// header that names a different file than the central directory, which this
// check must refuse. Inflation is node:zlib.
import { crc32, deflateRawSync, inflateRawSync } from "node:zlib";

const LOCAL = 0x04034b50;
const CENTRAL = 0x02014b50;
const END = 0x06054b50;
const UTF8_NAMES = 0x0800;
const ENCRYPTED = 0x0001;
const DESCRIPTOR = 0x0008;
// An exported DOCX/PPTX part is far smaller; a larger declared size is refused.
const MAX_ENTRY_BYTES = 256 * 1024 * 1024;

export type ZipEntry = { name: string; data: Uint8Array };

function dosTime(date: Date): [number, number] {
  const time = (date.getHours() << 11) | (date.getMinutes() << 5) | (date.getSeconds() >> 1);
  const day = ((date.getFullYear() - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate();
  return [time, day];
}

export function writeZip(entries: ZipEntry[], date = new Date()): Uint8Array {
  const [time, day] = dosTime(date);
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = Buffer.from(entry.name, "utf8");
    const flags = name.length === entry.name.length ? 0 : UTF8_NAMES;
    const packed = deflateRawSync(entry.data);
    const crc = crc32(entry.data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(LOCAL, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(flags, 6);
    local.writeUInt16LE(8, 8);
    local.writeUInt16LE(time, 10);
    local.writeUInt16LE(day, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(packed.length, 18);
    local.writeUInt32LE(entry.data.length, 22);
    local.writeUInt16LE(name.length, 26);
    locals.push(local, name, packed);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(CENTRAL, 0);
    central.writeUInt16LE((3 << 8) | 20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(flags, 8);
    central.writeUInt16LE(8, 10);
    central.writeUInt16LE(time, 12);
    central.writeUInt16LE(day, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(packed.length, 20);
    central.writeUInt32LE(entry.data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE((0o100600 << 16) >>> 0, 38);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, name);
    offset += 30 + name.length + packed.length;
  }
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(END, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

type Located = {
  name: string;
  method: number;
  crc: number;
  packedSize: number;
  size: number;
  start: number; // local header offset
  dataStart: number;
  end: number; // after the data and its data descriptor, if any
};

export type ZipArchive = {
  // Entry names in directory order, directories (ending in "/") included.
  readonly names: readonly string[];
  // Inflates one entry; the deflate stream must use exactly the declared
  // compressed bytes, and the size and CRC must match the directory.
  read(name: string): Uint8Array;
};

function fail(message: string): never {
  throw new Error(`zip: ${message}`);
}

const DESCRIPTOR_SIGNATURE = 0x08074b50;
const strictUtf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

function entryName(raw: Uint8Array, flags: number): string {
  if (flags & UTF8_NAMES) {
    try {
      return strictUtf8.decode(raw);
    } catch {
      return fail("entry name is not valid UTF-8");
    }
  }
  // Without the UTF-8 flag the name is CP437; only its ASCII subset is taken.
  if (raw.some((byte) => byte > 0x7f)) fail("non-ASCII entry name without the UTF-8 flag");
  return Buffer.from(raw).toString("latin1");
}

// Validates the whole structure before any entry is inflated:
// - exactly one end record, closing the file, on a single disk, no ZIP64;
// - the central directory fills the space between the last entry and the
//   end record, and its entries account for all of it;
// - names are valid, and unique ignoring ASCII case (Office part names are
//   case-insensitive);
// - every local header matches its central entry (name bytes, method,
//   encryption, descriptor and UTF-8 flags, and CRC and sizes from the local
//   header or the data descriptor);
// - local entries tile the file from offset 0 to the central directory with
//   no gap and no overlap, so no data hides outside the directory's view;
// - declared sizes stay within the per-entry and total budgets.
export function openZip(bytes: Uint8Array, maxTotalBytes = MAX_ENTRY_BYTES): ZipArchive {
  const buf = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const u16 = (at: number) => (at + 2 <= buf.length ? buf.readUInt16LE(at) : fail("truncated"));
  const u32 = (at: number) => (at + 4 <= buf.length ? buf.readUInt32LE(at) : fail("truncated"));
  const ends: number[] = [];
  for (let at = buf.length - 22; at >= Math.max(0, buf.length - 22 - 0xffff); at--) {
    if (buf.readUInt32LE(at) === END && at + 22 + buf.readUInt16LE(at + 20) === buf.length) {
      ends.push(at);
    }
  }
  const [end] = ends;
  if (end === undefined) fail("no end of central directory record");
  if (ends.length > 1) fail("more than one end of central directory record");
  const total = u16(end + 10);
  const dirSize = u32(end + 12);
  const dirStart = u32(end + 16);
  if (u16(end + 4) !== 0 || u16(end + 6) !== 0 || u16(end + 8) !== total) {
    fail("multi-disk archives are not supported");
  }
  if (total === 0xffff || dirSize === 0xffffffff || dirStart === 0xffffffff) {
    fail("ZIP64 archives are not supported");
  }
  if (dirStart + dirSize !== end) fail("central directory does not end at the end record");

  const entries: Located[] = [];
  const seen = new Set<string>();
  let declared = 0;
  let at = dirStart;
  for (let i = 0; i < total; i++) {
    if (u32(at) !== CENTRAL) fail("bad central directory entry");
    const flags = u16(at + 8);
    const method = u16(at + 10);
    const crc = u32(at + 16);
    const packedSize = u32(at + 20);
    const size = u32(at + 24);
    const nameLength = u16(at + 28);
    const next = at + 46 + nameLength + u16(at + 30) + u16(at + 32);
    const start = u32(at + 42);
    if (next > end) fail("central directory entry overruns the directory");
    const rawName = buf.subarray(at + 46, at + 46 + nameLength);
    const name = entryName(rawName, flags);
    at = next;
    if (!name) fail("entry without a name");
    const folded = name.replace(/[A-Z]/g, (c) => c.toLowerCase());
    if (seen.has(folded)) fail(`${name} appears twice`);
    seen.add(folded);
    if (flags & ENCRYPTED) fail(`${name}: encrypted entries are not supported`);
    if (method !== 0 && method !== 8) {
      fail(`${name}: compression method ${String(method)} is not supported`);
    }
    if (packedSize === 0xffffffff || size === 0xffffffff || start === 0xffffffff) {
      fail("ZIP64 archives are not supported");
    }
    if (size > MAX_ENTRY_BYTES) fail(`${name}: larger than ${String(MAX_ENTRY_BYTES)} bytes`);
    declared += size;
    if (declared > maxTotalBytes) fail(`entries exceed ${String(maxTotalBytes)} bytes in total`);
    if (method === 0 && packedSize !== size) fail(`${name}: stored size mismatch`);

    if (u32(start) !== LOCAL) fail(`${name}: bad local header`);
    const localFlags = u16(start + 6);
    const localNameLength = u16(start + 26);
    const dataStart = start + 30 + localNameLength + u16(start + 28);
    const localName = buf.subarray(start + 30, start + 30 + localNameLength);
    if (!localName.equals(rawName)) fail(`${name}: local header names a different file`);
    const disagrees = () => fail(`${name}: local header disagrees with the central directory`);
    if (((localFlags ^ flags) & (ENCRYPTED | DESCRIPTOR | UTF8_NAMES)) !== 0) disagrees();
    if (u16(start + 8) !== method) disagrees();
    const dataEnd = dataStart + packedSize;
    let entryEnd = dataEnd;
    if (flags & DESCRIPTOR) {
      // Optional signature, then CRC, compressed and uncompressed sizes.
      const sig = u32(dataEnd) === DESCRIPTOR_SIGNATURE ? 4 : 0;
      if (u32(dataEnd + sig) !== crc || u32(dataEnd + sig + 4) !== packedSize) disagrees();
      if (u32(dataEnd + sig + 8) !== size) disagrees();
      entryEnd = dataEnd + sig + 12;
    } else if (u32(start + 14) !== crc || u32(start + 18) !== packedSize) {
      disagrees();
    } else if (u32(start + 22) !== size) {
      disagrees();
    }
    if (entryEnd > dirStart) fail(`${name}: data overruns the central directory`);
    entries.push({ name, method, crc, packedSize, size, start, dataStart, end: entryEnd });
  }
  if (at !== end) fail("central directory size does not match its entries");
  let cursor = 0;
  for (const entry of [...entries].sort((a, b) => a.start - b.start)) {
    if (entry.start < cursor) fail(`${entry.name} overlaps another entry`);
    if (entry.start > cursor) fail(`unlisted bytes before ${entry.name}`);
    cursor = entry.end;
  }
  if (cursor !== dirStart) fail("unlisted bytes before the central directory");

  const byName = new Map(entries.map((entry) => [entry.name, entry]));
  return {
    names: entries.map((entry) => entry.name),
    read(name) {
      const entry = byName.get(name) ?? fail(`no entry ${name}`);
      const packed = buf.subarray(entry.dataStart, entry.dataStart + entry.packedSize);
      let data: Uint8Array = packed;
      if (entry.method === 8) {
        const result = inflateRawSync(packed, {
          maxOutputLength: Math.max(entry.size, 1),
          info: true,
        }) as unknown as { buffer: Buffer; engine: { bytesWritten: number } };
        if (result.engine.bytesWritten !== packed.length) {
          fail(`${name}: compressed data does not end with the deflate stream`);
        }
        data = result.buffer;
      }
      if (data.length !== entry.size || crc32(data) !== entry.crc) {
        fail(`${name}: bad CRC or size`);
      }
      return data;
    },
  };
}

// Every entry, inflated.
export function readZip(bytes: Uint8Array): ZipEntry[] {
  const archive = openZip(bytes);
  return archive.names.map((name) => ({ name, data: archive.read(name) }));
}
