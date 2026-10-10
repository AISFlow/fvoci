import { spawnSync } from "bun";
import { closeSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Test children write stdout/stderr to files, not pipes. Under
// `bun test --parallel` with the src suite, Bun 1.4.2 spawnSync with piped
// output intermittently never reaps an exited sudo child and spins.
export function run(
  command: string[],
  cwd?: string,
): { exitCode: number; stdout: string; stderr: string } {
  const capture = mkdtempSync(join(tmpdir(), "fvoci-footer-capture-"));
  try {
    const out = openSync(join(capture, "stdout"), "w"),
      err = openSync(join(capture, "stderr"), "w");
    let exitCode: number;
    try {
      exitCode = spawnSync(command, { cwd, stdin: "ignore", stdout: out, stderr: err }).exitCode;
    } finally {
      closeSync(out);
      closeSync(err);
    }
    return {
      exitCode,
      stdout: readFileSync(join(capture, "stdout"), "utf8"),
      stderr: readFileSync(join(capture, "stderr"), "utf8"),
    };
  } finally {
    rmSync(capture, { recursive: true, force: true });
  }
}
