/**
 * The packages bundled into the app's workers. Vite builds each worker on
 * its own, and one that is inlined (`?worker&inline`, the file viewer's)
 * reaches the bundle as text: the licence report of the bundle does not see
 * what is in it. This plugin runs in every worker's build (`worker.plugins`
 * in vite.config.ts) and notes the package of each module the worker
 * bundles; the notices step lists them, and checks their licences, with the
 * bundle's (notices.ts). The workers are built before the bundle is written.
 */
import path from 'node:path';

import type { Plugin } from 'vite';

import { readPackageLicenseAt, type PackageLicense } from './packages.ts';

/** The directories of the packages the workers bundled, in this build. */
const found = new Set<string>();

const NODE_MODULES = '/node_modules/';

/** The directory of the package a module's file is in, if it is in one. */
export function packageDirectoryOf(id: string): string | undefined {
  if (id.startsWith('\0')) return undefined;
  const file = (id.split('?')[0] ?? '').split(path.sep).join('/');
  const at = file.lastIndexOf(NODE_MODULES);
  if (at < 0) return undefined;
  const segments = file.slice(at + NODE_MODULES.length).split('/');
  const name = segments[0]?.startsWith('@') ? segments.slice(0, 2) : segments.slice(0, 1);
  if (name.length === 0 || name.some((segment) => segment === '')) return undefined;
  return `${file.slice(0, at)}${NODE_MODULES}${name.join('/')}`;
}

export function workerPackages(): Plugin {
  return {
    name: 'fvoci:worker-packages',
    apply: 'build',
    generateBundle(_options, bundle) {
      for (const item of Object.values(bundle)) {
        if (item.type !== 'chunk') continue;
        for (const id of item.moduleIds) {
          const directory = packageDirectoryOf(id);
          if (directory) found.add(directory);
        }
      }
    },
  };
}

/** The licences of the packages the workers of this build bundled. */
export function bundledWorkerPackages(): PackageLicense[] {
  return [...found].map((directory) => readPackageLicenseAt(directory));
}
