/**
 * Prints the sizes of the text files of a build and of their compressed
 * forms, as a Markdown table.
 *
 *   node tools/build/sizes.ts [folder]
 *
 * The folder is `dist/static` unless one is named. Files without a compressed
 * form (the fonts) are left out. Run `pnpm run build` first.
 */
import path from 'node:path';

import { type FileSizes, outputSizes } from './output.ts';

const folder = path.resolve(process.argv[2] ?? path.join(import.meta.dirname, '../../dist/static'));
const names = ['gzip', 'br', 'zstd'] as const;
const sizes = (await outputSizes(folder)).filter(
  ({ compressed }) => Object.keys(compressed).length > 0,
);

const number = (value: number | undefined): string =>
  value === undefined ? '' : value.toLocaleString('en-US');
const line = (cells: string[]): string => `| ${cells.join(' | ')} |`;

console.log(line(['file', 'bytes', ...names]));
console.log(line(['---', ...['bytes', ...names].map(() => '---:')]));
for (const { file, bytes, compressed } of sizes) {
  // The hash in a name changes with every change of the content.
  const name = file.replace(/-[0-9a-f]{16}(?=\.)/, '-*');
  console.log(line([`\`${name}\``, number(bytes), ...names.map((n) => number(compressed[n]))]));
}
const total = (pick: (entry: FileSizes) => number | undefined): string =>
  number(sizes.reduce((sum, entry) => sum + (pick(entry) ?? 0), 0));
console.log(
  line([
    'total',
    total((entry) => entry.bytes),
    ...names.map((n) => total((entry) => entry.compressed[n])),
  ]),
);
