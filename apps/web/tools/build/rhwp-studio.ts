/**
 * The editor of HWP documents, rhwp-studio, as the build takes it: the
 * package `@fvoci/rhwp-studio` of this repository (vendor/rhwp-studio), which
 * holds the editor's source as brought from rhwp's repository at one version.
 *
 * The editor and the engine it drives, `@rhwp/core` from npm, are one program
 * split over two packages, and move only together: the editor calls the
 * engine by names of its own version. `studioVersion` fails, and with it the
 * build and the tests, when the engine installed, the engine the editor's
 * package names and the editor itself are not all of one version.
 */
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import { packageDirectory } from '../licenses/packages.ts';

interface Manifest {
  version: string;
  dependencies?: Record<string, string>;
  rhwp?: { tag?: string; commit?: string; patched?: string[] };
}

const manifestOf = (directory: string): Manifest =>
  JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8')) as Manifest;

/** The folder of the editor's package. */
export const studioDirectory = (): string => packageDirectory('@fvoci/rhwp-studio');

/** The versions that must be one, by who says each. */
export function rhwpVersions(): Record<string, string> {
  const studio = manifestOf(studioDirectory());
  const web = manifestOf(path.resolve(import.meta.dirname, '../..'));
  return {
    'the editor (vendor/rhwp-studio/package.json: version)': studio.version,
    "the editor's tag (rhwp.tag)": (studio.rhwp?.tag ?? '').replace(/^v/, ''),
    "the engine the editor's package names (@rhwp/core)": studio.dependencies?.['@rhwp/core'] ?? '',
    'the engine the web app names (apps/web/package.json: @rhwp/core)':
      web.dependencies?.['@rhwp/core'] ?? '',
    'the engine installed (@rhwp/core)': manifestOf(packageDirectory('@rhwp/core')).version,
  };
}

/** Throws unless every one of `versions` is the same exact version. */
export function agreed(versions: Record<string, string>): string {
  const distinct = new Set(Object.values(versions));
  const [version] = distinct;
  if (distinct.size !== 1 || !version || !/^\d+\.\d+\.\d+$/.test(version)) {
    const lines = Object.entries(versions).map(([who, said]) => `  ${who}: ${said || '(none)'}`);
    throw new Error(
      `rhwp's engine and editor are not of one version; they move only together (vendor/rhwp-studio/import.mjs):\n${lines.join('\n')}`,
    );
  }
  return version;
}

/** The one version of rhwp's engine and editor. */
export const studioVersion = (): string => agreed(rhwpVersions());

/**
 * The files of the editor's source that are not as rhwp has them at the tag
 * (`upstream.json`, their SHA-256 as imported): our changes, which the
 * package names (`rhwp.patched`), together with any file added or gone.
 */
export function changedFromUpstream(directory = studioDirectory()): string[] {
  const hashes = JSON.parse(readFileSync(path.join(directory, 'upstream.json'), 'utf8')) as Record<
    string,
    string
  >;
  const under = (folder: string): string[] =>
    readdirSync(path.join(directory, 'upstream', folder), { withFileTypes: true }).flatMap(
      (entry) => {
        const at = path.posix.join(folder, entry.name);
        return entry.isDirectory() ? under(at) : [at];
      },
    );
  const present = new Set(under(''));
  const changed = Object.entries(hashes).filter(([file, hash]) => {
    if (!present.has(file)) return true;
    const bytes = readFileSync(path.join(directory, 'upstream', file));
    return createHash('sha256').update(bytes).digest('hex') !== hash;
  });
  const added = [...present].filter((file) => !(file in hashes));
  return [...changed.map(([file]) => file), ...added].sort();
}

/** The files the package says are changed (`rhwp.patched`). */
export const namedAsPatched = (directory = studioDirectory()): string[] =>
  [...(manifestOf(directory).rhwp?.patched ?? [])].sort();
