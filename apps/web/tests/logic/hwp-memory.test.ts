/**
 * rhwp held to the memory it may take (tools/build/wasm-memory.ts), in a file
 * of its own: rhwp's loader keeps the first instance it is given in a module
 * context, and an instance that trapped is left as it was mid-way. Here that
 * first instance may take 64 MiB, so that a small picture is past it; the
 * build's own limit is tried in the browser (e2e/hwp-viewer.spec.ts).
 */
import { randomBytes } from 'node:crypto';

import { initSync } from '@rhwp/core';
import { describe, expect, it } from 'vitest';

import { HwpReader } from '../../src/viewer-frame/hwp-document.ts';

import { koreanHwpx, rewriteArchive } from '../support/hwp-files.ts';
import { rhwpWasmBytes } from '../../tools/build/rhwp-wasm.ts';

const WASM = rhwpWasmBytes(64 << 20).buffer;
initSync({ module: WASM });

/** A picture of 8 MiB that does not compress, which rhwp writes into the page whole. */
const PICTURE = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  randomBytes(8 << 20),
]);

describe('rhwp past the memory it may take', () => {
  it('traps, and the reader lets the trap through rather than answer', async () => {
    const file = rewriteArchive(koreanHwpx(), (member) =>
      member.name === 'BinData/image1.png' ? { ...member, data: PICTURE, stored: true } : member,
    );
    const reader = new HwpReader();
    const asked = async (): Promise<string> => {
      const bytes = Uint8Array.from(file).buffer;
      await reader.answer({ kind: 'open', format: 'hwpx', bytes, wasm: WASM });
      return (await reader.answer({ kind: 'page', page: 1 })).kind;
    };
    await expect(asked()).rejects.toBeInstanceOf(WebAssembly.RuntimeError);
  });
});
