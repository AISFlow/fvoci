// The real configListInputs as the fixed 1000:1000 runtime actor, through
// setpriv, from a 0755 copy of the runner modules. Linux with sudo only;
// elsewhere it is skipped and counts as NOTRUN.
import { spawnSync } from "bun";
import { expect, test } from "bun:test";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

const linux = process.platform === "linux";

function asActor(actor: number): Record<string, string> {
  const probe = mkdtempSync(join(tmpdir(), "fvoci-config-list-probe-"));
  try {
    chmodSync(probe, 0o755);
    const tools = join(probe, "tools/selected-backend-ci");
    mkdirSync(tools, { recursive: true });
    for (const name of readdirSync(import.meta.dir))
      if (name.endsWith(".ts") && !name.endsWith(".test.ts"))
        copyFileSync(join(import.meta.dir, name), join(tools, name));
    const bun = join(probe, "bun");
    copyFileSync(process.execPath, bun);
    chmodSync(bun, 0o755);
    const result = spawnSync(
      [
        "sudo",
        "-n",
        "setpriv",
        `--reuid=${String(actor)}`,
        `--regid=${String(actor)}`,
        "--clear-groups",
        "env",
        "-i",
        // The runtime Bun is found on PATH, as in the CI footer.
        `PATH=${probe}:/usr/bin:/bin`,
        "TMPDIR=/tmp",
        bun,
        join(tools, "config-list.fixture.ts"),
      ],
      { cwd: "/", stdout: "pipe", stderr: "pipe" },
    );
    expect(result.exitCode, result.stderr.toString()).toBe(0);
    return JSON.parse(result.stdout.toString()) as Record<string, string>;
  } finally {
    rmSync(probe, { recursive: true, force: true });
  }
}

test.skipIf(!linux)("config-list admits only the exact consumed cohort", () => {
  const results = asActor(1000);
  expect(results).toEqual({
    baseline: "admitted",
    "tracked source changed": "refused:AssertionError",
    "access receipt groups differ": "refused:AssertionError",
    "received file mode changed": "refused:AssertionError",
    "private browser mode changed": "refused:AssertionError",
    "prepare phase": "refused:AssertionError",
    "orca-local execution": "refused:AssertionError",
    "runtime directory exists": "refused:AssertionError",
    "CLI missing from recorded inputs": "refused:AssertionError",
    "CLI is a symlink": "refused:AssertionError",
    "playwright bin is not the official CLI": "refused:AssertionError",
    "CLI bytes changed": "refused:AssertionError",
    "consumed receipt missing": "refused:Error",
    "baseline after restores": "admitted",
    launched: "false",
  });
});
test.skipIf(!linux)("config-list refuses any actor other than 1000:1000", () => {
  // The cohort itself cannot be qualified for another actor; nothing admits.
  expect(Object.values(asActor(1001))).not.toContain("admitted");
});
