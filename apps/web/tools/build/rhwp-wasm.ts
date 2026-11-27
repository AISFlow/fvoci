/**
 * rhwp's WebAssembly, the engine of the HWP renderer and of the HWP editor,
 * as the app hands it to their frames.
 *
 * The frames cannot fetch it, as they cannot fetch anything
 * (static/viewer-frame.html). The app can: the build puts the bytes of
 * `@rhwp/core`'s `rhwp_bg.wasm` among its files (`rhwp-wasm-<hash>.wasm`),
 * the module `virtual:rhwp-wasm` gives that file's address, and the app
 * fetches it the first time a Hangul document is opened and hands a copy to
 * each frame with the document (src/lib/file-viewer/engine.ts).
 *
 * It was a module of the frame's own bundle before, its bytes as hexadecimal
 * text. At rhwp 0.9.0 the file is 26.8 MB, and that text 53.7 MB: a frame
 * took four seconds to read the module and a second and a half more to pass
 * the text on to its worker, for every document opened. Fetched as bytes it
 * is a third of a second, once a visit, and 7.0 MB on the wire (Brotli)
 * against 7.8 MB.
 *
 * Its memory is given a maximum (`RHWP_MEMORY_LIMIT_BYTES`, wasm-memory.ts):
 * past it rhwp traps, and the worker it runs in is stopped, rather than a
 * file growing it until the tab's process runs out. A page with the largest
 * picture a file the viewer takes can hold needs about 650 MB (measured at
 * rhwp 0.8.7; tests/logic/hwp-memory.test.ts holds at 0.9.0).
 *
 * The package's own loader would fetch the file from beside itself when it is
 * given nothing to load (`new URL('rhwp_bg.wasm', import.meta.url)`), and
 * that address alone makes the build copy the file as it is, without the
 * maximum, into the output. The loader is always given the bytes, so that
 * branch is cut out of it; the build fails if the loader no longer reads as
 * expected.
 */
import { readFileSync, realpathSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

import type { Plugin } from 'vite';

import { withMemoryLimit } from './wasm-memory.ts';

const ID = 'virtual:rhwp-wasm';
const RESOLVED = `\0${ID}`;
/** The file's name in the output, before its hash, and its address where it is served instead. */
export const ASSET_NAME = 'rhwp-wasm.wasm';
const SERVED = '/@fvoci/rhwp-wasm.wasm';

/** The loader's fallback to an address beside it, in `rhwp.js`. */
const DEFAULT_ADDRESS = "module_or_path = new URL('rhwp_bg.wasm', import.meta.url);";

/** The most memory rhwp's WebAssembly may take, in bytes. */
export const RHWP_MEMORY_LIMIT_BYTES = 1024 * 1024 * 1024;

/** The folder of the installed `@rhwp/core`. */
export function rhwpDirectory(): string {
  const require = createRequire(import.meta.url);
  return realpathSync(path.dirname(require.resolve('@rhwp/core')));
}

/** The file of rhwp's WebAssembly, as the package has it. */
export const rhwpWasmFile = (): string => path.join(rhwpDirectory(), 'rhwp_bg.wasm');

/** rhwp's WebAssembly as the frames get it: its memory held to the limit. */
export function rhwpWasmBytes(limit = RHWP_MEMORY_LIMIT_BYTES): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(withMemoryLimit(readFileSync(rhwpWasmFile()), limit));
}

export function rhwpWasm(): Plugin {
  /** Whether the output is written (a build) or served (the dev server, the tests). */
  let building = false;
  return {
    name: 'fvoci:rhwp-wasm',
    configResolved(config) {
      building = config.command === 'build';
    },
    // Served, the file has an address of its own: there is no output to put it in.
    configureServer(server) {
      server.middlewares.use(SERVED, (_request, response) => {
        response.setHeader('Content-Type', 'application/wasm');
        response.end(rhwpWasmBytes());
      });
    },
    resolveId(id) {
      return id === ID ? RESOLVED : undefined;
    },
    load(id) {
      if (id !== RESOLVED) return undefined;
      this.addWatchFile(rhwpWasmFile());
      if (!building) return { code: `export default ${JSON.stringify(SERVED)};` };
      const file = this.emitFile({ type: 'asset', name: ASSET_NAME, source: rhwpWasmBytes() });
      return {
        code: `export default import.meta.ROLLUP_FILE_URL_${file};`,
        moduleSideEffects: false,
      };
    },
    transform(code, id) {
      if (path.basename(id.split('?')[0] ?? '') !== 'rhwp.js' || !id.includes('@rhwp')) {
        return undefined;
      }
      if (!code.includes(DEFAULT_ADDRESS)) {
        this.error(
          `@rhwp/core: rhwp.js no longer reads "${DEFAULT_ADDRESS}"; see ${import.meta.filename}`,
        );
      }
      const replaced = code.replace(
        DEFAULT_ADDRESS,
        "throw new Error('rhwp: the WebAssembly is given to init, never fetched');",
      );
      return { code: replaced, map: null };
    },
  };
}
