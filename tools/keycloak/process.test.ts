import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { safeText } from "./run.ts";

test.each(["normal", "failure", "spawn-failure", "interrupt", "broken-output"])(
  "runner drains and retires an actual %s child",
  async (mode) => {
    const directory = mkdtempSync(join(tmpdir(), "keycloak-process-"));
    try {
      const child = Bun.spawn(
        [
          process.execPath,
          "--no-env-file",
          join(import.meta.dir, "process.fixture.ts"),
          "control",
          mode,
          directory,
        ],
        {
          env: { PATH: process.env.PATH ?? "" },
          stdout: "pipe",
          stderr: "pipe",
        },
      );
      if (mode === "broken-output") await child.stderr.cancel();
      const [stdout, stderr, code] = await Promise.all([
        new Response(child.stdout).text(),
        mode === "broken-output" ? Promise.resolve("") : new Response(child.stderr).text(),
        child.exited,
      ]);
      expect(code, stderr).toBe(0);
      const value = JSON.parse(stdout) as {
        result: string;
        code: number | null;
        captured: string;
        closed: boolean;
        identitiesRetired: boolean;
      };
      expect(value.closed).toBe(true);
      expect(value.identitiesRetired).toBe(true);
      expect(value.result).toBe(
        mode === "interrupt" ? "interrupted" : mode === "spawn-failure" ? "spawn-failed" : "exited",
      );
      if (mode !== "spawn-failure") {
        expect(value.captured).toContain("stderr-collected");
        expect(value.captured).not.toContain("stdout-canary-secret");
      }
      if (mode === "interrupt") expect(value.captured).toContain("terminated-and-drained");
      if (mode === "normal" || mode === "failure")
        expect(value.code).toBe(mode === "normal" ? 0 : 7);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  },
);

test("retained diagnostics exclude connection URLs and hosts in addition to OAuth values", () => {
  expect(
    safeText(
      "https://provider.invalid/cb?code=abc123&state=abc123 127.0.0.1:5432 postgres://u:p@db.invalid/app\n",
    ),
  ).toBe("<redacted-url> <redacted-host> postgres://<redacted>\n");
});
