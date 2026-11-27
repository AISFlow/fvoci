/** The raw-colour gate (`tools/lint/colors.ts`): what it refuses, and where colours live. */
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { COLOR_HOMES, checkColors, findRawColors } from '../../tools/lint/colors.ts';

const ROOT = path.resolve(import.meta.dirname, '../..');

describe('raw colours', () => {
  it('are refused as values, functions and palette classes', () => {
    const source = [
      "const shade = '#1a2b3c';",
      '<path fill="#fff" />',
      '.x { color: #abcdef80; background: oklch(0.5 0.1 200); }',
      '<div class="bg-red-500 text-white bg-[#000]"></div>',
      "const veil = 'rgba(0, 0, 0, 0.4)';",
    ].join('\n');
    expect(findRawColors('src/a.svelte', source)).toHaveLength(8);
  });

  it('are refused among the other values of a declaration, once each', () => {
    const source = [
      '.x { border: 1px solid #ccc; box-shadow: 0 0 0 1px #000; }',
      '<div style="outline: 2px dashed #ff000080"></div>',
      '.y { background: linear-gradient(90deg, #fff, #000); color: #123456; }',
    ].join('\n');
    expect(findRawColors('src/a.svelte', source).map((one) => one.split('`')[1])).toEqual([
      '#ccc',
      '#000',
      '#ff000080',
      '#123456',
      '#fff',
      '#000',
    ]);
  });

  it('are not private fields, card numbers or tokens', () => {
    const source = [
      'readonly #dead: () => void;',
      'this.#dead();',
      '// `#12` finds the card numbered 12',
      '<div class="bg-primary text-muted-foreground border-destructive/40"></div>',
      '{#each list as item (item.id)}',
      'readonly #add = 1; static #facade: Map<string, string>;',
      'const label = { title: this.#bead, of: other.#cab };',
      '// written in a comment: the cards #123 and #456',
      ' * as: #12 is the card in a comment that goes on',
    ].join('\n');
    expect(findRawColors('src/a.svelte', source)).toEqual([]);
  });

  it('live only in the homes the gate names, each with its reason', () => {
    for (const home of COLOR_HOMES) expect(home.reason).not.toBe('');
    expect(findRawColors('src/styles/app.css', ':root { --x: oklch(1 0 0); }')).toEqual([]);
    expect(checkColors(ROOT).problems).toEqual([]);
  });
});
