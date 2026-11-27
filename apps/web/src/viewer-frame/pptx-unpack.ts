/**
 * Unpacks the archive checked and rewritten of a presentation's parts
 * (`zip-guard.ts` `repack`), in the PowerPoint worker: each part's bytes by
 * its name. That archive holds only parts counted under their true sizes,
 * stored or deflated, so what is unpacked here is what was held to the limits;
 * a part that unpacks past its size stops the file all the same.
 */
import { zipEntries, type ZipEntry } from '$lib/file-viewer/zip-directory.ts';

import { UnreadableFile } from './renderer.ts';

const LOCAL_SIGNATURE = 0x04034b50;
const LOCAL_SIZE = 30;
const STORED = 0;
const DEFLATED = 8;

function packedBytes(bytes: Uint8Array<ArrayBuffer>, entry: ZipEntry): Uint8Array<ArrayBuffer> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const at = entry.offset;
  if (at + LOCAL_SIZE > bytes.length || view.getUint32(at, true) !== LOCAL_SIGNATURE) {
    throw new UnreadableFile();
  }
  const data = at + LOCAL_SIZE + view.getUint16(at + 26, true) + view.getUint16(at + 28, true);
  if (data + entry.compressedSize > bytes.length) throw new UnreadableFile();
  return bytes.subarray(data, data + entry.compressedSize);
}

async function inflate(packed: Uint8Array<ArrayBuffer>, size: number): Promise<Uint8Array> {
  const source = new ReadableStream<BufferSource>({
    start(controller) {
      controller.enqueue(packed);
      controller.close();
    },
  });
  const stream = source.pipeThrough(new DecompressionStream('deflate-raw'));
  const out = new Uint8Array(size);
  let at = 0;
  const reader = stream.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (at + value.length > size) {
      await reader.cancel();
      throw new UnreadableFile();
    }
    out.set(value, at);
    at += value.length;
  }
  return at === size ? out : out.subarray(0, at);
}

/** Each part of the archive by its name; throws `UnreadableFile` when the bytes are no such archive. */
export async function unpack(buffer: ArrayBuffer): Promise<Map<string, Uint8Array>> {
  const bytes = new Uint8Array(buffer);
  const entries = zipEntries(bytes);
  if (!entries) throw new UnreadableFile();
  const parts = new Map<string, Uint8Array>();
  for (const entry of entries) {
    if (entry.name.endsWith('/')) continue;
    const packed = packedBytes(bytes, entry);
    if (entry.method === STORED) parts.set(entry.name, packed);
    else if (entry.method === DEFLATED) {
      try {
        parts.set(entry.name, await inflate(packed, entry.size));
      } catch {
        throw new UnreadableFile();
      }
    } else throw new UnreadableFile();
  }
  return parts;
}
