#!/usr/bin/env node
/**
 * Brings rhwp-studio, the editor of rhwp, into this package from rhwp's
 * repository at the tag of one version: the same version as `@rhwp/core`, the
 * engine the web app installs from npm (the editor is not published there).
 *
 *   node vendor/rhwp-studio/import.mjs <version>        from the repository root
 *
 * It clones the tag into `target/rhwp-src/v<version>` (the files it needs
 * alone; a folder that is already there is used as it is), runs nothing from
 * that folder, and copies into `upstream/`, under the addresses they have in
 * rhwp's repository:
 *
 * - the editor's source, its page and the two scripts its page starts with;
 * - what that source imports from outside its folder, the PDF import left out;
 * - rhwp's licence.
 *
 * Only code is copied by its kind. Every other file (data, pictures, fonts)
 * has to be named in `assets.json` with where it comes from and its licence:
 * one that is not stops the import, and so does a symbolic link. No font, no
 * sample document, no test and no document in Markdown is brought.
 *
 * It then writes the version and the commit into `package.json` and the
 * SHA-256 of every file as rhwp has it into `upstream.json`. A file of
 * `upstream/` that is not as rhwp has it is a change of ours: `package.json`
 * names each (`rhwp.patched`) and the web app's tests fail on one it does not
 * name. An import writes rhwp's files over ours, so the changes are made
 * again afterwards from their commits (`git log -- vendor/rhwp-studio/upstream`);
 * the import says which files they were.
 *
 * The engine and the editor move together: after an import, `@rhwp/core` in
 * `apps/web/package.json` and here is set to the same version, and
 * `pnpm install` run. The web app's check fails while they differ.
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';

const REPOSITORY = 'https://github.com/edwardkim/rhwp';
const HERE = import.meta.dirname;
const ROOT = path.resolve(HERE, '../..');
const UPSTREAM = path.join(HERE, 'upstream');

/** What is brought: whole folders, and single files. */
const FOLDERS = ['rhwp-studio/src'];
const FILES = [
  'LICENSE',
  'rhwp-studio/index.html',
  'rhwp-studio/public/theme-init.js',
  'rhwp-studio/public/locale-init.js',
  'rhwp-shared/forms/action-runtime.ts',
  'assets/fonts/enclosed-pua-metrics.json',
];
/** What is left out of the folders: tests, and documents (the repository's own grow no more). */
const LEFT_OUT = [/\.test\.ts$/, /\.md$/];
/** The kinds of file that are code, copied without being named. */
const CODE = new Set(['.ts', '.mts', '.mjs', '.js', '.css', '.html']);

const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
const posix = (file) => file.split(path.sep).join('/');

function fail(message) {
  console.error(`import: ${message}`);
  process.exit(1);
}

/** Every file under `folder`, from `base`; stops on a symbolic link. */
function filesUnder(base, folder) {
  const found = [];
  for (const entry of readdirSync(path.join(base, folder), { withFileTypes: true })) {
    const at = path.join(folder, entry.name);
    if (entry.isSymbolicLink()) fail(`${posix(at)} is a symbolic link`);
    if (entry.isDirectory()) found.push(...filesUnder(base, at));
    else if (entry.isFile()) found.push(posix(at));
  }
  return found.sort();
}

/** The tag's files this import reads, checked out into `folder` and nothing else. */
function clone(version, folder) {
  const git = (...args) => execFileSync('git', args, { stdio: 'inherit' });
  git(
    'clone',
    '--filter=blob:none',
    '--no-checkout',
    '--depth',
    '1',
    '--branch',
    `v${version}`,
    `${REPOSITORY}.git`,
    folder,
  );
  git('-C', folder, 'sparse-checkout', 'set', '--no-cone', ...FOLDERS.map((at) => `/${at}/`), ...FILES.map((at) => `/${at}`), '/rhwp-studio/package.json');
  git('-C', folder, 'checkout');
}

/**
 * The files to bring from a checkout, each as code, the licence, or an asset
 * the list names; throws on a file that is none of them, and on an asset the
 * list names that is not there.
 */
export function chosen(source, assets) {
  const files = [
    ...FOLDERS.flatMap((folder) => filesUnder(source, folder)),
    ...FILES.filter((file) => {
      if (!existsSync(path.join(source, file))) throw new Error(`${file} is not in rhwp's source`);
      if (lstatSync(path.join(source, file)).isSymbolicLink()) {
        throw new Error(`${file} is a symbolic link`);
      }
      return true;
    }),
  ].filter((file) => !LEFT_OUT.some((pattern) => pattern.test(file)));
  const unnamed = files.filter(
    (file) => file !== 'LICENSE' && !CODE.has(path.extname(file)) && !(file in assets),
  );
  if (unnamed.length > 0) {
    throw new Error(
      `files that are not code and that assets.json does not name (give each its origin and licence, or leave it out):\n  ${unnamed.join('\n  ')}`,
    );
  }
  const said = (value) => typeof value === 'string' && value.trim() !== '';
  const bare = Object.entries(assets)
    .filter(([, asset]) => !said(asset?.origin) || !said(asset?.license))
    .map(([file]) => file);
  if (bare.length > 0) {
    throw new Error(`assets.json names files without their origin or licence: ${bare.join(', ')}`);
  }
  const gone = Object.keys(assets).filter((file) => !files.includes(file));
  if (gone.length > 0) throw new Error(`assets.json names files that are not brought: ${gone.join(', ')}`);
  return files.sort();
}

function main() {
  const version = process.argv[2];
  if (!version || !/^\d+\.\d+\.\d+$/.test(version)) fail('usage: node vendor/rhwp-studio/import.mjs <version>');
  const source = path.join(ROOT, 'target/rhwp-src', `v${version}`);
  if (!existsSync(source)) clone(version, source);
  const at = (name) => {
    try {
      return execFileSync('git', ['-C', source, 'rev-parse', '--verify', `${name}^{commit}`], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim();
    } catch {
      return undefined;
    }
  };
  const commit = at('HEAD');
  // A folder left from before is used only as the tag has it.
  if (!commit || commit !== at(`refs/tags/v${version}`)) {
    fail(`target/rhwp-src/v${version} is not at the tag v${version}: remove it and import again`);
  }
  const recorded = JSON.parse(readFileSync(path.join(HERE, 'package.json'), 'utf8')).rhwp;
  if (recorded?.tag === `v${version}` && recorded.commit && recorded.commit !== commit) {
    fail(`the tag v${version} is now ${commit}, and was ${recorded.commit} when it was brought`);
  }
  const theirs = JSON.parse(readFileSync(path.join(source, 'rhwp-studio/package.json'), 'utf8'));
  if (theirs.version !== version) fail(`the tag's rhwp-studio is ${theirs.version}, not ${version}`);

  const assets = JSON.parse(readFileSync(path.join(HERE, 'assets.json'), 'utf8'));
  let files;
  try {
    files = chosen(source, assets);
  } catch (error) {
    fail(error.message);
  }

  const manifestFile = path.join(HERE, 'package.json');
  const manifest = JSON.parse(readFileSync(manifestFile, 'utf8'));
  const before = existsSync(path.join(HERE, 'upstream.json'))
    ? JSON.parse(readFileSync(path.join(HERE, 'upstream.json'), 'utf8'))
    : {};
  const ours = Object.keys(before).filter((file) => {
    const at = path.join(UPSTREAM, file);
    return !existsSync(at) || sha256(at) !== before[file];
  });

  rmSync(UPSTREAM, { recursive: true, force: true });
  const hashes = {};
  for (const file of files) {
    const to = path.join(UPSTREAM, file);
    mkdirSync(path.dirname(to), { recursive: true });
    copyFileSync(path.join(source, file), to);
    hashes[file] = sha256(to);
  }
  writeFileSync(path.join(HERE, 'upstream.json'), `${JSON.stringify(hashes, null, 2)}\n`);
  manifest.version = version;
  manifest.dependencies['@rhwp/core'] = version;
  manifest.rhwp = { ...manifest.rhwp, repository: REPOSITORY, tag: `v${version}`, commit };
  writeFileSync(manifestFile, `${JSON.stringify(manifest, null, 2)}\n`);

  console.log(`import: rhwp-studio ${version} (${commit}), ${files.length} files`);
  if (ours.length > 0) {
    console.log(
      `import: these files had changes of ours, now written over; make them again from their commits:\n  ${ours.join('\n  ')}`,
    );
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === import.meta.filename) main();
