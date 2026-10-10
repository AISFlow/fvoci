import { spawnSync } from "bun";
import { closeSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

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
