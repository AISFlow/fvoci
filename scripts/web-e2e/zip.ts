// Minimal ZIP reader/writer for Playwright trace.zip. Stored and deflate only.

import { inflateRawSync } from "node:zlib";
import { deflateRawSync } from "node:zlib";
import { crc32 } from "node:zlib";

export type ZipMember = {
  filename: string;
  method: number;
  flagBits: number;
  compressedSize: number;
  fileSize: number;
  externalAttr: number;
  localOffset: number;
  data: Uint8Array;
  isDir: boolean;
};

const localSignature = 0x04034b50;
const centralSignature = 0x02014b50;
const eocdSignature = 0x06054b50;

function u16(bytes: Uint8Array, offset: number): number {
  return (bytes[offset] ?? 0) | ((bytes[offset + 1] ?? 0) << 8);
}
function u32(bytes: Uint8Array, offset: number): number {
  return (
    ((bytes[offset] ?? 0) |
      ((bytes[offset + 1] ?? 0) << 8) |
      ((bytes[offset + 2] ?? 0) << 16) |
      ((bytes[offset + 3] ?? 0) << 24)) >>>
    0
  );
}

export function readZip(bytes: Uint8Array): ZipMember[] {
  let eocd = -1;
  const start = Math.max(0, bytes.length - 22 - 65535);
  for (let offset = bytes.length - 22; offset >= start; offset -= 1) {
    if (u32(bytes, offset) === eocdSignature) {
      eocd = offset;
      break;
    }
  }
  if (eocd < 0) throw new Error("bad zip");
  const count = u16(bytes, eocd + 10);
  let cursor = u32(bytes, eocd + 16);
  const members: ZipMember[] = [];
  for (let index = 0; index < count; index += 1) {
    if (u32(bytes, cursor) !== centralSignature) throw new Error("bad zip");
    const flagBits = u16(bytes, cursor + 8);
    const method = u16(bytes, cursor + 10);
    const compressedSize = u32(bytes, cursor + 20);
    const fileSize = u32(bytes, cursor + 24);
    const nameLength = u16(bytes, cursor + 28);
    const extraLength = u16(bytes, cursor + 30);
    const commentLength = u16(bytes, cursor + 32);
    const externalAttr = u32(bytes, cursor + 38);
    const localOffset = u32(bytes, cursor + 42);
    const filename = Buffer.from(bytes.subarray(cursor + 46, cursor + 46 + nameLength)).toString(
      "utf8",
    );
    if (u32(bytes, localOffset) !== localSignature) throw new Error("bad zip");
    const localName = u16(bytes, localOffset + 26);
    const localExtra = u16(bytes, localOffset + 28);
    const dataOffset = localOffset + 30 + localName + localExtra;
    const compressed = bytes.subarray(dataOffset, dataOffset + compressedSize);
    let data: Uint8Array;
    if (method === 0) data = compressed;
    else if (method === 8) data = inflateRawSync(compressed);
    else throw new Error(`unsupported zip method ${method}`);
    members.push({
      filename,
      method,
      flagBits,
      compressedSize,
      fileSize,
      externalAttr,
      localOffset,
      data,
      isDir: filename.endsWith("/"),
    });
    cursor += 46 + nameLength + extraLength + commentLength;
  }
  return members;
}

export function writeZip(
  entries: { name: string; data: Uint8Array | string; deflate?: boolean }[],
): Uint8Array {
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const data = typeof entry.data === "string" ? Buffer.from(entry.data) : Buffer.from(entry.data);
    const deflate = entry.deflate === true;
    const compressed = deflate ? deflateRawSync(data) : data;
    const name = Buffer.from(entry.name);
    const local = Buffer.alloc(30 + name.length);
    local.writeUInt32LE(localSignature, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(deflate ? 8 : 0, 8);
    local.writeUInt32LE(crc32(data) >>> 0, 14);
    local.writeUInt32LE(compressed.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(name.length, 26);
    name.copy(local, 30);
    locals.push(local, compressed);
    const central = Buffer.alloc(46 + name.length);
    central.writeUInt32LE(centralSignature, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(deflate ? 8 : 0, 10);
    central.writeUInt32LE(crc32(data) >>> 0, 16);
    central.writeUInt32LE(compressed.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE((0o100644 * 65536) >>> 0, 38);
    central.writeUInt32LE(offset, 42);
    name.copy(central, 46);
    centrals.push(central);
    offset += local.length + compressed.length;
  }
  const centralStart = offset;
  const centralSize = centrals.reduce((sum, part) => sum + part.length, 0);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(eocdSignature, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(centralSize, 12);
  eocd.writeUInt32LE(centralStart, 16);
  return Buffer.concat([...locals, ...centrals, eocd]);
}
