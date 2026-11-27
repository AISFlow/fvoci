/**
 * Writes the lists of everyday Han characters under charsets/.
 *
 *   node tools/fonts/derive-charsets.ts
 *
 * The lists are committed; this script is how they were made and how to make
 * them again when the Unicode data moves on. Each list is a national standard
 * for "the characters in everyday use", read from the Unihan database:
 *
 * - joyo: the Joyo kanji of Japan (2,136), property kJoyoKanji
 * - tgh-level1: level 1 of the Table of General Standard Chinese Characters
 *   of mainland China (3,500), property kTGH with an index up to 3500
 * - big5-level1: level 1 of Big5, the frequently used characters of Taiwan
 *   (5,401), the byte pairs A440 to C67E
 *
 * and the symbols each of the four national character sets has next to its
 * letters and Han characters (punctuation, units, arrows, box drawing, ...),
 * read from the byte ranges of the set through the platform's decoder:
 *
 * - ks-x-1001-symbols: rows 1 to 12 of KS X 1001 (Korea)
 * - ks-x-1001-hangul: rows 16 to 40 of KS X 1001, its 2,350 Hangul syllables:
 *   the ones everyday Korean is written with
 * - jis-x-0208-symbols: rows 1 to 8 of JIS X 0208 (Japan)
 * - gb-2312-symbols: rows 1 to 9 of GB 2312 (mainland China)
 * - big5-symbols: the Big5 byte pairs A140 to A3FE (Taiwan)
 */
import { writeFileSync } from 'node:fs';
import path from 'node:path';

import { fetchPinned } from './fetch.ts';
import sources from './sources.json' with { type: 'json' };
import { readZipMember } from '../zip.ts';

const OUTPUT = path.resolve(import.meta.dirname, 'charsets');

function writeList(name: string, description: string, codePoints: number[]): void {
  const sorted = [...new Set(codePoints)].sort((a, b) => a - b);
  const lines = [`# ${description}`, `# ${sorted.length} characters. Made by derive-charsets.ts.`];
  for (let index = 0; index < sorted.length; index += 40) {
    lines.push(String.fromCodePoint(...sorted.slice(index, index + 40)));
  }
  writeFileSync(path.join(OUTPUT, `${name}.txt`), `${lines.join('\n')}\n`);
  console.log(`${name}: ${sorted.length}`);
}

const archive = await fetchPinned('Unihan.zip', sources.unihan);
const mappings = readZipMember(archive, 'Unihan_OtherMappings.txt').toString('utf8');

const joyo: number[] = [];
const tghLevel1: number[] = [];
for (const line of mappings.split('\n')) {
  const [code, property, value] = line.split('\t');
  if (!code?.startsWith('U+') || value === undefined) continue;
  const codePoint = Number.parseInt(code.slice(2), 16);
  // A value that is itself a code point marks a variant form, not a member.
  if (property === 'kJoyoKanji' && !value.startsWith('U+')) joyo.push(codePoint);
  if (property === 'kTGH') {
    const index = Number(value.split(':')[1]);
    if (index >= 1 && index <= 3500) tghLevel1.push(codePoint);
  }
}

function range(first: number, last: number): number[] {
  return Array.from({ length: last - first + 1 }, (_, index) => first + index);
}

/** The characters a double-byte encoding assigns to the given lead and trail bytes. */
function decodePairs(encoding: string, leads: number[], trails: number[]): number[] {
  const decoder = new TextDecoder(encoding, { fatal: true });
  const codePoints: number[] = [];
  for (const lead of leads) {
    for (const trail of trails) {
      let text: string;
      try {
        text = decoder.decode(new Uint8Array([lead, trail]));
      } catch {
        continue; // not assigned
      }
      const codePoint = text.codePointAt(0);
      const privateUse = codePoint !== undefined && codePoint >= 0xe000 && codePoint <= 0xf8ff;
      if (codePoint !== undefined && text.length === 1 && !privateUse) codePoints.push(codePoint);
    }
  }
  return codePoints;
}

const HIGH = range(0xa1, 0xfe);
const BIG5_TRAILS = [...range(0x40, 0x7e), ...HIGH];
// Level 1 ends at C67E, in the middle of the last lead byte.
const big5Level1 = [
  ...decodePairs('big5', range(0xa4, 0xc5), BIG5_TRAILS),
  ...decodePairs('big5', [0xc6], range(0x40, 0x7e)),
];
const big5Symbols = decodePairs('big5', range(0xa1, 0xa3), BIG5_TRAILS);
const ksSymbols = decodePairs('euc-kr', range(0xa1, 0xac), HIGH);
const ksHangul = decodePairs('euc-kr', range(0xb0, 0xc8), HIGH);
const jisSymbols = decodePairs('euc-jp', range(0xa1, 0xa8), HIGH);
const gbSymbols = decodePairs('gbk', range(0xa1, 0xa9), HIGH);

const version = `Unihan ${sources.unihan.version}`;
writeList('joyo', `Joyo kanji (Japan, 2010). Source: ${version}, kJoyoKanji.`, joyo);
writeList(
  'tgh-level1',
  `Table of General Standard Chinese Characters (2013), level 1. Source: ${version}, kTGH 1-3500.`,
  tghLevel1,
);
writeList(
  'ks-x-1001-symbols',
  'Symbols of KS X 1001, rows 1-12 (Korea). Source: EUC-KR A1A1-ACFE.',
  ksSymbols,
);
writeList(
  'ks-x-1001-hangul',
  'Hangul syllables of KS X 1001, rows 16-40 (Korea). Source: EUC-KR B0A1-C8FE.',
  ksHangul,
);
writeList(
  'jis-x-0208-symbols',
  'Symbols of JIS X 0208, rows 1-8 (Japan). Source: EUC-JP A1A1-A8FE.',
  jisSymbols,
);
writeList(
  'gb-2312-symbols',
  'Symbols of GB 2312, rows 1-9 (mainland China). Source: GBK A1A1-A9FE.',
  gbSymbols,
);
writeList(
  'big5-symbols',
  'Symbols of Big5 (Taiwan). Source: the Big5 byte pairs A140-A3FE.',
  big5Symbols,
);
writeList(
  'big5-level1',
  'Big5 level 1, the frequently used characters (Taiwan). Source: the Big5 byte pairs A440-C67E.',
  big5Level1,
);
