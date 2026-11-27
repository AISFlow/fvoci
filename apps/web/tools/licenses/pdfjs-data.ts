/**
 * The licences of the data files of pdf.js that the file viewer bundles.
 *
 * The package says "Apache-2.0", the licence of pdf.js itself. The character
 * maps (`cmaps/`, Adobe's) and the WebAssembly decoders (`wasm/`: PDFium's
 * JBIG2, OpenJPEG, qcms, each with pdf.js's code around it) are other
 * people's work under their own licences, whose texts the package carries
 * beside them. Each set is listed when a module made from one of its files
 * is in the bundle (tools/build/pdfjs-data.ts names them).
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { packageDirectory, type PackageLicense } from './packages.ts';

interface DataSet {
  /** The part of a bundled module's path that says it is one of this set's files. */
  files: string;
  name: string;
  /** The licence files of the set, under the package, and the licence each states. */
  licenses: { file: string; identifier: string }[];
}

const SETS: DataSet[] = [
  {
    files: 'virtual:pdfjs-data/cmaps/',
    name: 'pdfjs-dist CMaps (Adobe)',
    licenses: [{ file: 'cmaps/LICENSE', identifier: 'BSD-3-Clause' }],
  },
  {
    files: 'virtual:pdfjs-data/wasm/jbig2',
    name: 'pdfjs-dist JBIG2 decoder (PDFium)',
    licenses: [
      { file: 'wasm/LICENSE_JBIG2', identifier: 'BSD-3-Clause' },
      { file: 'wasm/LICENSE_PDFJS_JBIG2', identifier: 'Apache-2.0' },
    ],
  },
  {
    files: 'virtual:pdfjs-data/wasm/openjpeg',
    name: 'pdfjs-dist JPEG 2000 decoder (OpenJPEG)',
    licenses: [
      { file: 'wasm/LICENSE_OPENJPEG', identifier: 'BSD-2-Clause' },
      { file: 'wasm/LICENSE_PDFJS_OPENJPEG', identifier: 'BSD-2-Clause' },
    ],
  },
  {
    files: 'virtual:pdfjs-data/wasm/qcms',
    name: 'pdfjs-dist colour management (qcms)',
    licenses: [
      { file: 'wasm/LICENSE_QCMS', identifier: 'MIT' },
      { file: 'wasm/LICENSE_PDFJS_QCMS', identifier: 'MIT' },
    ],
  },
];

/** The notices of the data sets that modules of the bundle were made from. */
export function pdfjsDataLicenses(bundledModules: readonly string[]): PackageLicense[] {
  const used = SETS.filter((set) => bundledModules.some((id) => id.includes(set.files)));
  if (used.length === 0) return [];
  const directory = packageDirectory('pdfjs-dist');
  const { version } = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8')) as {
    version: string;
  };
  return used.map((set) => ({
    name: set.name,
    version,
    identifier: [...new Set(set.licenses.map((license) => license.identifier))].join(' AND '),
    text: set.licenses
      .map((license) => readFileSync(path.join(directory, license.file), 'utf8').trim())
      .join('\n\n'),
  }));
}
