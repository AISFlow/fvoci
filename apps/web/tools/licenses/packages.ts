/** Reading the licence of an installed package from `node_modules`. */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

import exceptions from './exceptions.json' with { type: 'json' };

export interface PackageLicense {
  name: string;
  version: string;
  /** SPDX expression from `package.json`. */
  identifier: string;
  /** Text of the licence file shipped in the package, or empty. */
  text: string;
  /**
   * Licences accepted for this item alone that `deny.toml` does not have, each
   * by a decision written where the item is made (tools/licenses/rhwp-wasm.ts).
   */
  accepted?: readonly string[];
}

const require = createRequire(path.resolve(import.meta.dirname, '../../package.json'));

/**
 * Directory of a package that the web app depends on directly, or, with
 * `from`, that the package in that directory depends on.
 */
export function packageDirectory(name: string, from?: string): string {
  const resolver = from ? createRequire(path.join(from, 'package.json')) : require;
  // `exports` of some packages hide package.json, so walk up from any entry.
  const searchPaths = resolver.resolve.paths(name) ?? [];
  for (const base of searchPaths) {
    const candidate = path.join(base, name);
    if (existsSync(path.join(candidate, 'package.json'))) return candidate;
  }
  throw new Error(`package ${name} is not installed`);
}

/** The text of the licence file a package ships, or empty. */
export function licenseFileText(directory: string): string {
  const licenseFile = readdirSync(directory).find((file) =>
    /^(licen[sc]e|copying)(\.|$)/i.test(file),
  );
  return licenseFile ? readFileSync(path.join(directory, licenseFile), 'utf8').trim() : '';
}

/**
 * The licence of a package whose manifest names none but whose licence file
 * does. Such a package is listed under `stated` in exceptions.json with the
 * licence and the sentence its file has to contain; the name alone is not
 * taken on trust, the text is looked at.
 */
export function statedLicense(name: string, licenseText: string): string | undefined {
  const entry = exceptions.stated.find((candidate) => candidate.name === name);
  return entry && licenseText.includes(entry.says) ? entry.license : undefined;
}

export function readPackageLicense(name: string, from?: string): PackageLicense {
  return readPackageLicenseAt(packageDirectory(name, from));
}

/** The licence of the package installed in `directory`. */
export function readPackageLicenseAt(directory: string): PackageLicense {
  const manifest = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8')) as {
    name: string;
    version: string;
    license?: string;
  };
  const text = licenseFileText(directory);
  return {
    name: manifest.name,
    version: manifest.version,
    identifier: manifest.license ?? statedLicense(manifest.name, text) ?? '',
    text,
  };
}
