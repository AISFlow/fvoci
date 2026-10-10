// The ZIP subset the documents smoke needs: write a deflated archive for the
// Markdown import, read stored or deflated entries of an exported DOCX/PPTX
// with their CRC checked. ZIP64, encryption and other methods are refused.
import { crc32, deflateRawSync, inflateRawSync } from "node:zlib";

const LOCAL = 0x04034b50;
const CENTRAL = 0x02014b50;
const END = 0x06054b50;
const UTF8_NAMES = 0x0800;
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

export function readZip(bytes: Uint8Array): ZipEntry[] {
  const buf = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let end = -1;
  for (let at = buf.length - 22; at >= Math.max(0, buf.length - 22 - 0xffff); at--) {
    if (buf.readUInt32LE(at) === END) {
      end = at;
      break;
    }
  }
  if (end < 0) throw new Error("not a zip archive");
  const total = buf.readUInt16LE(end + 10);
  const start = buf.readUInt32LE(end + 16);
  if (total === 0xffff || start === 0xffffffff) throw new Error("ZIP64 archives are not supported");
  const entries: ZipEntry[] = [];
  let at = start;
  for (let i = 0; i < total; i++) {
    if (buf.readUInt32LE(at) !== CENTRAL) throw new Error("bad zip central directory");
    const flags = buf.readUInt16LE(at + 8);
    const method = buf.readUInt16LE(at + 10);
    const crc = buf.readUInt32LE(at + 16);
    const packedSize = buf.readUInt32LE(at + 20);
    const size = buf.readUInt32LE(at + 24);
    const nameLength = buf.readUInt16LE(at + 28);
    const extraLength = buf.readUInt16LE(at + 30);
    const commentLength = buf.readUInt16LE(at + 32);
    const local = buf.readUInt32LE(at + 42);
    const rawName = buf.subarray(at + 46, at + 46 + nameLength);
    const name = rawName.toString(flags & UTF8_NAMES ? "utf8" : "latin1");
    at += 46 + nameLength + extraLength + commentLength;
    if (flags & 1) throw new Error(`${name}: encrypted zip entries are not supported`);
    if (packedSize === 0xffffffff || size === 0xffffffff || local === 0xffffffff) {
      throw new Error("ZIP64 archives are not supported");
    }
    if (size > MAX_ENTRY_BYTES)
      throw new Error(`${name}: entry larger than ${String(MAX_ENTRY_BYTES)} bytes`);
    if (buf.readUInt32LE(local) !== LOCAL) throw new Error(`${name}: bad local header`);
    const dataStart = local + 30 + buf.readUInt16LE(local + 26) + buf.readUInt16LE(local + 28);
    const packed = buf.subarray(dataStart, dataStart + packedSize);
    let data: Uint8Array;
    if (method === 0) data = packed;
    else if (method === 8) data = inflateRawSync(packed, { maxOutputLength: size + 1 });
    else throw new Error(`${name}: compression method ${String(method)} is not supported`);
    if (data.length !== size || crc32(data) !== crc) throw new Error(`${name}: bad CRC or size`);
    entries.push({ name, data });
  }
  return entries;
}
