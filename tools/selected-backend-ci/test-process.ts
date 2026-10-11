import { spawnSync } from "bun";
import {
  closeSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Copies the runner modules (this directory and drivers/, no tests) to
// `destination`, the tools/selected-backend-ci of a probe checkout: runtime.ts
// imports the lane driver modules.
export function copyRunnerModules(destination: string): void {
  for (const part of ["", "drivers"]) {
    mkdirSync(join(destination, part), { recursive: true, mode: 0o755 });
    for (const name of readdirSync(join(import.meta.dir, part)))
      if (name.endsWith(".ts") && !name.endsWith(".test.ts"))
        copyFileSync(join(import.meta.dir, part, name), join(destination, part, name));
  }
}

// spawnSync for the sudo/setpriv test children, output captured in files.
// Known: when these suites shared one Bun 1.4.2 `bun test --parallel` run
// with ./src, a test worker intermittently spun in userspace (voluntary
// context switches flat) inside spawnSync while its direct sudo child stayed
// a zombie. The capture files were the open stdout/stderr, so pipes are not
// the cause. The pidfd was registered EPOLLIN in the spawnSync epoll and its
// fdinfo named the zombie. Unknown: the Bun code path. apps/web `test` runs
// these suites in their own `bun test` invocation, where it did not recur.
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
