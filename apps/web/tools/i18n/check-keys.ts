/**
 * Compares the keys of every locale's messages with those of the base locale.
 *
 *   node tools/i18n/check-keys.ts [--verbose]
 *
 * Prints the number of missing and of extra keys per locale and names them;
 * more than a few missing keys are named only with `--verbose`. Fails when a
 * locale has a key the base locale lacks, or when a locale that has to be
 * complete (Korean) lacks one. The locales and the files are those of
 * `project.inlang/settings.json`.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import {
  COMPLETE_LOCALES,
  compareKeys,
  type KeyDifference,
  keyErrors,
  messageKeys,
} from './keys.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

interface Settings {
  baseLocale: string;
  locales: string[];
  'plugin.inlang.messageFormat': { pathPattern: string | string[] };
}

const NAMED_WITHOUT_VERBOSE = 10;
const verbose = process.argv.includes('--verbose');

const settings = JSON.parse(
  readFileSync(path.join(WEB_ROOT, 'project.inlang/settings.json'), 'utf8'),
) as Settings;
const patterns = [settings['plugin.inlang.messageFormat'].pathPattern].flat();

function keysOf(locale: string): string[] {
  const keys = patterns.flatMap((pattern) => {
    const file = path.join(WEB_ROOT, pattern.replace('{locale}', locale));
    return messageKeys(JSON.parse(readFileSync(file, 'utf8')));
  });
  return [...new Set(keys)];
}

const base = keysOf(settings.baseLocale);
const differences = new Map<string, KeyDifference>();
for (const locale of settings.locales) {
  if (locale !== settings.baseLocale) differences.set(locale, compareKeys(base, keysOf(locale)));
}

console.log(`${settings.baseLocale.padEnd(8)} ${String(base.length).padStart(4)} keys`);
for (const [locale, { missing, extra }] of differences) {
  console.log(
    `${locale.padEnd(8)} ${String(missing.length).padStart(4)} missing ${String(extra.length).padStart(4)} extra`,
  );
  for (const key of extra) console.log(`           extra: ${key}`);
  if (verbose || missing.length <= NAMED_WITHOUT_VERBOSE) {
    for (const key of missing) console.log(`           missing: ${key}`);
  }
}
if (!verbose && [...differences.values()].some((d) => d.missing.length > NAMED_WITHOUT_VERBOSE)) {
  console.log('\n--verbose names every missing key');
}

const errors = keyErrors(differences, COMPLETE_LOCALES);
if (errors.length > 0) {
  console.error(`\nmessage keys:\n  ${errors.join('\n  ')}`);
  process.exit(1);
}
console.log('\nmessage keys: ok');
