/**
 * The notices of the file viewer's HWP renderer, `@rhwp/core`.
 *
 * Its package names one licence, rhwp's own (MIT), and holds its licence
 * file. Its WebAssembly is rhwp built together with other people's work:
 *
 * - the Rust crates it depends on, each under its own licence. Their notices
 *   are in `rhwp-wasm-crates.txt`, generated from rhwp's source at the
 *   installed version with `rhwp-about.toml` (the licences of deny.toml and
 *   IJG: the generation fails on any other);
 * - data built into it (outlines and widths taken from fonts, tables of
 *   Unicode, code ported from other projects), whose notices the package
 *   carries in its `notices` folder, and Adobe's note on its AFM files, which
 *   the package leaves out (`rhwp-adobe-afm.txt`, from rhwp's source).
 *
 * `rhwp-notices.json` names every file of that folder, as listed in the app's
 * notices or left out with the reason, and each licence accepted for this
 * WebAssembly that deny.toml does not have, with the reason. A file the list
 * does not name, or one it names that is gone, fails the build: the notices
 * of a new version are read before they are shipped.
 *
 * All are listed when the module made from the WebAssembly is in the bundle
 * (tools/build/rhwp-wasm.ts).
 *
 * After updating `@rhwp/core`, generate the crates' file again from the new
 * version (the build fails until it names that version), from the repository
 * root, in a folder of rhwp's source alone, in which nothing is run:
 *
 *   git clone --depth 1 --branch v<version> https://github.com/edwardkim/rhwp.git target/rhwp-whole
 *   RUSTUP_TOOLCHAIN=stable cargo about generate -m target/rhwp-whole/Cargo.toml \
 *     -c apps/web/tools/licenses/rhwp-about.toml \
 *     --target wasm32-unknown-unknown --locked --fail \
 *     apps/web/tools/licenses/rhwp-wasm.hbs -o apps/web/tools/licenses/rhwp-wasm-crates.txt
 *
 * (`RUSTUP_TOOLCHAIN` keeps rustup from installing the toolchain rhwp pins.)
 */
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import known from './rhwp-notices.json' with { type: 'json' };
import { packageDirectory, readPackageLicense, type PackageLicense } from './packages.ts';

const WASM_MODULE = 'virtual:rhwp-wasm';
const CRATES_FILE = path.join(import.meta.dirname, 'rhwp-wasm-crates.txt');
const ADOBE_AFM_FILE = path.join(import.meta.dirname, 'rhwp-adobe-afm.txt');
const KNOWN_FILE = 'tools/licenses/rhwp-notices.json';

/** The licences accepted for this WebAssembly that deny.toml does not have. */
export const RHWP_ACCEPTED: readonly string[] = known.accepted.map((item) => item.license);

/** The one of them a crate comes under: what `rhwp-about.toml` accepts over the repository's list. */
export const RHWP_CRATE_ACCEPTED: readonly string[] = ['IJG'];

/** What the notices of the data name, besides the licences of deny.toml they come under. */
const DATA_LICENSES = [
  'MIT',
  'Apache-2.0',
  'BSD-3-Clause',
  'Zlib',
  'Unicode-3.0',
  ...RHWP_ACCEPTED,
];

/** Every file under `directory`, by its address from it with "/" between folders. */
function filesUnder(directory: string, prefix = ''): string[] {
  return readdirSync(directory, { withFileTypes: true })
    .flatMap((entry) =>
      entry.isDirectory()
        ? filesUnder(path.join(directory, entry.name), `${prefix}${entry.name}/`)
        : [`${prefix}${entry.name}`],
    )
    .sort();
}

/**
 * The text of the notices the package carries, each file under its address;
 * throws when the folder holds a file the list does not name, or lacks one it
 * names.
 */
export function rhwpDataNotices(
  folder: string,
  listed: readonly string[] = known.listed,
  leftOut: readonly string[] = known.leftOut.map((item) => item.file),
): string {
  const found = filesUnder(folder);
  const named = new Set([...listed, ...leftOut]);
  const unknown = found.filter((file) => !named.has(file));
  const gone = [...named].filter((file) => !found.includes(file));
  if (unknown.length > 0 || gone.length > 0) {
    throw new Error(
      `the notices of @rhwp/core are not the ones ${KNOWN_FILE} names: read them, and list each or leave it out with the reason\n  new: ${unknown.join(', ') || '(none)'}\n  gone: ${gone.join(', ') || '(none)'}`,
    );
  }
  const rule = '='.repeat(72);
  return listed
    .map(
      (file) =>
        `${rule}\n${file}\n${rule}\n\n${readFileSync(path.join(folder, file), 'utf8').trim()}`,
    )
    .join('\n\n');
}

/** The licences of rhwp and of what is built into its WebAssembly, when that is in the bundle. */
export function rhwpWasmLicenses(bundledModules: readonly string[]): PackageLicense[] {
  if (!bundledModules.some((id) => id.includes(WASM_MODULE))) return [];
  const own = readPackageLicense('@rhwp/core');
  const directory = packageDirectory('@rhwp/core');
  const { version } = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8')) as {
    version: string;
  };
  const text = readFileSync(CRATES_FILE, 'utf8').trim();
  if (!text.includes(`\n  rhwp ${version} `)) {
    throw new Error(
      `${CRATES_FILE} is not for @rhwp/core ${version}: generate it again (see rhwp-wasm.ts)`,
    );
  }
  const listed = /^Licenses: (.+)$/m.exec(text)?.[1]?.split(' ') ?? [];
  if (listed.length === 0) throw new Error(`${CRATES_FILE}: no "Licenses:" line`);
  const data = rhwpDataNotices(path.join(directory, 'notices'));
  return [
    own,
    {
      name: '@rhwp/core WebAssembly (Rust crates)',
      version,
      identifier: listed.join(' AND '),
      accepted: RHWP_CRATE_ACCEPTED,
      text,
    },
    {
      name: '@rhwp/core WebAssembly (data and ported code)',
      version,
      identifier: DATA_LICENSES.join(' AND '),
      accepted: RHWP_ACCEPTED,
      text: `${data}\n\n${readFileSync(ADOBE_AFM_FILE, 'utf8').trim()}`,
    },
  ];
}
