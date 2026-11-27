import { describe, expect, it } from 'vitest';

import { rhwpWasmBytes, RHWP_MEMORY_LIMIT_BYTES } from '../../tools/build/rhwp-wasm.ts';
import { WASM_PAGE, withMemoryLimit } from '../../tools/build/wasm-memory.ts';

/** A module of one memory of `pages` pages at least, exported as `memory`. */
const moduleOf = (pages: number): Uint8Array =>
  Uint8Array.from([
    ...[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
    ...[0x05, 0x03, 0x01, 0x00, pages],
    ...[0x07, 0x0a, 0x01, 0x06, ...Buffer.from('memory'), 0x02, 0x00],
  ]);

const memoryOf = async (module: Uint8Array): Promise<WebAssembly.Memory> => {
  const { instance } = await WebAssembly.instantiate(Uint8Array.from(module));
  return instance.exports.memory as WebAssembly.Memory;
};

describe("a WebAssembly module's memory given a maximum", () => {
  it('grows up to it and no further', async () => {
    const memory = await memoryOf(withMemoryLimit(moduleOf(2), 4 * WASM_PAGE));
    expect(memory.buffer.byteLength).toBe(2 * WASM_PAGE);
    memory.grow(2);
    expect(() => memory.grow(1)).toThrow(RangeError);
    const free = await memoryOf(moduleOf(2));
    expect(() => free.grow(3)).not.toThrow();
  });

  it('refuses a module it cannot hold: a maximum already there, or a limit under the minimum', () => {
    const held = withMemoryLimit(moduleOf(2), 4 * WASM_PAGE);
    expect(() => withMemoryLimit(held, 8 * WASM_PAGE)).toThrow(/minimum alone/);
    expect(() => withMemoryLimit(moduleOf(8), 4 * WASM_PAGE)).toThrow(/under what the module/);
    expect(() => withMemoryLimit(Uint8Array.from([1, 2, 3]), WASM_PAGE)).toThrow(/version 1/);
  });

  it("rhwp's, as the build hands it to the frame, may take 1 GiB", () => {
    const pages = RHWP_MEMORY_LIMIT_BYTES / WASM_PAGE;
    expect(pages).toBe(16_384);
    // The memory section: one memory, a minimum (79 pages in rhwp 0.9.0) and 16384 at most.
    expect(Buffer.from(rhwpWasmBytes()).toString('hex')).toContain('050601014f808001');
  });
});
