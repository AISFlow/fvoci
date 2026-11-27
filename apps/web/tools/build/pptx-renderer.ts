/**
 * The PowerPoint renderer of the file viewer (@aiden0z/pptx-renderer), as
 * the build takes it.
 *
 * Its bundle carries the decoder of embedded fonts, mtx-decompressor, inside
 * it: MPL-2.0 code, a licence the project's list does not allow, and which
 * Vite's licence report cannot see (the package is not imported, it is in
 * the file). The viewer draws no embedded fonts (the worker leaves them out,
 * src/viewer-frame/pptx-parts.ts), so the build cuts the decoder out of the
 * renderer's bundle and leaves in its place a function that refuses. The
 * cut is found by the text that begins and ends it, in the version pinned
 * in package.json: if either is not there exactly once the build fails,
 * and it fails as well if any of the decoder's own text is in the output.
 *
 * The chart library the renderer imports (echarts) is replaced by a stub
 * (the alias in vite.config.ts, src/viewer-frame/no-echarts.ts): charts are
 * not drawn. So is its ZIP library (JSZip), for the renderer alone
 * (src/viewer-frame/no-jszip.ts): the worker reads the archive.
 */
import path from 'node:path';

import type { Plugin } from 'vite';

const BUNDLE = /@aiden0z[/\\]pptx-renderer[/\\]dist[/\\]aiden0z-pptx-renderer\.es\.js$/;
/** Where mtx-decompressor begins: its bit stream, `Stream` in its source. */
const START = 'var hi = class Lc {\n  // current bit position within the byte at `pos`';
/** Where it ends: what follows its `decompressMtx` (named `mv` in the bundle). */
const END = '\nconst pv = 20556, hv = 4, fv = 268435456, Ec = Object.freeze({';
/** What stands in for `decompressMtx`. */
const STUB = 'function mv() {\n  throw new Error("embedded fonts are not drawn in this build");\n}';
const NO_ZIP = path.resolve(import.meta.dirname, '../../src/viewer-frame/no-jszip.ts');
/** Text of the decoder that must not reach the output. */
const DECODER_TEXT = ['LZCOMP', 'MTX data', 'MTX header', 'MTX_BITIO'];

function once(code: string, text: string): number {
  const at = code.indexOf(text);
  return at >= 0 && !code.includes(text, at + 1) ? at : -1;
}

export function pptxRenderer(): Plugin {
  return {
    name: 'fvoci:pptx-renderer',
    enforce: 'pre',
    resolveId(source, importer) {
      if (source !== 'jszip' || importer === undefined) return undefined;
      return BUNDLE.test(importer.split('?')[0] ?? '') ? NO_ZIP : undefined;
    },
    transform(code, id) {
      if (!BUNDLE.test(id.split('?')[0] ?? '')) return undefined;
      const start = once(code, START);
      const end = once(code, END);
      if (start < 0 || end < start) {
        this.error(
          'the font decoder in @aiden0z/pptx-renderer is not where tools/build/pptx-renderer.ts looks for it: was the package updated?',
        );
      }
      return { code: `${code.slice(0, start)}${STUB}${code.slice(end)}`, map: null };
    },
    generateBundle(_options, bundle) {
      for (const item of Object.values(bundle)) {
        if (item.type !== 'chunk') continue;
        const found = DECODER_TEXT.find((text) => item.code.includes(text));
        if (found) {
          this.error(`${item.fileName} has the embedded-font decoder in it ("${found}")`);
        }
      }
    },
  };
}
