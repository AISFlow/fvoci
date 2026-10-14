// The auth-session lane purpose and the --local-auth consumer entry. Every
// admission child runs with an explicit expected actor (the current uid/gid),
// so the result does not depend on the runner account; production entries
// keep their fixed 1000:1000 default.
import { afterAll, describe, expect, test } from "bun:test";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { registeredModules, tursoUiModules } from "./admission.ts";
import { lanePurpose } from "./drivers/binding.ts";
import { digest, gid, root, sha, uid, write } from "./io.ts";
import { laneRetirement } from "./runtime.ts";
import type { LocalConsumerGrant, LocalGrant } from "./types.ts";

const scratch = mkdtempSync(join(tmpdir(), "fvoci-auth-admission-"));
afterAll(() => {
  rmSync(scratch, { recursive: true, force: true });
});
const actor = JSON.stringify([uid(), gid()]);

// Modules the auth lane purpose and the --local-auth entry newly register.
const laneAuthModules = [
  "tools/turso/ui-auth-session.ts",
  "tools/turso/ui-audit.ts",
  "tools/turso/ui-common.ts",
  "tools/web-e2e/compat.ts",
  "tools/web-e2e/smtp-sink.ts",
  "apps/web/e2e-pending/auth-session-contract.ts",
];
const consumerAuthModules = [
  "tools/turso/ui-auth-session.ts",
  "tools/web-e2e/smtp-sink.ts",
  "apps/web/e2e-pending/auth-session-playwright.config.ts",
  "apps/web/e2e-pending/auth-session-tsconfig.json",
  "apps/web/e2e-pending/auth-session-selected-backend.acceptance.ts",
  "apps/web/e2e-pending/auth-session-helpers.ts",
  "apps/web/e2e-pending/auth-session-contract.ts",
  "apps/web/e2e-pending/selected-backend-fixture.ts",
];

function command(argv: string[], cwd: string): string {
  const result = Bun.spawnSync(argv, { cwd, env: { PATH: process.env.PATH }, stderr: "pipe" });
  if (result.exitCode !== 0) throw new Error("isolated admission fixture command failed");
  return result.stdout.toString().trim();
}

// An isolated committed checkout: the real admission modules and hash-only
// witnesses for every other registered module.
function checkout() {
  const path = mkdtempSync(join(scratch, "checkout-"));
  const output = mkdtempSync(join(scratch, "output-"));
  for (const name of new Set([...tursoUiModules, ...registeredModules])) {
    const file = join(path, name);
    mkdirSync(dirname(file), { recursive: true });
    if (
      [
        "tools/selected-backend-ci/admission.ts",
        "tools/selected-backend-ci/io.ts",
        "tools/selected-backend-ci/types.ts",
      ].includes(name)
    )
      copyFileSync(join(root, name), file);
    else writeFileSync(file, "// isolated hash-only registration witness\n");
  }
  command(["git", "init", "--quiet"], path);
  command(["git", "add", "tools", "apps", "scripts"], path);
  command(
    [
      "git",
      "-c",
      "user.name=Admission fixture",
      "-c",
      "user.email=fixture@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "--quiet",
      "-m",
      "isolated source",
    ],
    path,
  );
  const identity = {
    schema: 1,
    status: "GRANTED",
    executionMode: "orca-local",
    exclusiveLocalBatch: true,
    currentDispatchConfirmed: true,
    owner: "orca:admission-fixture",
    runId: "run_ab12",
    taskId: "task_cd34",
    dispatchId: "ctx_ef56",
    workerTerminal: "fixture-worker",
    rootTerminal: "fixture-root",
    worktree: path,
    uid: uid(),
    gid: gid(),
    source: command(["git", "rev-parse", "HEAD"], path),
    tree: command(["git", "rev-parse", "HEAD^{tree}"], path),
    expiresUtc: "2099-01-01T00:00:00Z",
    outputRoot: output,
  };
  const env: Record<string, string> = {
    PATH: process.env.PATH ?? "",
    FVOCI_SELECTED_EXECUTION_MODE: "orca-local",
    FVOCI_SELECTED_LOCAL_ALLOCATION: join(output, "grant.json"),
    FVOCI_CI_OWNER: identity.owner,
    FVOCI_ROOT_RUN_OWNER: identity.owner,
    FVOCI_LOCAL_RUN_ID: identity.runId,
    FVOCI_LOCAL_TASK_ID: identity.taskId,
    FVOCI_LOCAL_DISPATCH_ID: identity.dispatchId,
    ORCA_TERMINAL_HANDLE: identity.workerTerminal,
    FVOCI_LOCAL_ROOT_TERMINAL: identity.rootTerminal,
    FVOCI_CI_SELECTED_RUNS: join(output, "runtime"),
    RUNNER_TEMP: output,
  };
  return { path, output, identity, env };
}

// Exit 0 when the entry admits, 23 with the refusal message when it refuses.
function admit(
  fixture: ReturnType<typeof checkout>,
  grant: object,
  call: string,
  entry: string,
): { code: number; refusal: string } {
  const file = join(fixture.output, "grant.json");
  writeFileSync(file, JSON.stringify(grant), { mode: 0o600 });
  const module = JSON.stringify(join(fixture.path, "tools/selected-backend-ci/admission.ts"));
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "--eval",
      `import { ${entry} } from ${module}; try { ${call}; process.exit(0); } catch (error) { process.stdout.write(String(error instanceof Error ? error.message : error)); process.exit(23); }`,
    ],
    {
      cwd: fixture.path,
      env: { ...fixture.env, FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256: sha(file) },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  return { code: result.exitCode, refusal: result.stdout.toString() };
}

describe("selected lanes admit exactly the auth-session purpose", () => {
  function lane(change: (grant: LocalGrant, path: string) => void = () => undefined) {
    const fixture = checkout();
    const grant: LocalGrant = {
      ...fixture.identity,
      purpose: "auth-session",
      allowedModes: ["run"],
      registrationHashes: Object.fromEntries(
        registeredModules.map((name) => [name, sha(join(fixture.path, name))]),
      ),
      stageCommands: {},
    };
    change(grant, fixture.path);
    return admit(fixture, grant, `localAllocation("run", ${actor})`, "localAllocation").code;
  }
  test("the auth-session lane grant is admitted", () => {
    expect(lane()).toBe(0);
  });
  test("the existing lane purposes stay admitted", () => {
    expect(lane((grant) => Reflect.deleteProperty(grant, "purpose"))).toBe(0);
    expect(lane((grant) => (grant.purpose = "equal-work"))).toBe(0);
  });
  for (const purpose of ["turso-ui", "keycloak", "auth", "AUTH-SESSION", "auth-session ", null])
    test(`an unknown lane purpose ${JSON.stringify(purpose)} is refused`, () => {
      expect(lane((grant) => Reflect.set(grant, "purpose", purpose))).toBe(23);
    });
  for (const name of laneAuthModules)
    for (const fault of ["changed", "missing"] as const)
      test(`a ${fault} ${name} lane hash is refused`, () => {
        expect(
          lane((grant) => {
            if (fault === "missing") Reflect.deleteProperty(grant.registrationHashes, name);
            else grant.registrationHashes[name] = "0".repeat(64);
          }),
        ).toBe(23);
      });
});

describe("the local Turso runtime argv selects exactly one consumer", () => {
  function consumerResult(
    run: string,
    mode: "--local-auth" | "--local-restore",
    change: (grant: LocalConsumerGrant, path: string) => void = () => undefined,
  ): { code: number; refusal: string } {
    const fixture = checkout();
    const build = join(fixture.output, "turso-ui");
    mkdirSync(build, { mode: 0o700 });
    const reference = (name: string) => {
      const path = join(build, name);
      writeFileSync(path, "{}\n", { mode: 0o600 });
      return { path, sha256: sha(path) };
    };
    const heavy = { path: process.execPath, sha256: sha(process.execPath) };
    const entry = join(fixture.path, "tools/turso/ui.ts");
    const grant: LocalConsumerGrant = {
      ...fixture.identity,
      purpose: "turso-ui",
      allowedModes: ["record-before", "freeze", "current-build", "server"],
      registrationHashes: Object.fromEntries(
        tursoUiModules.map((name) => [name, sha(join(fixture.path, name))]),
      ),
      stageCommands: {
        "record-before": [process.execPath, "--no-env-file", entry, "--record-before"],
        run: [process.execPath, "--no-env-file", entry, run],
      },
      heavy,
      build: {
        manifest: reference("current-build.json"),
        physical: reference("physical-before.private.json"),
      },
      localLibsql: {
        binary: heavy,
        directory: join(fixture.output, "runtime"),
        label: "fvoci-admission-fixture",
        host: "127.0.0.1",
        port: 0,
        tls: "generated-private-san-ip-loopback",
        childOnlyTrust: true,
      },
      canonicalRuntime: {
        imageId: "sha256:" + "a".repeat(64),
        networkAuthorized: true,
        shellProof: reference("shell-proof.json"),
      },
      inputSha256: "",
    };
    change(grant, fixture.path);
    grant.inputSha256 = digest(
      JSON.stringify({
        purpose: grant.purpose,
        source: grant.source,
        tree: grant.tree,
        registrationHashes: grant.registrationHashes,
        stageCommands: grant.stageCommands,
        heavy: grant.heavy,
        build: grant.build ?? null,
        localLibsql: grant.localLibsql ?? null,
        canonicalRuntime: grant.canonicalRuntime ?? null,
        keycloak: grant.keycloak ?? null,
      }),
    );
    return admit(
      fixture,
      grant,
      `localTursoUiRun(${JSON.stringify(mode)}, ${actor})`,
      "localTursoUiRun",
    );
  }
  const consumer = (...args: Parameters<typeof consumerResult>) => consumerResult(...args).code;
  test("an auth grant admits only the --local-auth entry", () => {
    expect(consumer("--local-auth", "--local-auth")).toBe(0);
    expect(consumer("--local-auth", "--local-restore")).toBe(23);
  });
  test("a restore grant admits only the --local-restore entry", () => {
    expect(consumer("--local-restore", "--local-restore")).toBe(0);
    expect(consumer("--local-restore", "--local-auth")).toBe(23);
  });
  test("any other runtime argv is unapproved", () => {
    for (const mode of ["--local-auth", "--local-restore"] as const) {
      expect(consumer("--local-other", mode)).toBe(23);
      expect(consumer(mode, mode, (grant) => grant.stageCommands.run?.push("--extra"))).toBe(23);
      expect(
        consumer(mode, mode, (grant, path) => {
          const run = grant.stageCommands.run;
          if (run) run[2] = join(path, "tools/turso/ui-flow.ts");
        }),
      ).toBe(23);
    }
  });
  test("an auth-session purpose never reaches the consumer entry", () => {
    expect(
      consumer("--local-auth", "--local-auth", (grant) =>
        Reflect.set(grant, "purpose", "auth-session"),
      ),
    ).toBe(23);
  });
  for (const name of consumerAuthModules)
    for (const fault of ["changed", "missing"] as const)
      test(`a ${fault} ${name} consumer hash is refused`, () => {
        const result = consumerResult("--local-auth", "--local-auth", (grant) => {
          if (fault === "missing") Reflect.deleteProperty(grant.registrationHashes, name);
          else grant.registrationHashes[name] = "0".repeat(64);
        });
        expect(result.code).toBe(23);
        // A recorded but changed module is refused by its own hash check.
        if (fault === "changed") expect(result.refusal).toBe("unregistered module " + name);
      });
});

test("the lane and consumer registries name every auth module", () => {
  for (const name of laneAuthModules) expect<readonly string[]>(registeredModules).toContain(name);
  for (const name of consumerAuthModules) expect<readonly string[]>(tursoUiModules).toContain(name);
});

describe("the manifest names exactly its local grant purpose", () => {
  const local = (purpose?: LocalGrant["purpose"]) => ({ purpose }) as LocalGrant;
  test("auth-session binds both backend lanes and never install", () => {
    for (const lane of ["postgres", "sqlite"] as const)
      expect(() => {
        lanePurpose(lane, "auth-session", local("auth-session"));
      }).not.toThrow();
    expect(() => {
      lanePurpose("install", undefined, local("auth-session"));
    }).not.toThrow();
    expect(() => {
      lanePurpose("install", "auth-session", local("auth-session"));
    }).toThrow();
  });
  test("a purpose mismatch between manifest and grant is refused", () => {
    for (const [manifest, grant] of [
      ["auth-session", local("equal-work")],
      ["equal-work", local("auth-session")],
      [undefined, local("auth-session")],
      ["auth-session", local()],
      ["auth-session", null],
      ["turso-ui", local("turso-ui")],
      ["keycloak", local("keycloak")],
    ] as const)
      expect(() => {
        lanePurpose("postgres", manifest, grant);
      }).toThrow();
  });
  test("the original and equal-work bindings stay admitted", () => {
    expect(() => {
      lanePurpose("postgres", undefined, null);
    }).not.toThrow();
    expect(() => {
      lanePurpose("sqlite", undefined, local());
    }).not.toThrow();
    expect(() => {
      lanePurpose("sqlite", "equal-work", local("equal-work"));
    }).not.toThrow();
  });
});

describe("auth-session lane retirement", () => {
  const source = "a".repeat(40),
    tree = "b".repeat(40),
    owner = "orca:fixture";
  function retirement(
    lane: "postgres" | "sqlite",
    flow: "on" | "off",
    change: (receipt: Record<string, unknown>) => void = () => undefined,
    purpose: "auth-session" | "equal-work" | "none" = "auth-session",
  ): boolean {
    const path = join(mkdtempSync(join(scratch, "run-")), "run");
    mkdirSync(path, { mode: 0o700 });
    const receipt: Record<string, unknown> = {
      source,
      tree,
      root_owner: owner,
      final_exit_code: 0,
      owned_container_absent: true,
      selected_flow: flow,
      cleanup_errors: [],
      owned_loopback_port_closed: true,
      recorded_process_identities_retired: true,
      purpose: "auth-session",
      auth_session: { cases: 3, events: "PASS", verdict_sha256: "c".repeat(64) },
      actual_browser_tests: 3,
      retries: 0,
    };
    change(receipt);
    write(join(path, "receipt.json"), receipt);
    if (lane === "postgres")
      write(join(path, "parent-receipt.json"), {
        source,
        tree,
        root_owner: owner,
        selected_flow: flow,
        all_owned_fixtures_closed: true,
      });
    const selected = purpose === "none" ? undefined : purpose;
    return laneRetirement(path, lane, flow, source, tree, owner, 0, selected).qualified;
  }
  for (const lane of ["postgres", "sqlite"] as const)
    for (const flow of ["on", "off"] as const) {
      test(`${lane}/${flow} qualifies exactly the three auth cases and their events`, () => {
        expect(retirement(lane, flow)).toBe(true);
      });
      test(`${lane}/${flow} refuses a missing or weaker auth proof`, () => {
        const faults: ((receipt: Record<string, unknown>) => void)[] = [
          (r) => Reflect.deleteProperty(r, "purpose"),
          (r) => (r.purpose = "equal-work"),
          (r) => Reflect.deleteProperty(r, "auth_session"),
          (r) => (r.auth_session = null),
          (r) => (r.auth_session = { cases: 2, events: "PASS" }),
          (r) => (r.auth_session = { cases: 3, events: "FAIL" }),
          (r) => (r.auth_session = { cases: 3 }),
          (r) => (r.actual_browser_tests = 1),
          (r) => (r.actual_browser_tests = 8),
          (r) => (r.retries = 1),
          (r) => (r.current_schema_server_restart = { restartBrowserExit: 0 }),
        ];
        for (const fault of faults) expect(retirement(lane, flow, fault)).toBe(false);
      });
      test(`${lane}/${flow} auth receipt never qualifies another lane purpose`, () => {
        expect(retirement(lane, flow, () => undefined, "equal-work")).toBe(false);
        expect(retirement(lane, flow, () => undefined, "none")).toBe(false);
      });
    }
});
