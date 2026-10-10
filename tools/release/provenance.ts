// Stamp a rendered release directory with the tooling commit (docs/RELEASING.md).
//
// The product (image, compose.yml, env.example, INSTALL.md, notes text,
// version, labels) comes from the tagged commit, rendered by that commit's
// scripts/release-dist.sh. The smoke that exercises it is test tooling and
// comes from the ref the workflow run started from (the tag itself on a tag
// push, the dispatching branch on workflow_dispatch). This records the second
// commit next to the first so both are explicit:
//
//   release.json      "toolingSha" and "toolingRef" added; "sourceSha" unchanged
//   RELEASE-NOTES.md  a provenance section naming both commits
//   SHA256SUMS        recomputed over the same five files
//
// It runs from the workflow ref, so it also stamps directories rendered by the
// release-dist.sh of an older tag. A directory is stamped once.
//
//   bun tools/release/provenance.ts --dist DIR --tooling-sha <40-hex> --tooling-ref REF
// Exit 2 for a usage error, 1 for a refused directory or input.
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs as parseUtilArgs } from "node:util";
import { ReleaseError, dumpJson, isRecord, readText, repr, sorted, splitLines } from "./py.ts";

export const SUMMED = [
  "compose.yml",
  "env.example",
  "INSTALL.md",
  "release.json",
  "RELEASE-NOTES.md",
] as const;
const SHA_RE = /^[0-9a-f]{40}$/;
const REF_RE = /^refs\/(heads|tags)\/[A-Za-z0-9._/-]+$/;

class UsageError extends Error {}

const USAGE = "usage: provenance.ts --dist DIR --tooling-sha TOOLING_SHA --tooling-ref TOOLING_REF";

export type Args = { dist: string; toolingSha: string; toolingRef: string };

// --name VALUE or --name=VALUE, each option exactly once, nothing else.
export function parseArgs(argv: string[]): Args {
  let parsed: ReturnType<typeof parseCli>;
  try {
    parsed = parseCli(argv);
  } catch (error) {
    throw new UsageError(error instanceof Error ? error.message : String(error));
  }
  const { values, tokens } = parsed;
  if (values.help) throw new UsageError("help");
  const seen = new Set<string>();
  for (const token of tokens) {
    if (token.kind !== "option") continue;
    if (seen.has(token.name)) throw new UsageError(`--${token.name} given twice`);
    seen.add(token.name);
  }
  const missing = ["dist", "tooling-sha", "tooling-ref"].filter((name) => !seen.has(name));
  if (missing.length) {
    throw new UsageError(`missing ${missing.map((name) => `--${name}`).join(", ")}`);
  }
  return {
    dist: values.dist ?? "",
    toolingSha: values["tooling-sha"] ?? "",
    toolingRef: values["tooling-ref"] ?? "",
  };
}

function parseCli(argv: string[]) {
  return parseUtilArgs({
    args: argv,
    strict: true,
    allowPositionals: false,
    tokens: true,
    options: {
      dist: { type: "string" },
      "tooling-sha": { type: "string" },
      "tooling-ref": { type: "string" },
      help: { type: "boolean", short: "h" },
    },
  });
}

function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export function checkSums(dist: string): void {
  const sums = new Map<string, string>();
  for (const line of splitLines(readText(join(dist, "SHA256SUMS")))) {
    const at = line.indexOf("  ");
    const name = at < 0 ? "" : line.slice(at + 2);
    if (sums.has(name)) throw new ReleaseError(`SHA256SUMS lists ${repr(name)} twice`);
    sums.set(name, at < 0 ? line : line.slice(0, at));
  }
  const listed = sorted(sums.keys());
  const expected = sorted(SUMMED);
  if (listed.length !== expected.length || listed.some((name, i) => name !== expected[i])) {
    throw new ReleaseError(`SHA256SUMS lists ${repr(listed)}, expected ${repr(expected)}`);
  }
  for (const [name, digest] of sums) {
    if (sha256(readFileSync(join(dist, name))) !== digest) {
      throw new ReleaseError(`SHA256SUMS does not match ${name}`);
    }
  }
}

export function stamp(dist: string, toolingSha: string, toolingRef: string): string {
  if (!SHA_RE.test(toolingSha)) throw new ReleaseError("--tooling-sha must be a full commit SHA");
  if (!REF_RE.test(toolingRef)) {
    throw new ReleaseError("--tooling-ref must be refs/heads/<name> or refs/tags/<name>");
  }
  checkSums(dist);
  const recordPath = join(dist, "release.json");
  const record: unknown = JSON.parse(readText(recordPath));
  if (!isRecord(record)) throw new ReleaseError("release.json is not an object");
  const { sourceSha, version } = record;
  if (typeof sourceSha !== "string" || !SHA_RE.test(sourceSha)) {
    throw new ReleaseError("release.json has no sourceSha");
  }
  if (Object.hasOwn(record, "toolingSha") || Object.hasOwn(record, "toolingRef")) {
    throw new ReleaseError("release.json is already stamped");
  }
  if (typeof version !== "string") throw new ReleaseError("release.json has no version");
  record.toolingSha = toolingSha;
  record.toolingRef = toolingRef;
  writeFileSync(recordPath, dumpJson(record, { indent: 2 }) + "\n");

  const notesPath = join(dist, "RELEASE-NOTES.md");
  const notes =
    readText(notesPath).replace(/\n+$/, "") +
    "\n\n## Provenance\n\n" +
    `- Product (image build, compose.yml, env.example, INSTALL.md, these notes): ` +
    `\`${sourceSha}\` (tag v${version})\n` +
    `- Release smoke tooling: \`${toolingSha}\` (${toolingRef})\n`;
  writeFileSync(notesPath, notes);

  writeFileSync(
    join(dist, "SHA256SUMS"),
    SUMMED.map((name) => `${sha256(readFileSync(join(dist, name)))}  ${name}\n`).join(""),
  );
  return `stamped ${dist}: product ${sourceSha}, tooling ${toolingSha} (${toolingRef})`;
}

export function main(argv: string[]): number {
  let args: Args;
  try {
    args = parseArgs(argv);
  } catch (error) {
    if (!(error instanceof UsageError)) throw error;
    if (error.message === "help") {
      process.stdout.write(`${USAGE}\n`);
      return 0;
    }
    process.stderr.write(`${USAGE}\nprovenance.ts: error: ${error.message}\n`);
    return 2;
  }
  try {
    process.stdout.write(stamp(args.dist, args.toolingSha, args.toolingRef) + "\n");
    return 0;
  } catch (error) {
    process.stderr.write(
      `release-provenance: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    return 1;
  }
}

if (import.meta.main) process.exitCode = main(process.argv.slice(2));
