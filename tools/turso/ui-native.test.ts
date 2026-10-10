import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { sha, write } from "../selected-backend-ci/io.ts";
import { OFF, ON } from "./ui-audit.ts";
import { failureCode, UiError, type Record_ } from "./ui-common.ts";
import { Captured, exitedChild, failureOf, fakeScope, must } from "./ui-fakes.ts";
import { browser, fixture, stop } from "./ui-native.ts";
import type { Manifest } from "./ui-record.ts";

let directory: string;
const saved = { ...process.env };
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "fvoci-ui-native-"));
  delete process.env.FVOCI_SELECTED_EXECUTION_MODE;
});
afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
  for (const key of Object.keys(process.env))
    if (!(key in saved)) Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, saved);
});

const fixtureManifest = {
  binaries: { "fvoci-e2e-fixture": { path: "/never/executed" } },
} as unknown as Manifest;
const baselinePacket = (): Record_ => ({
  originalFailure: "TURSO_UI_BASELINE_FAILED",
  nativeOutcome: {
    operation: "failed",
    rollback: "unknown",
    commit: "not-attempted",
    baselineFailure: {
      phase: "table-contract",
      category: "protocol",
      tableComparison: {
        expectedCount: 99,
        actualCount: 98,
        setEqual: false,
        orderEqual: false,
        actualOnlyCount: 0,
        expectedOnlyCount: 1,
        actualOnlyUnderscoreCount: 0,
        firstMismatchIndex: 0,
        actualMismatchExpectedIndex: 1,
      },
    },
    baselineRollbackFailure: { phase: "rollback", category: "request" },
  },
  lifecycleDrain: "unconfirmed",
  drainOutcome: "failed",
  rows: ["PRIVATE_ROW_CANARY"],
  token: "PRIVATE_AUTH_CANARY",
});

describe("native fixture", () => {
  test("baseline nonzero publishes before the packet write and process cleanup", async () => {
    for (const fault of ["none", "write", "finish", "both", "diagnostic-write"]) {
      const events: string[] = [];
      const output = new Captured();
      let first = true;
      const { scope, calls } = fakeScope({
        spawn: () => exitedChild(7, JSON.stringify(baselinePacket())),
        root: () => directory,
        output: {
          out: (line) => {
            output.out(line);
          },
          err: (line) => {
            events.push("diagnostic");
            if (fault === "diagnostic-write" && first) {
              first = false;
              throw new Error("PRIVATE_STDERR_WRITE_CANARY");
            }
            output.err(line);
          },
        },
        write: (path, value) => {
          events.push("write");
          if (fault === "write" || fault === "both") throw new Error("PRIVATE_WRITE_CANARY");
          write(path, value);
        },
        finish: () => {
          events.push("finish");
          if (fault === "finish" || fault === "both")
            throw new UiError("UI_PROCESS_CLOSURE_FAILED");
          return 7;
        },
      });
      let caught: unknown;
      try {
        await fixture(scope, fixtureManifest, "baseline", {});
      } catch (error) {
        caught = error;
      }
      expect((caught as Error).message).toBe("TURSO_UI_BASELINE_FAILED");
      expect(events.slice(0, 3)).toEqual(["diagnostic", "write", "finish"]);
      expect(calls.finish).toHaveLength(1);
      expect(output.all).not.toContain("PRIVATE_");
      if (fault !== "diagnostic-write") {
        const published = JSON.parse(output.stderr[0] as string) as Record<string, Record_>;
        expect(published.baselineFailure).toEqual(
          (baselinePacket().nativeOutcome as Record<string, unknown>).baselineFailure as Record_,
        );
        expect(must(published.nativeOutcome).rollback).toBe("unknown");
      } else expect(output.all).toContain("UI_NATIVE_BASELINE_DIAGNOSTIC_WRITE_FAILED");
      for (const name of readdirSync(directory)) rmSync(join(directory, name));
    }
  });

  test("native nonzero precedes finish and packet write failures", async () => {
    const packet = {
      originalFailure: "TURSO_UI_ACTOR_FAILED",
      nativeOutcome: { operation: "failed", rollback: "unknown", commit: "not-attempted" },
      lifecycleDrain: "unconfirmed",
      leases: 1,
    };
    for (const fault of ["none", "finish", "write", "both", "malformed", "invalid-code"]) {
      const run = mkdtempSync(join(directory, "run-"));
      const payload =
        fault === "invalid-code" ? { ...packet, originalFailure: "PRIVATE_SDK_DETAIL" } : packet;
      const output = new Captured();
      const { scope, calls } = fakeScope({
        spawn: () => exitedChild(7, fault === "malformed" ? "not-json" : JSON.stringify(payload)),
        root: () => run,
        output,
        finish: () => {
          if (fault === "finish" || fault === "both")
            throw new UiError("UI_PROCESS_CLOSURE_FAILED");
          return 7;
        },
        write: (path, value) => {
          if (fault === "write" || fault === "both") throw new Error("PRIVATE_WRITE_DETAIL");
          write(path, value);
        },
      });
      let actual = "accepted";
      try {
        await fixture(scope, fixtureManifest, "owner", {});
      } catch (error) {
        actual = failureCode(error);
      }
      expect(actual).toBe(
        fault === "malformed"
          ? "UI_CONSUMER_FAILED"
          : fault === "invalid-code"
            ? "UI_NATIVE_FAILURE_CODE_REFUSED"
            : "TURSO_UI_ACTOR_FAILED",
      );
      expect(calls.finish).toHaveLength(1);
      const receipts = readdirSync(run).filter((n) =>
        /^fixture-failure-.*\.private\.json$/.test(n),
      );
      expect(receipts).toHaveLength(fault === "none" || fault === "finish" ? 1 : 0);
      if (receipts.length)
        expect(
          (JSON.parse(readFileSync(join(run, must(receipts[0])), "utf8")) as Record_).receipt,
        ).toEqual(packet);
      if (["finish", "write", "both"].includes(fault)) {
        const diagnostic = JSON.parse(output.stderr[0] as string) as Record<string, unknown>;
        expect(diagnostic.originalFailure).toBe("TURSO_UI_ACTOR_FAILED");
        expect(
          (diagnostic.nativeCleanupErrors as string[]).includes(
            "UI_NATIVE_FAILURE_RECEIPT_WRITE_FAILED",
          ),
        ).toBe(fault === "write" || fault === "both");
      }
      for (const secret of ["PRIVATE_STDERR", "PRIVATE_WRITE_DETAIL", "PRIVATE_SDK_DETAIL"])
        expect(output.all).not.toContain(secret);
    }
  });

  test("the child receives only the allowlisted environment and the canonical input", async () => {
    for (const key of Object.keys(process.env)) Reflect.deleteProperty(process.env, key);
    process.env.PATH = "/pure/path";
    process.env.GITHUB_TOKEN = "PRIVATE_GITHUB_TOKEN";
    let seen: Record<string, string> = {};
    let fed = "";
    const ok = {
      lifecycleDrain: "confirmed",
      leases: 0,
      serverCloseReceipt: "not-exposed-by-sdk",
      rows: {},
    };
    const { scope } = fakeScope({
      spawn: (args, _label, options) => {
        expect(args).toEqual(["/never/executed", "observe"]);
        seen = options.env;
        const child = exitedChild(0, JSON.stringify(ok));
        (child as unknown as { stdin: unknown }).stdin = {
          write: (text: string) => {
            fed += text;
          },
          end: () => {},
        };
        return child;
      },
    });
    const value = await fixture(
      scope,
      fixtureManifest,
      "observe",
      { FVOCI_LIBSQL_URL: "u", FVOCI_LIBSQL_AUTH_TOKEN: "t", FVOCI_BIND: "x" },
      { workspaceId: "w", documentIds: ["d"] },
    );
    expect(value).toEqual(ok);
    expect(fed).toBe('{"documentIds":["d"],"workspaceId":"w"}');
    expect(Object.keys(seen).sort()).toEqual(
      [
        "E2E_DATABASE_BACKEND",
        "FVOCI_E2E_TURSO_UI_SELECTED",
        "FVOCI_LIBSQL_AUTH_TOKEN",
        "FVOCI_LIBSQL_URL",
        "PATH",
      ].sort(),
    );
  });

  test("a successful native helper with an unconfirmed drain is refused", async () => {
    const { scope } = fakeScope({
      spawn: () =>
        exitedChild(
          0,
          JSON.stringify({
            lifecycleDrain: "unconfirmed",
            leases: 0,
            serverCloseReceipt: "not-exposed-by-sdk",
          }),
        ),
    });
    expect(await failureOf(fixture(scope, fixtureManifest, "owner", {}))).toContain(
      "UI_NATIVE_DRAIN_FAILED",
    );
    expect(await failureOf(fixture(null, fixtureManifest, "owner", {}))).toContain(
      "UI_OWNED_PROCESS_SCOPE_REQUIRED",
    );
  });
});

interface BrowserInputs {
  workspace: string;
  cli: string;
  pkg: string;
  physical: string;
  manifest: Pick<Manifest, "bun" | "physicalInputs">;
  run: string;
}
function browserInputs(): BrowserInputs {
  const workspace = mkdtempSync(join(directory, "fvoci-turso-cli-pure-"));
  const cli = join(workspace, "node_modules/playwright/cli.js");
  mkdirSync(join(workspace, "node_modules/playwright"), { recursive: true });
  writeFileSync(cli, "/* pure fixture; never executed */");
  const pkg = join(workspace, "node_modules/playwright/package.json");
  writeFileSync(pkg, JSON.stringify({ version: "1.63.0", bin: { playwright: "cli.js" } }));
  const physical = join(workspace, "physical.private.json");
  write(physical, { files: { external: { [cli]: sha(cli), [pkg]: sha(pkg) } } });
  const manifest = {
    bun: { path: "/never-executed/admitted-bun", sha256: "", version: "" },
    physicalInputs: { path: physical, sha256: sha(physical) },
  };
  const run = join(workspace, "run");
  mkdirSync(run, 0o700);
  writeFileSync(join(run, "playwright.private.json"), '{"fixture":true}');
  for (const key of Object.keys(process.env)) Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, {
    PATH: "/pure/path",
    FVOCI_LIBSQL_URL: "PRIVATE_ENDPOINT",
    FVOCI_LIBSQL_AUTH_TOKEN: "PRIVATE_TOKEN",
    GITHUB_TOKEN: "PRIVATE_GITHUB_TOKEN",
    DATABASE_URL: "PRIVATE_DATABASE",
    PASSWORD_PEPPER_KEYS: "PRIVATE_PEPPER",
  });
  return { workspace, cli, pkg, physical, manifest, run };
}
const rewrite = (inputs: BrowserInputs, change: (external: Record<string, string>) => void) => {
  const value = JSON.parse(readFileSync(inputs.physical, "utf8")) as {
    files: { external: Record<string, string> };
  };
  change(value.files.external);
  writeFileSync(inputs.physical, JSON.stringify(value));
  inputs.manifest.physicalInputs.sha256 = sha(inputs.physical);
};

describe("browser", () => {
  test("the direct CLI keeps arguments, environment, wait and ownership", async () => {
    for (const [spec, grep] of [
      [ON, "^selected normal main:"],
      [OFF, undefined],
      [ON, "^selected normal main restart:"],
    ] as const) {
      const inputs = browserInputs();
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(0) });
      const result = await browser(
        scope,
        inputs.manifest,
        inputs.run,
        { FVOCI_E2E_SELECTED_FLOW: "on" },
        spec,
        grep,
        inputs.workspace,
      );
      expect(result).toEqual({ fixture: true });
      const [args, label, options] = must(calls.spawn[0]);
      const expected = [
        "/never-executed/admitted-bun",
        "--no-install",
        inputs.cli,
        "test",
        "--config",
        "e2e-pending/collab-playwright.config.ts",
        "--reporter=line,json",
      ];
      if (grep) expected.push("--grep", grep);
      expect(args).toEqual([...expected, "e2e-pending/" + spec]);
      expect(label).toBe("browser");
      expect(options.cwd).toBe(join(inputs.workspace, "apps/web"));
      expect(options.stderr).toBe(options.stdout);
      expect(Object.keys(options.env).sort()).toEqual(
        [
          "CI",
          "FVOCI_E2E_RESULT_DIR",
          "FVOCI_E2E_SELECTED_FLOW",
          "PATH",
          "PLAYWRIGHT_JSON_OUTPUT_FILE",
        ].sort(),
      );
      expect(options.env.CI).toBe("true");
      expect(JSON.stringify(args) + JSON.stringify(options.env)).not.toContain("PRIVATE_");
      expect(calls.finish).toEqual([
        [calls.spawn.length ? must(scope.allocations[0]).process : (null as never), undefined],
      ]);
      expect(statSync(join(inputs.run, "browser.private.log")).mode & 0o777).toBe(0o600);
      expect(statSync(join(inputs.run, "playwright.private.json")).mode & 0o777).toBe(0o600);
    }
  });

  test("a missing, symlinked, nonregular, drifted or unadmitted CLI is refused before the child", async () => {
    for (const fault of [
      "missing",
      "symlink",
      "directory",
      "drift",
      "unadmitted",
      "package-drift",
      "package-version",
      "package-bin",
      "receipt-drift",
    ]) {
      const inputs = browserInputs();
      if (fault === "missing") unlinkSync(inputs.cli);
      else if (fault === "symlink") {
        const target = join(inputs.workspace, "node_modules/playwright/actual.js");
        renameSync(inputs.cli, target);
        symlinkSync(target, inputs.cli);
      } else if (fault === "directory") {
        unlinkSync(inputs.cli);
        mkdirSync(inputs.cli);
      } else if (fault === "drift") writeFileSync(inputs.cli, "changed fixture bytes");
      else if (fault === "package-drift") writeFileSync(inputs.pkg, "{}");
      else if (fault === "package-version" || fault === "package-bin") {
        writeFileSync(
          inputs.pkg,
          JSON.stringify({
            version: fault === "package-version" ? "1.62.0" : "1.63.0",
            bin: { playwright: fault === "package-bin" ? "other.js" : "cli.js" },
          }),
        );
        rewrite(inputs, (external) => (external[inputs.pkg] = sha(inputs.pkg)));
      } else if (fault === "unadmitted")
        rewrite(inputs, (external) => Reflect.deleteProperty(external, inputs.cli));
      else writeFileSync(inputs.physical, "{}");
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(0) });
      const code =
        fault === "receipt-drift" ? "UI_PHYSICAL_RECEIPT_CHANGED" : "UI_PLAYWRIGHT_CLI_REFUSED";
      expect(
        await failureOf(
          browser(scope, inputs.manifest, inputs.run, {}, ON, undefined, inputs.workspace),
        ),
      ).toBe(code);
      expect(calls.spawn).toHaveLength(0);
      expect(existsSync(join(inputs.run, "browser.private.log"))).toBe(false);
    }
  });

  test("an explicit secret environment is refused before the child", async () => {
    for (const key of [
      "FVOCI_LIBSQL_URL",
      "FVOCI_LIBSQL_AUTH_TOKEN",
      "DATABASE_URL",
      "DATABASE_APP_URL",
      "PASSWORD_PEPPER_KEYS",
    ]) {
      const inputs = browserInputs();
      const { scope, calls } = fakeScope({ spawn: () => exitedChild(0) });
      expect(
        await failureOf(
          browser(
            scope,
            inputs.manifest,
            inputs.run,
            { [key]: "PRIVATE_VALUE" },
            ON,
            undefined,
            inputs.workspace,
          ),
        ),
      ).toContain("UI_BROWSER_SECRET_ENV_REFUSED");
      expect(calls.spawn).toHaveLength(0);
      expect(existsSync(join(inputs.run, "browser.private.log"))).toBe(false);
    }
  });

  test("a child failure keeps the original failure and finishes the owned process", async () => {
    const inputs = browserInputs();
    const output = new Captured();
    const { scope, calls } = fakeScope({ spawn: () => exitedChild(1), output });
    expect(
      await failureOf(
        browser(scope, inputs.manifest, inputs.run, {}, ON, undefined, inputs.workspace),
      ),
    ).toContain("UI_ACTUAL_BROWSER_FAILED");
    expect(calls.finish).toHaveLength(1);
    const failing = fakeScope({
      spawn: () => exitedChild(1),
      output,
      finish: () => {
        throw new UiError("UI_PROCESS_CLOSURE_FAILED");
      },
    });
    const again = browserInputs();
    expect(
      await failureOf(
        browser(failing.scope, again.manifest, again.run, {}, ON, undefined, again.workspace),
      ),
    ).toContain("UI_ACTUAL_BROWSER_FAILED");
    expect(JSON.parse(output.stderr[0] as string)).toEqual({
      originalFailure: "UI_ACTUAL_BROWSER_FAILED",
      browserProcessClosure: "UI_PROCESS_CLOSURE_FAILED",
    });
  });
});

describe("server stop", () => {
  test("the canonical restart projection keeps detailed identities private", async () => {
    const child = exitedChild(0, "", 10);
    const { scope } = fakeScope({ spawn: () => child });
    scope.spawn(["server"], "server", { env: {} });
    const row = { pid: 10, parentPid: 1, startTicks: "100", state: "S" };
    scope.capture(row, "server", 0);
    const result = await stop(scope, { child }, "http://127.0.0.1:12345", directory, () =>
      Promise.resolve(true),
    );
    expect(result).toEqual({ serverExit: 0, portClosed: true, recordedIdentitiesRetired: true });
    const packet = must(readdirSync(directory).find((n) => n.startsWith("server-identities-")));
    expect(
      (JSON.parse(readFileSync(join(directory, packet), "utf8")) as Record_).identities,
    ).toEqual([row]);
    expect(result).not.toHaveProperty("identities");
    const open = fakeScope({ spawn: () => child });
    open.scope.spawn(["server"], "server", { env: {} });
    open.scope.capture(row, "server", 0);
    expect(
      await failureOf(
        stop(open.scope, { child }, "http://127.0.0.1:12345", directory, () =>
          Promise.resolve(false),
        ),
      ),
    ).toContain("UI_SERVER_CLOSURE_FAILED");
  });
});
