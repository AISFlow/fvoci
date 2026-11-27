/**
 * What the frame does with a compound file (HWP) before its renderer reads
 * it, in the renderer's worker: checks that each stream has the sectors it
 * says and that no sector is read as two streams, reads its `FileHeader` for
 * a password, holds what its streams unpack to, together, to the limit, and
 * writes the file anew of the streams it counted (compound-writer.ts). rhwp
 * reads a file it would refuse (and then read leniently, by rules of its
 * own) otherwise than this does; the file it gets is one it takes as written.
 *
 * The streams of an HWP document are deflated inside (the records of its
 * text, its pictures). rhwp (0.9.0) unpacks them itself: the text (DocInfo,
 * BodyText, ViewText) within a budget of its own (256 MiB a stream, 512 MiB
 * in all), and each picture (BinData) when a page first needs it, with no
 * budget for them together and, on some of its paths, none for one; a file
 * it reads leniently (one its compound file reader refuses) the same. So what every stream
 * unpacks to is counted here first, as rhwp unpacks it (raw deflate, then
 * zlib, and as it came when neither reads it), by reading its codes without
 * unpacking it (deflated-size.ts). What comes out before a stream fails to
 * unpack counts as well: rhwp may have taken it, and the memory it took stays.
 */
import {
  childrenOf,
  COMPOUND_TYPES,
  compoundFile,
  readStream,
  rootStream,
  verifyChains,
  type CompoundEntry,
  type CompoundFile,
} from '$lib/file-viewer/compound-file.ts';
import { HWP_SIGNATURE, UNPACKED_LIMIT_BYTES } from '$lib/file-viewer/formats.ts';

import { writeCompound, type CompoundPart } from './compound-writer.ts';
import { largerDeflatedSize } from './deflated-size.ts';
import { LockedFile, TooLargeFile, UnreadableFile } from './renderer.ts';

/** The signature (32 bytes), the version (4) and the properties (4) of `FileHeader`. */
const HEAD_SIZE = 40;
/** The property that says the document is locked with a password. */
const PASSWORD = 1 << 1;
/**
 * The property that says the document is a distribution document (배포용 문서), which
 * Hangul keeps from being copied and printed: its text is in `ViewText`, encrypted.
 */
const DISTRIBUTION = 1 << 2;
/** Where the deflated bytes start as rhwp tries them: raw, then after a zlib header. */
const RAW_START = 0;
const ZLIB_START = 2;

const ascii = new TextDecoder('ascii');

/**
 * The most a stream may unpack to as rhwp unpacks it, counted as far as
 * `room`. Each way is counted, whichever rhwp keeps: the count of one is a
 * bound on what rhwp unpacks that way, not a sign that rhwp took it.
 */
function unpackedSize(data: Uint8Array, room: number): number {
  if (data.byteLength > room) return data.byteLength;
  return Math.max(data.byteLength, largerDeflatedSize(data, RAW_START, ZLIB_START, room));
}

/**
 * The streams of the directory's tree, each with the names on its way from
 * the root; undefined when an entry is reached twice or a storage holds a
 * name twice (the format compares names ignoring case).
 */
export function streamsOf(
  file: CompoundFile,
): { names: string[]; entry: CompoundEntry }[] | undefined {
  const [root] = file.entries;
  if (!root) return undefined;
  const found: { names: string[]; entry: CompoundEntry }[] = [];
  const seen = new Set<CompoundEntry>([root]);
  const pending = [{ storage: root, names: [] as string[] }];
  for (let next = pending.pop(); next; next = pending.pop()) {
    const taken = new Set<string>();
    for (const entry of childrenOf(file, next.storage)) {
      const name = entry.name.toUpperCase();
      if (seen.has(entry) || taken.has(name)) return undefined;
      seen.add(entry);
      taken.add(name);
      const names = [...next.names, entry.name];
      if (entry.type === COMPOUND_TYPES.STREAM) found.push({ names, entry });
      if (entry.type === COMPOUND_TYPES.STORAGE) pending.push({ storage: entry, names });
    }
  }
  return found;
}

/**
 * The streams, read and counted as they unpack; throws `TooLargeFile` when
 * they unpack to more than `limit` together.
 */
function counted(file: CompoundFile, limit: number): CompoundPart[] {
  const streams = streamsOf(file);
  if (!streams) throw new UnreadableFile();
  const parts: CompoundPart[] = [];
  let total = 0;
  for (const { names, entry } of streams) {
    const data = readStream(file, entry, entry.size);
    if (!data) throw new UnreadableFile();
    total += unpackedSize(data, limit - total);
    if (total > limit) throw new TooLargeFile();
    parts.push({ names, data });
  }
  return parts;
}

/** The properties `FileHeader` gives the document; throws `UnreadableFile` when it is no HWP's. */
export function headerProperties(file: CompoundFile): number {
  const header = rootStream(file, 'FileHeader');
  const head = header && readStream(file, header, HEAD_SIZE);
  if (head?.length !== HEAD_SIZE) throw new UnreadableFile();
  if (ascii.decode(head.subarray(0, HWP_SIGNATURE.length)) !== HWP_SIGNATURE) {
    throw new UnreadableFile();
  }
  return new DataView(head.buffer, head.byteOffset, head.byteLength).getUint32(36, true);
}

/** Whether the properties `FileHeader` gives say the document is a distribution document. */
export const isDistribution = (properties: number): boolean => (properties & DISTRIBUTION) !== 0;

/**
 * The file written anew of its streams, once checked; throws
 * `UnreadableFile`, `LockedFile` or `TooLargeFile` otherwise. `limit` is what
 * the streams may unpack to.
 */
export function checkCompound(buffer: ArrayBuffer, limit = UNPACKED_LIMIT_BYTES): ArrayBuffer {
  const file = compoundFile(new Uint8Array(buffer));
  if (!file || !verifyChains(file)) throw new UnreadableFile();
  if (headerProperties(file) & PASSWORD) throw new LockedFile();
  const parts = counted(file, limit);
  try {
    return writeCompound(parts).buffer;
  } catch {
    // A name the format cannot hold, or one a storage holds twice.
    throw new UnreadableFile();
  }
}
