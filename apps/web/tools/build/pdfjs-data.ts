/**
 * The module `virtual:pdfjs-data`: the data files of pdf.js that the file
 * viewer's frame may ask for (src/viewer-frame/pdf-data.ts), each in a module
 * of its own that holds its bytes in Base64 and is loaded only when pdf.js
 * asks for that file.
 *
 *   export const cMaps: Record<string, () => Promise<string>>;    // by file name
 *   export const decoders: Record<string, () => Promise<string>>;
 *
 * Made here because neither `import.meta.glob` (it follows the package
 * manager's links and then names the files by a path that does not lead to
 * them) nor `?inline` (it takes only the kinds of file Vite knows) can.
 */
import { readdirSync, readFileSync, realpathSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

import type { Plugin } from 'vite';

const ID = 'virtual:pdfjs-data';
const RESOLVED = `\0${ID}`;
/**
 * One file's module: `virtual:pdfjs-data/<folder>/<file name>.js`. The `.js`
 * keeps Vite's own handling of `.wasm` files off it.
 */
const FILE_PREFIX = `${RESOLVED}/`;

/** The WebAssembly decoders pdf.js reads images and colour with; not its script engine (scripting is off). */
export const DECODERS = ['jbig2.wasm', 'openjpeg.wasm', 'qcms_bg.wasm'];
const FOLDERS = ['cmaps', 'wasm'];

function packageDirectory(): string {
  const require = createRequire(import.meta.url);
  // `exports` hides package.json; the main module lies in `build/`.
  return realpathSync(path.resolve(path.dirname(require.resolve('pdfjs-dist')), '..'));
}

function table(folder: string, names: readonly string[]): string {
  const lines = names.map((name) => {
    const id = JSON.stringify(`${ID}/${folder}/${name}.js`);
    return `  ${JSON.stringify(name)}: () => import(${id}).then((module) => module.default),`;
  });
  return `{\n${lines.join('\n')}\n}`;
}

export function pdfjsData(): Plugin {
  return {
    name: 'fvoci:pdfjs-data',
    resolveId(id) {
      if (id === ID) return RESOLVED;
      return id.startsWith(`${ID}/`) ? `\0${id}` : undefined;
    },
    load(id) {
      const root = packageDirectory();
      if (id === RESOLVED) {
        const cMaps = readdirSync(path.join(root, 'cmaps'))
          .filter((name) => name.endsWith('.bcmap'))
          .sort();
        return [
          `export const cMaps = ${table('cmaps', cMaps)};`,
          `export const decoders = ${table('wasm', DECODERS)};`,
        ].join('\n');
      }
      if (!id.startsWith(FILE_PREFIX)) return undefined;
      const [folder = '', module = ''] = id.slice(FILE_PREFIX.length).split('/');
      const name = module.replace(/\.js$/, '');
      if (!FOLDERS.includes(folder) || name !== path.basename(name)) return undefined;
      const file = path.join(root, folder, name);
      this.addWatchFile(file);
      return {
        code: `export default ${JSON.stringify(readFileSync(file).toString('base64'))};`,
        moduleSideEffects: false,
      };
    },
  };
}
