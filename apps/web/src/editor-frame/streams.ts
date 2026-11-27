/**
 * The streams of an HWP document that a save leaves out, which rhwp's report
 * of the save does not name, and whether the save takes a distribution
 * document's protection away.
 *
 * An HWP is a compound file: streams in storages (folders), compared here by
 * their paths, ignoring case as the format does. rhwp writes the streams it
 * reads: one it has no use for is gone from the saved file, with a report of
 * no losses. So the frame compares the document as it was opened with the
 * saved one, as it compares an HWPX's parts (parts.ts), and what is missing is
 * told as a loss, which the writer is asked about as about rhwp's own.
 *
 * - A stream under `BinData/` is found by what it holds (its size and CRC-32):
 *   rhwp may number the pictures and other data anew.
 * - Every other stream is found by its path. An empty stream holds nothing to
 *   lose.
 * - `PrvImage` and `PrvText`, the preview Hangul shows of a file before it is
 *   opened, are told as what Hangul makes again when it saves the file.
 * - A distribution document (배포용 문서: Hangul keeps it from being copied and
 *   printed) has its text in `ViewText`, encrypted. rhwp saves that text as
 *   any document's, in `BodyText`, and the saved file is no distribution
 *   document any more: that is told once, as the protection taken away, and
 *   not again as `ViewText`'s streams left out.
 *
 * This finds streams left out, not streams kept under their path and written
 * otherwise: what rhwp writes of what it reads is its report's to say. Only
 * paths, sizes and checksums are kept of the document opened, not its bytes.
 */
import { compoundFile, readStream } from '$lib/file-viewer/compound-file.ts';
import { LOSS_PATH_LIMIT, type Loss } from '$lib/file-viewer/editor-protocol.ts';

import { headerProperties, isDistribution, streamsOf } from '../viewer-frame/compound-guard.ts';

const BINARY = 'BINDATA/';
const VIEW_TEXT = 'VIEWTEXT/';
/** The preview of the file that Hangul writes as it saves one. */
const PREVIEWS = new Set(['PRVIMAGE', 'PRVTEXT']);

interface Stream {
  /** Its path, as the file names it: `BinData/BIN0001.jpg`. */
  path: string;
  /** Its path as it is compared: in capitals. */
  key: string;
  size: number;
  /** What a stream under `BinData/` holds, as it is compared: its size and CRC-32. */
  content?: string;
}

/** What is kept of an HWP document to compare a save with. */
export interface HwpStreams {
  distribution: boolean;
  streams: readonly Stream[];
}

const CRC_TABLE = Uint32Array.from({ length: 256 }, (_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit += 1) value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  return value;
});

function crc32(data: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of data) crc = (CRC_TABLE[(crc ^ byte) & 0xff] ?? 0) ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

/** What an HWP document holds, as compared. Throws on bytes that are no HWP this reads. */
export function hwpStreams(bytes: Uint8Array): HwpStreams {
  const file = compoundFile(bytes);
  const found = file && streamsOf(file);
  if (!file || !found) throw new Error('not a compound file');
  const streams = found.map(({ names, entry }): Stream => {
    const path = names.join('/');
    const key = path.toUpperCase();
    if (!key.startsWith(BINARY)) return { path, key, size: entry.size };
    const data = readStream(file, entry, entry.size);
    if (!data) throw new Error('a stream does not read');
    return { path, key, size: entry.size, content: `${String(entry.size)}:${String(crc32(data))}` };
  });
  // In the order of their paths: the directory's own order is a tree's.
  streams.sort((one, other) => (one.key < other.key ? -1 : one.key > other.key ? 1 : 0));
  return { distribution: isDistribution(headerProperties(file)), streams };
}

const loss = (stream: Stream): Loss => ({
  code: 'partOmitted',
  subject: 'stream',
  reason: PREVIEWS.has(stream.key) ? 'previewMadeAgain' : 'notKeptByEditor',
  path: stream.path.slice(0, LOSS_PATH_LIMIT),
});

/**
 * What a save (`kept`, read of the saved file) leaves out of the document
 * `given` was read from: whether it takes the protection of a distribution
 * document away (`unprotected`), and the streams that are not in the saved
 * file, each as a loss.
 */
export function streamsLeftOut(
  given: HwpStreams,
  kept: HwpStreams,
): { unprotected: boolean; losses: Loss[] } {
  const unprotected =
    given.distribution &&
    (!kept.distribution || !kept.streams.some((stream) => stream.key.startsWith(VIEW_TEXT)));
  const paths = new Set(kept.streams.map((stream) => stream.key));
  // How many of each content the saved file's data holds, taken as they are found again.
  const data = new Map<string, number>();
  for (const { content } of kept.streams) {
    if (content !== undefined) data.set(content, (data.get(content) ?? 0) + 1);
  }
  const losses = given.streams.flatMap((stream): Loss[] => {
    if (stream.size === 0 || (unprotected && stream.key.startsWith(VIEW_TEXT))) return [];
    if (stream.content === undefined) return paths.has(stream.key) ? [] : [loss(stream)];
    const left = data.get(stream.content) ?? 0;
    if (left === 0) return [loss(stream)];
    data.set(stream.content, left - 1);
    return [];
  });
  return { unprotected, losses };
}
