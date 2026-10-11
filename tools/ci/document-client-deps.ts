// The process client (crates/document-extract-client) must not link the native
// parser stack; the documents workflow runs this over its cargo metadata.
//
//   bun tools/ci/document-client-deps.ts METADATA_JSON
//
// Exit 0 when no forbidden package is in the metadata's package list, 1 when
// one is or the file is not cargo metadata, 2 on a usage error.
import { readFileSync } from "node:fs";

export const FORBIDDEN_PACKAGES: readonly string[] = [
  "rhwp",
  "cfb",
  "zip",
  "flate2",
  "skia-safe",
  "skia-bindings",
];

const USAGE = "usage: document-client-deps.ts METADATA_JSON";

/**
 * The forbidden packages named in cargo metadata, sorted. `packages` lists
 * every package of the resolved graph, optional ones included, so a feature
 * that pulls in a native crate shows up here too.
 */
export function leakedPackages(metadata: unknown): string[] {
  if (typeof metadata !== "object" || metadata === null || Array.isArray(metadata)) {
    throw new Error("cargo metadata must be a JSON object");
  }
  const packages = (metadata as Record<string, unknown>).packages;
  if (!Array.isArray(packages) || packages.length === 0) {
    throw new Error("cargo metadata has no packages list");
  }
  const names = new Set<string>();
  for (const pkg of packages as unknown[]) {
    const name =
      typeof pkg === "object" && pkg !== null && !Array.isArray(pkg)
        ? (pkg as Record<string, unknown>).name
        : undefined;
    if (typeof name !== "string") throw new Error("cargo metadata package without a name");
    names.add(name);
  }
  return FORBIDDEN_PACKAGES.filter((name) => names.has(name)).sort();
}

export type Io = { read: (path: string) => Uint8Array; err: (text: string) => void };

const PROCESS_IO: Io = {
  read: (path) => readFileSync(path),
  err: (text) => process.stderr.write(text),
};

export function main(argv: readonly string[], { read, err }: Io = PROCESS_IO): number {
  const [path] = argv;
  if (argv.length !== 1 || path === undefined || path.startsWith("-")) {
    err(`${USAGE}\n`);
    return 2;
  }
  let leaked: string[];
  try {
    const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(read(path));
    leaked = leakedPackages(JSON.parse(text));
  } catch (error) {
    err(
      `document-client-deps: ${path}: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    return 1;
  }
  if (leaked.length > 0) {
    err(
      `document-client-deps: native dependencies leaked into process client: ${leaked.join(", ")}\n`,
    );
    return 1;
  }
  return 0;
}

if (import.meta.main) process.exitCode = main(Bun.argv.slice(2));
