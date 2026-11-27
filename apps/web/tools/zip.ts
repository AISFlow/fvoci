/** Reads members of a zip archive. Enough for the archives the tools read; no zip64, no encryption. */
import { inflateRawSync } from 'node:zlib';

interface ZipEntry {
  name: string;
  method: number;
  compressedSize: number;
  localHeader: number;
}

function entries(archive: Buffer): ZipEntry[] {
  const endSignature = archive.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (endSignature === -1) throw new Error('not a zip archive');
  const count = archive.readUInt16LE(endSignature + 10);
  let at = archive.readUInt32LE(endSignature + 16);
  const found: ZipEntry[] = [];
  for (let index = 0; index < count; index += 1) {
    if (archive.readUInt32LE(at) !== 0x02014b50) throw new Error('corrupt zip directory');
    const nameLength = archive.readUInt16LE(at + 28);
    found.push({
      name: archive.toString('utf8', at + 46, at + 46 + nameLength),
      method: archive.readUInt16LE(at + 10),
      compressedSize: archive.readUInt32LE(at + 20),
      localHeader: archive.readUInt32LE(at + 42),
    });
    at += 46 + nameLength + archive.readUInt16LE(at + 30) + archive.readUInt16LE(at + 32);
  }
  return found;
}

/** The names of the members of an archive, in the order of its directory. */
export function listZipMembers(archive: Buffer): string[] {
  return entries(archive).map((entry) => entry.name);
}

export function readZipMember(archive: Buffer, memberName: string): Buffer {
  const entry = entries(archive).find((candidate) => candidate.name === memberName);
  if (!entry) throw new Error(`${memberName} is not in the archive`);
  const dataStart =
    entry.localHeader +
    30 +
    archive.readUInt16LE(entry.localHeader + 26) +
    archive.readUInt16LE(entry.localHeader + 28);
  const data = archive.subarray(dataStart, dataStart + entry.compressedSize);
  if (entry.method === 0) return Buffer.from(data);
  if (entry.method === 8) return inflateRawSync(data);
  throw new Error(`${memberName}: unsupported zip compression method ${entry.method}`);
}
