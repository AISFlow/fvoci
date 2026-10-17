// Local consumer admission as the fixed 1000:1000 runtime actor, through
// setpriv, from a 0755 copy of the runner modules, whatever the runner uid
// (GitHub runners are 1001). Linux with sudo only; elsewhere it is skipped and
// counts as NOTRUN.
import { afterAll, expect, test } from "bun:test";
import { chmodSync, copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { faults, purposes } from "./local-consumers.fixture.ts";
import type { Fault, Purpose } from "./local-consumers.fixture.ts";
import { copyRunnerModules, run } from "./test-process.ts";

const linux = process.platform === "linux";
const probe = mkdtempSync(join(tmpdir(), "fvoci-local-consumers-probe-"));
chmodSync(probe, 0o755);
const tools = join(probe, "tools/selected-backend-ci");
copyRunnerModules(tools);
const bun = join(probe, "bun");
copyFileSync(process.execPath, bun);
chmodSync(bun, 0o755);
afterAll(() => {
  rmSync(probe, { recursive: true, force: true });
});

function asActor(actor: number, purpose: Purpose, fault: Fault) {
  const result = run(
    [
      "sudo",
      "-n",
      "setpriv",
      `--reuid=${String(actor)}`,
      `--regid=${String(actor)}`,
      "--clear-groups",
      "env",
      "-i",
      `PATH=${probe}:/usr/bin:/bin`,
      "TMPDIR=/tmp",
      bun,
      join(tools, "local-consumers.fixture.ts"),
      purpose,
      fault,
    ],
    "/",
  );
  expect(result.exitCode, result.stderr).toBe(0);
  return JSON.parse(result.stdout) as { exitCode: number; stderr: string };
}

const admitted = (purpose: Purpose, fault: Fault) =>
  fault === "none" ||
  fault === "runtime" ||
  fault === "equal-work-selected" ||
  (fault === "sso-command" && purpose === "keycloak");

for (const purpose of purposes)
  for (const fault of faults) {
    if (purpose === "turso-ui" && fault.startsWith("runtime-observer")) continue;
    test.skipIf(!linux)(`${purpose} local allocation ${fault}`, () => {
      const result = asActor(1000, purpose, fault);
      expect(result.exitCode, result.stderr).toBe(admitted(purpose, fault) ? 0 : 23);
    });
    // The same admitted cohort, built and run end to end by another actor.
    if (admitted(purpose, fault))
      test.skipIf(!linux)(`${purpose} local allocation ${fault} checks actor 1001`, () => {
        const result = asActor(1001, purpose, fault);
        if (fault === "equal-work-selected") {
          expect(result.exitCode).toBe(0);
          return;
        }
        expect(result.exitCode).toBe(23);
        expect(result.stderr).toContain(
          "selected runtime actor must be the fixed handoff uid and gid",
        );
      });
  }
