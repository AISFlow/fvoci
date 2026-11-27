/**
 * The licence of the HWP editor, rhwp-studio, for the notices: the build has
 * its source from a package of this repository (vendor/rhwp-studio), which
 * the bundler's own report of packages does not list, so it is listed here
 * when a module of it is in the bundle. rhwp's licence (MIT) is the file the
 * import brought with the source (`upstream/LICENSE`).
 *
 * What the editor depends on from npm (`@noble/hashes`) and the engine it
 * drives (`@rhwp/core`, rhwp-wasm.ts) are listed as every package is.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { studioDirectory } from '../build/rhwp-studio.ts';

import type { PackageLicense } from './packages.ts';

/** How a module of the editor's source is told, by its address: where it is, or where it is linked from. */
const SOURCE = /(?:vendor|@fvoci)\/rhwp-studio\/upstream\//;

export function rhwpStudioLicenses(bundledModules: readonly string[]): PackageLicense[] {
  const bundled = bundledModules.some((id) => SOURCE.test(id.split(path.sep).join('/')));
  if (!bundled) return [];
  const directory = studioDirectory();
  const manifest = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8')) as {
    version: string;
    license: string;
    rhwp: { repository: string; commit: string };
  };
  return [
    {
      name: `rhwp-studio (${manifest.rhwp.repository}, commit ${manifest.rhwp.commit})`,
      version: manifest.version,
      identifier: manifest.license,
      text: readFileSync(path.join(directory, 'upstream/LICENSE'), 'utf8').trim(),
    },
  ];
}
