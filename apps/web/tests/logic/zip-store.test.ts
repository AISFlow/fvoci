/** A ZIP stored without compression (`storedZip`), read back as any reader would. */
import { describe, expect, it } from 'vitest';

import { crc32, storedZip } from '$lib/logic/zip-store.ts';

/** The files of a stored ZIP, by its central directory, as a reader finds them. */
function readBack(zip: Uint8Array): Record<string, string> {
  const view = new DataView(zip.buffer, zip.byteOffset, zip.byteLength);
  const end = zip.length - 22;
  expect(view.getUint32(end, true)).toBe(0x06054b50);
  const count = view.getUint16(end + 10, true);
  let at = view.getUint32(end + 16, true);
  const files: Record<string, string> = {};
  const decoder = new TextDecoder();
  for (let index = 0; index < count; index += 1) {
    expect(view.getUint32(at, true)).toBe(0x02014b50);
    const size = view.getUint32(at + 24, true);
    const nameLength = view.getUint16(at + 28, true);
    const local = view.getUint32(at + 42, true);
    const name = decoder.decode(zip.subarray(at + 46, at + 46 + nameLength));
    expect(view.getUint32(local, true)).toBe(0x04034b50);
    expect(view.getUint16(local + 8, true)).toBe(0);
    const start = local + 30 + view.getUint16(local + 26, true);
    const bytes = zip.subarray(start, start + size);
    expect(crc32(bytes)).toBe(view.getUint32(at + 16, true));
    files[name] = decoder.decode(bytes);
    at += 46 + nameLength;
  }
  return files;
}

describe('a stored ZIP', () => {
  it('has the CRC-32 that ZIP has', () => {
    expect(crc32(new TextEncoder().encode('hello'))).toBe(0x3610a686);
    expect(crc32(new Uint8Array())).toBe(0);
  });

  it('holds its files, named in UTF-8, as they were', () => {
    const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
    const zip = storedZip([
      { name: '위키/첫 페이지.md', bytes: encode('# 첫\n') },
      { name: 'Wiki/empty.md', bytes: new Uint8Array() },
    ]);
    expect(readBack(zip)).toEqual({ '위키/첫 페이지.md': '# 첫\n', 'Wiki/empty.md': '' });
    expect(readBack(storedZip([]))).toEqual({});
  });
});
