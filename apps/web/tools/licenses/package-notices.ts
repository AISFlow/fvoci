/**
 * Notices a package ships besides its licence file, for material in its
 * code that is under other terms. Each is added after the package's licence
 * in the notices, whenever the package is bundled.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { packageDirectory, type PackageLicense } from './packages.ts';

/** Each package's notices: the file under the package, and what of the package it is for. */
const NOTICES: Record<string, { file: string; about: string }[]> = {
  // The PowerPoint renderer of the file viewer draws preset shapes from a
  // table derived from the geometries of ECMA-376 (its THIRD_PARTY_NOTICES.md).
  '@aiden0z/pptx-renderer': [
    {
      file: 'licenses/ECMA-text-copyright-notice.txt',
      about: 'Its preset shape geometries are derived from ECMA-376 Part 1 (Office Open XML).',
    },
  ],
};

/** The package's licence, its other notices added to the text. */
export function withPackageNotices(item: PackageLicense): PackageLicense {
  const notices = NOTICES[item.name];
  if (!notices) return item;
  const directory = packageDirectory(item.name);
  const added = notices.map(
    ({ file, about }) => `${about}\n\n${readFileSync(path.join(directory, file), 'utf8').trim()}`,
  );
  return { ...item, text: [item.text, ...added].join('\n\n') };
}
