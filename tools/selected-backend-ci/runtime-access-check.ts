// One fixed permissions preflight, run by the existing sudo/setpriv boundary.
// stdin is private; stdout contains counts and path hashes only.
import { deepEquals } from "bun";
import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import process from "node:process";
import { accessible, digest, gid, groups, uid } from "./io.ts";

export function preflight(request: { groups: number[]; files: Record<string, number> }) {
  assert.ok(uid() === 1000 && gid() === 1000 && deepEquals(groups(), request.groups));
  const missing = Object.entries(request.files)
    .filter(([p, mask]) => !accessible(p, mask))
    .map(([p]) => p);
  return {
    uid: uid(),
    gid: gid(),
    groups: groups(),
    checked: Object.keys(request.files).length,
    missing: missing.length,
    missing_path_sha256: missing.slice(0, 16).map((p) => digest(p)),
  };
}
if (import.meta.main) {
  try {
    const value: unknown = JSON.parse(readFileSync(0, "utf8"));
    const receipt = preflight(value as Parameters<typeof preflight>[0]);
    process.stdout.write(JSON.stringify(receipt) + "\n");
    process.exitCode = receipt.missing ? 1 : 0;
  } catch {
    process.stderr.write("runtime access preflight refused\n");
    process.exitCode = 1;
  }
}
