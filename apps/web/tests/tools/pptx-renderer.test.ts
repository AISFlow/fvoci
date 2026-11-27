/**
 * The PowerPoint renderer as the build takes it: its embedded-font decoder
 * (MPL-2.0) cut out of its bundle (tools/build/pptx-renderer.ts).
 */
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { pptxRenderer } from '../../tools/build/pptx-renderer.ts';

const require = createRequire(path.resolve(import.meta.dirname, '../../package.json'));
const bundle = require.resolve('@aiden0z/pptx-renderer');
const esBundle = path.join(path.dirname(bundle), 'aiden0z-pptx-renderer.es.js');

type Transform = (code: string, id: string) => { code: string } | undefined;

function transform(code: string, id: string, errors: string[]): { code: string } | undefined {
  const plugin = pptxRenderer();
  const hook = plugin.transform as unknown as Transform;
  return hook.call(
    {
      error: (message: string) => {
        errors.push(message);
        throw new Error(message);
      },
    },
    code,
    id,
  );
}

describe("the PowerPoint renderer's bundle", () => {
  const code = readFileSync(esBundle, 'utf8');

  it('has the decoder in it as published', () => {
    expect(code).toContain('LZCOMP');
  });

  it('loses the decoder, and nothing else refers to what was cut', () => {
    const result = transform(code, esBundle, []);
    if (!result) throw new Error('not transformed');
    for (const text of ['LZCOMP', 'MTX data', 'MTX header', 'MTX_BITIO', 'class Lc']) {
      expect(result.code).not.toContain(text);
    }
    expect(result.code).toContain('embedded fonts are not drawn in this build');
    // The bit stream class was used by the decoder alone.
    expect(result.code).not.toMatch(/new hi\(/);
    expect(result.code.length).toBeLessThan(code.length - 20_000);
  });

  it('fails the build when the decoder is not where it is looked for', () => {
    const errors: string[] = [];
    expect(() => transform('export {};', esBundle, errors)).toThrow();
    expect(errors[0]).toMatch(/was the package updated/);
  });

  it("puts a stub in place of the renderer's ZIP library, and only the renderer's", () => {
    const hook = pptxRenderer().resolveId as unknown as (
      source: string,
      importer?: string,
    ) => unknown;
    expect(hook('jszip', esBundle)).toMatch(/src[/\\]viewer-frame[/\\]no-jszip\.ts$/);
    expect(hook('jszip', '/node_modules/docx-preview/dist/docx-preview.mjs')).toBeUndefined();
    expect(hook('pako', esBundle)).toBeUndefined();
  });

  it('leaves other modules alone', () => {
    expect(transform('var hi = 1;', '/somewhere/else.js', [])).toBeUndefined();
  });
});
