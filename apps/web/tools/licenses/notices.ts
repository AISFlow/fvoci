/**
 * Third-party notices of a build.
 *
 * What is listed is what is in the output, not what is installed:
 *
 * - packages whose modules Vite bundled (its own licence report),
 * - packages whose style sheets are imported from src/styles,
 * - packages whose tools wrote source files into this project ("vendored"),
 * - packages whose data a tool of this project turned into source files, when
 *   those files are in the bundle ("derived": the emoji lists, from CLDR),
 * - the data files of pdf.js the file viewer bundles, under their own
 *   licences (pdfjs-data.ts), and packages inside another package's
 *   prebuilt bundle (prebuilt.ts),
 * - the HWP renderer, whose code runs in a worker built apart, and the Rust
 *   crates built into its WebAssembly (rhwp-wasm.ts),
 * - the fonts the font tool produced,
 * - the fonts of the formula library, when that library is in the bundle
 *   (katex-fonts.ts: its package names only the licence of its code).
 *
 * Every package has to carry a licence from the list in the repository's
 * `deny.toml`; there are no exceptions for anything that ships. The fonts are
 * under the SIL Open Font License, which that list does not have because no
 * Rust crate needs it; they are accepted by name here. The HWP renderer's
 * WebAssembly holds work under a few licences more, each accepted for it alone
 * and named with the reason (rhwp-wasm.ts, rhwp-notices.json). Nothing else is.
 */
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';

import type { Plugin } from 'vite';

import exceptions from './exceptions.json' with { type: 'json' };
import { katexFontLicense } from './katex-fonts.ts';
import { withPackageNotices } from './package-notices.ts';
import { readPackageLicense, statedLicense, type PackageLicense } from './packages.ts';
import { bundledWorkerPackages } from './worker-packages.ts';
import { pdfjsDataLicenses } from './pdfjs-data.ts';
import { prebuiltLicenses } from './prebuilt.ts';
import { rhwpStudioLicenses } from './rhwp-studio.ts';
import { rhwpWasmLicenses } from './rhwp-wasm.ts';
import { isAllowed, readAllowedLicenses } from './policy.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');
const LICENSE_REPORT = '.vite/license.json';
export const NOTICES_FILE = 'static/THIRD_PARTY_NOTICES.txt';

/** The only licence accepted for font files. */
const FONT_LICENSE = 'OFL-1.1';

export interface FontLicense {
  family: string;
  version: string;
  identifier: string;
  text: string;
}

/** Packages named by a bare `@import` in a style sheet of the app. */
export function styleSheetPackages(css: string): string[] {
  const names = new Set<string>();
  for (const match of css.matchAll(/@import\s+['"]([^'"./][^'"]*)['"]/g)) {
    const specifier = match[1] ?? '';
    const parts = specifier.split('/');
    names.add(specifier.startsWith('@') ? parts.slice(0, 2).join('/') : (parts[0] ?? ''));
  }
  return [...names].sort();
}

function section(title: string, identifier: string, text: string): string {
  const rule = '-'.repeat(72);
  const body = text === '' ? `(the package ships no licence file; licence: ${identifier})` : text;
  return `${rule}\n${title}\nLicense: ${identifier}\n${rule}\n\n${body}\n`;
}

export function renderNotices(packages: PackageLicense[], fonts: FontLicense[]): string {
  const parts = [
    'fvoci web app: third-party notices',
    '',
    'The web app contains code and fonts from the projects below.',
    '',
  ];
  const sorted = [...packages].sort((a, b) => a.name.localeCompare(b.name));
  for (const item of sorted) {
    parts.push(section(`${item.name} ${item.version}`, item.identifier, item.text));
  }
  for (const font of [...fonts].sort((a, b) => a.family.localeCompare(b.family))) {
    parts.push(section(`${font.family} ${font.version} (font)`, font.identifier, font.text));
  }
  return parts.join('\n');
}

/**
 * Checks the licences of everything in the bundle and writes the notices file.
 * Needs `build.license.fileName` to be `.vite/license.json`.
 */
export function thirdPartyNotices(): Plugin {
  return {
    name: 'fvoci:third-party-notices',
    apply: 'build',
    generateBundle: {
      order: 'post',
      handler(_options, bundle) {
        const report = bundle[LICENSE_REPORT];
        if (report?.type !== 'asset') {
          this.error(`${LICENSE_REPORT} is missing: set build.license.fileName to it`);
        }
        // A package whose manifest names no licence takes what its licence
        // file states, if it is listed under `stated` in exceptions.json;
        // otherwise it has none and is refused below.
        const bundled = (JSON.parse(String(report.source)) as PackageLicense[]).map((item) => {
          if (item.identifier) return item;
          const text = item.text || readPackageLicense(item.name).text;
          return { ...item, text, identifier: statedLicense(item.name, text) ?? '' };
        });
        // The raw report is an input of this step, not part of the product.
        Reflect.deleteProperty(bundle, LICENSE_REPORT);

        const byName = new Map(bundled.map((item) => [item.name, item]));
        const css = readFileSync(path.join(WEB_ROOT, 'src/styles/app.css'), 'utf8');
        const extra = [...styleSheetPackages(css), ...exceptions.vendored.map((item) => item.name)];
        for (const name of extra) {
          if (!byName.has(name)) byName.set(name, readPackageLicense(name));
        }
        // Data that became source files: listed when a module made from it was bundled.
        const bundledModules = Object.values(bundle).flatMap((item) =>
          item.type === 'chunk' ? item.moduleIds : [],
        );
        for (const { name, modules } of exceptions.derived) {
          const used = bundledModules.some((id) => id.split(path.sep).join('/').includes(modules));
          if (used && !byName.has(name)) byName.set(name, readPackageLicense(name));
        }
        // The file viewer's pdf.js data: other people's files, under their own licences.
        for (const item of pdfjsDataLicenses(bundledModules)) byName.set(item.name, item);
        // The HWP renderer: its worker is a build of its own, and its WebAssembly holds crates.
        for (const item of rhwpWasmLicenses(bundledModules)) byName.set(item.name, item);
        // The HWP editor: built from a package of this repository, which the report does not list.
        for (const item of rhwpStudioLicenses(bundledModules)) byName.set(item.name, item);
        // Packages in the app's workers, which reach the bundle as text (worker-packages.ts).
        for (const item of bundledWorkerPackages()) {
          if (!byName.has(item.name)) byName.set(item.name, item);
        }
        // Packages inside another package's prebuilt bundle (prebuilt.ts).
        for (const item of prebuiltLicenses([...byName.keys()])) {
          if (!byName.has(item.name)) byName.set(item.name, item);
        }

        const allowed = new Set(readAllowedLicenses());
        const refused = [...byName.values()].filter(
          (item) => !isAllowed(item.identifier, new Set([...allowed, ...(item.accepted ?? [])])),
        );
        if (refused.length > 0) {
          this.error(
            `packages in the bundle with a licence that deny.toml does not allow (a package whose package.json names none: read its licence file and list it under "stated" in tools/licenses/exceptions.json):\n  ${refused
              .map((item) => `${item.name} ${item.version}: ${item.identifier || '(none)'}`)
              .join('\n  ')}`,
          );
        }

        const fontsFile = path.join(WEB_ROOT, 'src/styles/fonts/licenses.json');
        const fonts = existsSync(fontsFile)
          ? (JSON.parse(readFileSync(fontsFile, 'utf8')) as FontLicense[])
          : [];
        // The formula library brings fonts of its own, under a licence its package does not name.
        if (byName.has('katex')) fonts.push(katexFontLicense());
        const refusedFonts = fonts.filter((font) => font.identifier !== FONT_LICENSE);
        if (refusedFonts.length > 0) {
          this.error(
            `fonts with a licence other than ${FONT_LICENSE}: ${refusedFonts.map((font) => font.family).join(', ')}`,
          );
        }

        this.emitFile({
          type: 'asset',
          fileName: NOTICES_FILE,
          source: renderNotices([...byName.values()].map(withPackageNotices), fonts),
        });
        this.info(`notices: ${byName.size} packages, ${fonts.length} fonts`);
      },
    },
  };
}
