/**
 * A WebAssembly module's memory given a maximum (src/viewer-frame/hwp.ts):
 * past it, growing the memory fails, and the module's allocator traps
 * (`WebAssembly.RuntimeError`) instead of taking the memory of the tab, whose
 * process a sandboxed frame and its workers share.
 *
 * The module must define one memory of its own with a minimum alone, as
 * wasm-bindgen writes it; anything else fails the build rather than going out
 * without the maximum.
 */

const MAGIC = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
const MEMORY_SECTION = 5;
/** A memory's limits: a minimum alone, or a minimum and a maximum. */
const MINIMUM_ONLY = 0x00;
const WITH_MAXIMUM = 0x01;
/** The size of a page of WebAssembly memory, in bytes. */
export const WASM_PAGE = 65_536;

function leb(value: number): number[] {
  const out: number[] = [];
  let rest = value;
  do {
    const low = rest % 128;
    rest = Math.floor(rest / 128);
    out.push(rest > 0 ? low | 0x80 : low);
  } while (rest > 0);
  return out;
}

/** An unsigned LEB128 number at `at`, and where what follows it starts. */
function readLeb(bytes: Uint8Array, at: number): { value: number; next: number } {
  let value = 0;
  let scale = 1;
  let next = at;
  for (;;) {
    const byte = bytes[next];
    if (byte === undefined || scale > 2 ** 35) throw new Error('wasm-memory: a number runs out');
    next += 1;
    value += (byte & 0x7f) * scale;
    scale *= 128;
    if ((byte & 0x80) === 0) return { value, next };
  }
}

/** The memory's limits, as the payload of its section; throws on any other kind. */
function limitsOf(module: Uint8Array, start: number, end: number): { minimum: number } {
  const count = readLeb(module, start);
  const flags = module[count.next];
  if (count.value !== 1 || flags !== MINIMUM_ONLY) {
    throw new Error('wasm-memory: the module does not define one memory with a minimum alone');
  }
  const minimum = readLeb(module, count.next + 1);
  if (minimum.next !== end) throw new Error('wasm-memory: the memory section has more in it');
  return { minimum: minimum.value };
}

/** The module with its memory held to `limit` bytes (whole pages, rounded down). */
export function withMemoryLimit(module: Uint8Array, limit: number): Uint8Array {
  if (!MAGIC.every((byte, index) => module[index] === byte)) {
    throw new Error('wasm-memory: not a WebAssembly module of version 1');
  }
  const pages = Math.floor(limit / WASM_PAGE);
  let at = MAGIC.length;
  while (at < module.length) {
    const size = readLeb(module, at + 1);
    const end = size.next + size.value;
    if (module[at] === MEMORY_SECTION) {
      const { minimum } = limitsOf(module, size.next, end);
      if (minimum > pages)
        throw new Error('wasm-memory: the limit is under what the module starts with');
      const payload = [1, WITH_MAXIMUM, ...leb(minimum), ...leb(pages)];
      const section = Uint8Array.from([MEMORY_SECTION, ...leb(payload.length), ...payload]);
      const out = new Uint8Array(at + section.length + (module.length - end));
      out.set(module.subarray(0, at));
      out.set(section, at);
      out.set(module.subarray(end), at + section.length);
      return out;
    }
    at = end;
  }
  throw new Error('wasm-memory: the module defines no memory');
}
