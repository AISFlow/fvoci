/**
 * The central directory of a ZIP archive, read without inflating anything:
 * enough to tell an Office document from another archive, and to know how
 * large its parts say they are before anything unpacks them.
 *
 * Only what the viewer opens is handled: no archive split over several files,
 * and no ZIP64 (a file of at most `VIEW_LIMIT_BYTES` needs it only to claim
 * parts of 4 GB or more; such a part reads as 4 GB, too large in any case).
 * Anything malformed gives undefined: the bytes are then not a kind the
 * viewer draws.
 *
 * Everything here is what the archive says, and another reader (JSZip, in
 * the renderers) may read the same bytes otherwise. So the frame does not
 * hand an archive on: it unpacks the parts read here, counts them, and
 * writes a new archive of only those (src/viewer-frame/zip-guard.ts).
 */

const END_SIGNATURE = 0x06054b50;
const ENTRY_SIGNATURE = 0x02014b50;
const LOCAL_SIGNATURE = 0x04034b50;
const LOCAL_SIZE = 30;
const STORED = 0;
/** The end record is 22 bytes, followed by a comment of at most 65535. */
const END_SIZE = 22;
const END_SEARCH = END_SIZE + 0xffff;
const ENTRY_SIZE = 46;
/** More parts than any document has; past this the archive is not one the viewer reads. */
const ENTRY_LIMIT = 10_000;

/** One part of an archive, as its central directory describes it. */
export interface ZipEntry {
  /** Its name, read as UTF-8 (a byte that is none becomes U+FFFD). */
  name: string;
  /** Its name as the bytes the directory has. */
  rawName: Uint8Array;
  /** Its general purpose flags; bit 0 is encryption. */
  flags: number;
  /** 0 stored, 8 deflated; anything else the viewer does not unpack. */
  method: number;
  crc: number;
  compressedSize: number;
  /** The size it says it unpacks to. */
  size: number;
  /** Where its local header starts. */
  offset: number;
}

const decoder = new TextDecoder('utf-8', { fatal: false });

function endRecord(view: DataView): number | undefined {
  const lowest = Math.max(0, view.byteLength - END_SEARCH);
  for (let at = view.byteLength - END_SIZE; at >= lowest; at -= 1) {
    if (view.getUint32(at, true) === END_SIGNATURE) return at;
  }
  return undefined;
}

/** The parts in the archive's central directory, or undefined when the bytes are no archive this reads. */
export function zipEntries(bytes: Uint8Array): ZipEntry[] | undefined {
  if (bytes.byteLength < END_SIZE) return undefined;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const end = endRecord(view);
  if (end === undefined) return undefined;
  const disk = view.getUint16(end + 4, true);
  const count = view.getUint16(end + 10, true);
  const size = view.getUint32(end + 12, true);
  let at = view.getUint32(end + 16, true);
  if (disk !== 0 || count > ENTRY_LIMIT || at + size > end) return undefined;

  const entries: ZipEntry[] = [];
  for (let index = 0; index < count; index += 1) {
    if (at + ENTRY_SIZE > end || view.getUint32(at, true) !== ENTRY_SIGNATURE) return undefined;
    const nameLength = view.getUint16(at + 28, true);
    const extraLength = view.getUint16(at + 30, true);
    const commentLength = view.getUint16(at + 32, true);
    const nameStart = at + ENTRY_SIZE;
    if (nameStart + nameLength > end) return undefined;
    const rawName = bytes.subarray(nameStart, nameStart + nameLength);
    entries.push({
      name: decoder.decode(rawName),
      rawName,
      flags: view.getUint16(at + 8, true),
      method: view.getUint16(at + 10, true),
      crc: view.getUint32(at + 16, true),
      compressedSize: view.getUint32(at + 20, true),
      size: view.getUint32(at + 24, true),
      offset: view.getUint32(at + 42, true),
    });
    at = nameStart + nameLength + extraLength + commentLength;
  }
  return entries;
}

/**
 * What a part holds when it is stored as it is (not deflated) and holds at
 * most `most` bytes, read from after its local header; undefined otherwise.
 */
export function storedPart(
  bytes: Uint8Array,
  entry: ZipEntry,
  most: number,
): Uint8Array | undefined {
  if (entry.method !== STORED || entry.size !== entry.compressedSize || entry.size > most) {
    return undefined;
  }
  const at = entry.offset;
  if (at + LOCAL_SIZE > bytes.byteLength) return undefined;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(at, true) !== LOCAL_SIGNATURE) return undefined;
  const start = at + LOCAL_SIZE + view.getUint16(at + 26, true) + view.getUint16(at + 28, true);
  return start + entry.size <= bytes.byteLength
    ? bytes.subarray(start, start + entry.size)
    : undefined;
}
