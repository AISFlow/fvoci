/**
 * Builds the web fonts.
 *
 *   node tools/fonts/build.ts [--family <id>,<id>] [--force]
 *
 * For every family in families.ts the tool downloads the pinned source
 * (sources.json), cuts it into its files (a "common" one, a "rest", and for
 * some families more in between) in WOFF2, and
 * writes the `@font-face` rules that tell a browser which characters are in
 * which file. Nothing it writes is committed: the output directory
 * src/styles/fonts is ignored by git, and the build runs this tool first.
 *
 * Without `--family` the families the design tokens name are built. A run is
 * skipped when the inputs are the ones the existing output was made from.
 */
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import subsetFont from 'subset-font';

import { FAMILIES, partition, type Family } from './families.ts';
import { fetchPinned } from './fetch.ts';
import { readCodePoints, readName } from './sfnt.ts';
import sources from './sources.json' with { type: 'json' };
import { formatUnicodeRange, toText } from './unicode-range.ts';

const OUTPUT = path.resolve(import.meta.dirname, '../../src/styles/fonts');
const STAMP = path.join(OUTPUT, 'stamp.json');

type SourceId = keyof typeof sources.fonts;

interface FileReport {
  file: string;
  family: string;
  part: string;
  characters: number;
  bytes: number;
}

function argument(name: string): string | undefined {
  const index = process.argv.indexOf(name);
  return index === -1 ? undefined : process.argv[index + 1];
}

function selectedFamilies(): Family[] {
  const requested = argument('--family')?.split(',');
  if (!requested) return FAMILIES.filter((family) => family.use === 'stack');
  return requested.map((id) => {
    const family = FAMILIES.find((candidate) => candidate.id === id);
    if (!family) {
      throw new Error(`unknown family ${id}; known: ${FAMILIES.map((f) => f.id).join(', ')}`);
    }
    return family;
  });
}

/** A digest of everything the output depends on. */
function inputDigest(families: Family[]): string {
  const hash = createHash('sha256');
  const here = import.meta.dirname;
  for (const file of readdirSync(here, { recursive: true, withFileTypes: true })) {
    if (file.isFile()) hash.update(readFileSync(path.join(file.parentPath, file.name)));
  }
  hash.update(families.map((family) => family.id).join(','));
  const subsetter = path.resolve(here, '../../node_modules/subset-font/package.json');
  hash.update(readFileSync(subsetter));
  return hash.digest('hex');
}

function fontFace(
  family: Family,
  weight: string,
  local: readonly string[],
  file: string,
  unicodeRange: string,
): string {
  const src = [...local.map((name) => `local('${name}')`), `url('./${file}') format('woff2')`];
  return [
    '@font-face {',
    `  font-family: '${family.family}';`,
    '  font-style: normal;',
    `  font-weight: ${weight};`,
    '  font-display: swap;',
    `  src: ${src.join(', ')};`,
    `  unicode-range: ${unicodeRange};`,
    '}',
  ].join('\n');
}

async function buildFamily(family: Family): Promise<{ css: string; files: FileReport[] }> {
  const excluded = new Set<number>();
  if (family.excludeCoverageOf) {
    const other = sources.fonts[family.excludeCoverageOf as SourceId];
    const font = await fetchPinned(`${family.excludeCoverageOf}.ttf`, other.font);
    for (const code of readCodePoints(font)) excluded.add(code);
  }
  const rules: string[] = [];
  const files: FileReport[] = [];

  for (const { source, weights } of family.sources) {
    const pinned = sources.fonts[source as SourceId];
    const font = await fetchPinned(`${source}.ttf`, pinned.font);
    for (const { name: part, codePoints } of partition(family, readCodePoints(font), excluded)) {
      const file = `${source}-${part}.woff2`;
      const subset = await subsetFont(font, toText(codePoints), {
        targetFormat: 'woff2',
        ...(family.pinAxes ? { variationAxes: family.pinAxes } : {}),
      });
      writeFileSync(path.join(OUTPUT, file), subset);
      files.push({
        file,
        family: family.family,
        part,
        characters: codePoints.length,
        bytes: subset.length,
      });
      const unicodeRange = formatUnicodeRange(codePoints);
      for (const { weight, local } of weights) {
        rules.push(fontFace(family, weight, local, file, unicodeRange));
      }
    }
  }
  return { css: `${rules.join('\n\n')}\n`, files };
}

async function main(): Promise<void> {
  const families = selectedFamilies();
  const digest = inputDigest(families);
  if (!process.argv.includes('--force') && existsSync(STAMP)) {
    const stamp = JSON.parse(readFileSync(STAMP, 'utf8')) as { digest: string; files: string[] };
    if (
      stamp.digest === digest &&
      stamp.files.every((file) => existsSync(path.join(OUTPUT, file)))
    ) {
      console.log('fonts: up to date');
      return;
    }
  }

  rmSync(OUTPUT, { recursive: true, force: true });
  mkdirSync(OUTPUT, { recursive: true });

  const report: FileReport[] = [];
  const licenses = new Map<
    string,
    { family: string; version: string; identifier: string; text: string }
  >();
  const written: string[] = [];

  for (const family of families) {
    const started = Date.now();
    const { css, files } = await buildFamily(family);
    const cssFile = `faces-${family.id}.css`;
    writeFileSync(path.join(OUTPUT, cssFile), css);
    written.push(cssFile, ...files.map((file) => file.file));
    report.push(...files);

    const first = family.sources[0];
    if (!first) throw new Error(`${family.id} has no source`);
    const pinned = sources.fonts[first.source as SourceId];
    const font = await fetchPinned(`${first.source}.ttf`, pinned.font);
    const license = await fetchPinned(`${first.source}.OFL.txt`, pinned.license);
    licenses.set(family.family, {
      family: family.family,
      version: `${readName(font, 5) ?? 'unknown version'} (${pinned.origin})`,
      identifier: 'OFL-1.1',
      text: license.toString('utf8').trim(),
    });
    console.log(
      `${family.family}: ${files.length} files, CSS ${Buffer.byteLength(css)} bytes, ${Date.now() - started} ms`,
    );
  }

  writeFileSync(
    path.join(OUTPUT, 'licenses.json'),
    `${JSON.stringify([...licenses.values()], null, 2)}\n`,
  );
  writeFileSync(path.join(OUTPUT, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  written.push('licenses.json', 'report.json');
  writeFileSync(STAMP, `${JSON.stringify({ digest, files: written }, null, 2)}\n`);

  console.log('');
  for (const item of report) {
    console.log(
      `${item.file.padEnd(34)} ${String(item.characters).padStart(6)} characters ${String(item.bytes).padStart(9)} bytes`,
    );
  }
  const total = report.reduce((sum, item) => sum + item.bytes, 0);
  console.log(`${'total'.padEnd(34)} ${' '.repeat(17)} ${String(total).padStart(9)} bytes`);
}

await main();
