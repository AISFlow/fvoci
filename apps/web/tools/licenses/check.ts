/**
 * Checks the licence of every installed package, tools included.
 *
 *   node tools/licenses/check.ts
 *
 * A licence has to be on the list in the repository's `deny.toml`, or the
 * package has to be listed in exceptions.json with that licence and a reason:
 * under `tooling` when it only runs on the build machine, under `notShipped`
 * when it is installed along with something else and the build leaves it out.
 * A package whose manifest names no licence may be listed under `stated` with
 * the licence its own licence file states; the file is read to check. An
 * exception that matches nothing is an error too, so the list cannot outlive
 * what it was written for. The build applies the same list to what ships,
 * without the first two kinds of exception (notices.ts).
 */
import { execFileSync } from 'node:child_process';
import path from 'node:path';

import exceptions from './exceptions.json' with { type: 'json' };
import { licenseFileText, statedLicense } from './packages.ts';
import { isAllowed, readAllowedLicenses } from './policy.ts';

interface ListedPackage {
  name: string;
  versions: string[];
  /** Where each installed version lies. */
  paths: string[];
  license: string;
}

/** Installed and not in what ships, for one of two reasons. */
const EXCEPTED = [...exceptions.tooling, ...exceptions.notShipped];

function matchesName(pattern: string, name: string): boolean {
  return pattern.endsWith('*') ? name.startsWith(pattern.slice(0, -1)) : pattern === name;
}

function installedPackages(): Record<string, ListedPackage[]> {
  const pnpm = process.env.npm_execpath ?? 'pnpm';
  const output = execFileSync(pnpm, ['licenses', 'list', '--json'], {
    cwd: path.resolve(import.meta.dirname, '../..'),
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
  });
  return JSON.parse(output) as Record<string, ListedPackage[]>;
}

const allowed = new Set(readAllowedLicenses());
const used = new Set<number>();
const usedStated = new Set<string>();
const refused: string[] = [];
const counts: [string, number, string][] = [];

for (const [license, packages] of Object.entries(installedPackages())) {
  if (isAllowed(license, allowed)) {
    counts.push([license, packages.length, 'allowed']);
    // The listing reads a licence out of the licence file when the manifest
    // names none; the report of the build does not, and takes it from
    // `stated`. So an entry for a package listed here is in use as well.
    for (const item of packages) {
      const inFile = item.paths.map((directory) =>
        statedLicense(item.name, licenseFileText(directory)),
      );
      if (inFile.length > 0 && inFile.every((found) => found === license)) {
        usedStated.add(item.name);
      }
    }
    continue;
  }
  let excepted = 0;
  let stated = 0;
  for (const item of packages) {
    // No licence in the manifest, one in the licence file: the file decides.
    const inFile = item.paths.map((directory) =>
      statedLicense(item.name, licenseFileText(directory)),
    );
    if (inFile.length > 0 && inFile.every((found) => found && isAllowed(found, allowed))) {
      usedStated.add(item.name);
      stated += 1;
      continue;
    }
    const index = EXCEPTED.findIndex(
      (entry) => entry.license === license && matchesName(entry.name, item.name),
    );
    if (index === -1) refused.push(`${item.name} ${item.versions.join(', ')}: ${license}`);
    else {
      used.add(index);
      excepted += 1;
    }
  }
  counts.push([
    license,
    packages.length,
    [
      ...(excepted > 0 || stated === 0 ? [`${excepted} excepted (tooling, or not shipped)`] : []),
      ...(stated > 0 ? [`${stated} stated in the licence file`] : []),
    ].join(', '),
  ]);
}

const stale = [
  ...EXCEPTED.filter((_, index) => !used.has(index)),
  ...exceptions.stated.filter((entry) => !usedStated.has(entry.name)),
];

for (const [license, count, verdict] of counts.sort((a, b) => b[1] - a[1])) {
  console.log(`${String(count).padStart(5)}  ${license.padEnd(36)} ${verdict}`);
}
if (refused.length > 0) {
  console.error(`\nlicences not allowed by deny.toml and not excepted:\n  ${refused.join('\n  ')}`);
}
if (stale.length > 0) {
  console.error(
    `\nexceptions that match no installed package (remove them):\n  ${stale
      .map((entry) => `${entry.name}: ${entry.license}`)
      .join('\n  ')}`,
  );
}
if (refused.length > 0 || stale.length > 0) process.exit(1);
console.log('\nlicences: ok');
