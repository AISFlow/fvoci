/**
 * Packages that ship a prebuilt bundle of their own with other packages
 * inside it. Vite's report names only the outer package, so the ones inside
 * are listed here and added to the notices whenever the outer one is bundled.
 */
import { packageDirectory, readPackageLicense, type PackageLicense } from './packages.ts';

/** Each package inside, with the chain of packages that depends on it, from the outer one. */
const PREBUILT: Record<string, string[][]> = {
  // `browser` maps jszip to dist/jszip.min.js, which has pako, lie (with
  // immediate) and setimmediate in it (the Word renderer of the file viewer).
  jszip: [['pako'], ['lie'], ['lie', 'immediate'], ['setimmediate']],
};

/** The licences of the packages inside the prebuilt bundles of `bundled`. */
export function prebuiltLicenses(bundled: Iterable<string>): PackageLicense[] {
  const found: PackageLicense[] = [];
  for (const outer of bundled) {
    for (const chain of PREBUILT[outer] ?? []) {
      let directory = packageDirectory(outer);
      for (const name of chain.slice(0, -1)) directory = packageDirectory(name, directory);
      found.push(readPackageLicense(chain.at(-1) ?? '', directory));
    }
  }
  return found;
}
