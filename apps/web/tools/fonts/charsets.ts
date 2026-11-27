/**
 * The characters that go into the "common" file of each font family.
 *
 * A family is cut into files: one with the characters almost every page in
 * that language uses, and one with everything else the font has, which is
 * only downloaded when a page shows a character from it. A family can name
 * more files in between (see families.ts): Korean keeps the syllables beyond
 * the everyday ones apart from the rest.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { expand, type Range } from './unicode-range.ts';

/** Unicode blocks, or the used parts of them. */
const BLOCKS = {
  basicLatin: [[0x20, 0x7e]],
  latin1: [[0xa0, 0xff]],
  latinExtendedA: [[0x100, 0x17f]],
  spacingModifiers: [[0x2b0, 0x2ff]],
  combiningDiacritics: [[0x300, 0x36f]],
  generalPunctuation: [[0x2000, 0x206f]],
  superscripts: [[0x2070, 0x209f]],
  currency: [[0x20a0, 0x20cf]],
  letterlike: [[0x2100, 0x214f]],
  arrows: [[0x2190, 0x21ff]],
  mathOperators: [[0x2200, 0x22ff]],
  boxDrawing: [[0x2500, 0x259f]],
  geometricShapes: [[0x25a0, 0x25ff]],
  cjkPunctuation: [[0x3000, 0x303f]],
  hiragana: [[0x3040, 0x309f]],
  katakana: [
    [0x30a0, 0x30ff],
    [0x31f0, 0x31ff],
  ],
  bopomofo: [
    [0x3100, 0x312f],
    [0x31a0, 0x31bf],
  ],
  hangulCompatibilityJamo: [[0x3130, 0x318f]],
  enclosedCjk: [[0x3200, 0x32ff]],
  hangulSyllables: [[0xac00, 0xd7a3]],
  fullwidthForms: [[0xff00, 0xffef]],
} as const satisfies Record<string, readonly Range[]>;

export type BlockName = keyof typeof BLOCKS;
export type ListName =
  | 'joyo'
  | 'tgh-level1'
  | 'big5-level1'
  | 'ks-x-1001-hangul'
  | 'ks-x-1001-symbols'
  | 'jis-x-0208-symbols'
  | 'gb-2312-symbols'
  | 'big5-symbols';

/** A list under charsets/: every character on a line that is not a comment. */
export function readList(name: ListName): number[] {
  const text = readFileSync(path.resolve(import.meta.dirname, 'charsets', `${name}.txt`), 'utf8');
  const codePoints: number[] = [];
  for (const line of text.split('\n')) {
    if (line.startsWith('#')) continue;
    for (const character of line) {
      const codePoint = character.codePointAt(0);
      if (codePoint !== undefined && codePoint > 0x20 && codePoint !== 0xa0) {
        codePoints.push(codePoint);
      }
    }
  }
  return codePoints;
}

export interface CharsetSpec {
  blocks: readonly BlockName[];
  lists?: readonly ListName[];
}

/** The code points a specification names, whether or not a font has them. */
export function resolveCharset(spec: CharsetSpec): Set<number> {
  const codePoints = new Set<number>();
  for (const block of spec.blocks) for (const code of expand(BLOCKS[block])) codePoints.add(code);
  for (const list of spec.lists ?? []) for (const code of readList(list)) codePoints.add(code);
  return codePoints;
}
