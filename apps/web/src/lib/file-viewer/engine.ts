/**
 * rhwp's WebAssembly, the engine that reads Hangul (HWP, HWPX) documents:
 * fetched by the app the first time one is opened, and handed to the frame
 * that shows or edits it together with the document.
 *
 * The frames cannot fetch it: their policy keeps them off the network
 * (static/viewer-frame.html). The app fetches it from among its own files
 * (`virtual:rhwp-wasm`, tools/build/rhwp-wasm.ts) and keeps the bytes for
 * the rest of the visit; each frame gets a copy, which it passes on to
 * whatever runs the engine. The app itself never runs them.
 *
 * A fetch that fails, or is not done within `ENGINE_FETCH_LIMIT`, is given up
 * and not kept: the next document asks again. So is an answer that is not
 * WebAssembly (a gate in front of the server answers with its login page).
 */
import address from 'virtual:rhwp-wasm';

/** How long the fetch may take, in milliseconds: 7 MB on the wire, the first time in a visit. */
export const ENGINE_FETCH_LIMIT = 120_000;

/** What every WebAssembly module starts with: `\0asm`. */
const MAGIC = [0x00, 0x61, 0x73, 0x6d];

let fetched: Promise<ArrayBuffer> | undefined;

async function fetchEngine(): Promise<ArrayBuffer> {
  const late = new AbortController();
  const timer = setTimeout(() => {
    late.abort();
  }, ENGINE_FETCH_LIMIT);
  try {
    // eslint-disable-next-line fvoci/no-direct-server -- a file of the build, not the server's data
    const response = await fetch(address, { credentials: 'omit', signal: late.signal });
    if (!response.ok) throw new Error(`the engine was not served: ${String(response.status)}`);
    const bytes = await response.arrayBuffer();
    const start = new Uint8Array(bytes, 0, Math.min(MAGIC.length, bytes.byteLength));
    if (!MAGIC.every((byte, index) => start[index] === byte)) {
      throw new Error('what was served as the engine is not WebAssembly');
    }
    return bytes;
  } finally {
    clearTimeout(timer);
  }
}

/** A copy of the engine's bytes, to hand over; rejects when they could not be fetched. */
export async function engineBytes(): Promise<ArrayBuffer> {
  fetched ??= fetchEngine().catch((error: unknown) => {
    fetched = undefined;
    throw error;
  });
  return (await fetched).slice(0);
}
