import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  AdmissionError,
  DIAGNOSTIC_UNIT_NAME,
  INVENTORY_TEST_NAME,
  MIGRATION_TEST_NAME,
  RESET_TEST_NAME,
  REVIEWED_REF,
  TEST_NAME,
  type Env,
  type Inputs,
} from "./guard-policy.ts";
import { runChild, type ChildResult } from "./guard-io.ts";
import {
  freezeCompiledTest,
  main,
  runDiagnosticUnit,
  runPrimary,
  runUi,
  type Seams,
} from "./guard.ts";
import { inventoryFailure, inventorySuccess, resetSuccess } from "./guard-fixtures.ts";

const REPO = resolve(import.meta.dir, "../..");
const GUARD = join(import.meta.dir, "guard.ts");
const A = "a".repeat(40);
const URL = "libsql://isolated-owner.aws-us-east-1.turso.io";
const roots: string[] = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});
const sha256 = (data: string | Buffer) => createHash("sha256").update(data).digest("hex");

function tempRoot(): string {
  const root = mkdtempSync(join(tmpdir(), "fvoci-turso-guard-"));
  roots.push(root);
  return root;
}

interface Call {
  argv: string[];
  env: Record<string, string>;
}

/** Seams whose child runs are scripted; each call may also mutate files. */
function seams(
  results: (ChildResult | ((call: number) => ChildResult))[],
  digest = { value: "fixture" },
) {
  const calls: Call[] = [];
  const value: Seams = {
    sourceDigest: () => digest.value,
    runChild: (argv, env) => {
      calls.push({ argv, env });
      const next = results[calls.length - 1];
      if (next === undefined) return Promise.reject(new Error("unexpected child"));
      return Promise.resolve(typeof next === "function" ? next(calls.length) : next);
    },
  };
  return { seams: value, calls, digest };
}

/** File-only freeze receipt; no native producer or ELF/SDK is executed. */
function frozen(): { root: string; manifest: Record<string, string>; write: () => void } {
  const root = tempRoot();
  writeFileSync(join(root, "turso-connection-libtest"), "\x7fELFpure fixture, never executed");
  mkdirSync(join(root, "fvoci-sqlite"));
  writeFileSync(
    join(root, "fvoci-sqlite/consumer-inputs.json"),
    "pure native-input binding, not a native producer",
  );
  writeFileSync(join(root, "turso-compile.json"), "pure retained compile-output binding");
  const manifest = {
    sha: A,
    source_digest: "fixture",
    binary_sha256: sha256(readFileSync(join(root, "turso-connection-libtest"))),
    native_input_sha256: sha256(readFileSync(join(root, "fvoci-sqlite/consumer-inputs.json"))),
    cargo_output_sha256: sha256(readFileSync(join(root, "turso-compile.json"))),
  };
  const write = () => {
    writeFileSync(join(root, "turso-connection-build.json"), JSON.stringify(manifest));
  };
  write();
  return { root, manifest, write };
}

function primaryEnv(root: string, extra: Env = {}): Env {
  return {
    PATH: "/usr/bin:/bin",
    RUNNER_TEMP: root,
    FVOCI_DATABASE_BACKEND: "libsql-remote",
    FVOCI_TEST_TURSO_DATABASE_URL: URL,
    FVOCI_TEST_TURSO_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN",
    FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false",
    LD_LIBRARY_PATH: "/fixture/lib",
    SSL_CERT_FILE: "/fixture/cert",
    SSL_CERT_DIR: "/fixture/certs",
    TZ: "UTC",
    FVOCI_TEST_TURSO_CONNECTION_SELECTED: "1",
    FVOCI_DATABASE_APP_URL: "FAKE_PRIVATE_TOKEN",
    UNRELATED_FAKE_CREDENTIAL: "FAKE_PRIVATE_TOKEN",
    ...extra,
  };
}

async function refusal(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    expect(error).toBeInstanceOf(AdmissionError);
    return (error as Error).message;
  }
  throw new Error("expected a refusal");
}

const ok = (output: string, status = 0): ChildResult => ({ status, output });
const connection =
  "test " +
  TEST_NAME +
  " ... FVOCI_TURSO_RECEIPT primary=OK rollback=OK close=OK leases=ZERO\nok\n" +
  "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n";
const migration =
  "test " +
  MIGRATION_TEST_NAME +
  " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK fk_proof=EXTENDED current=OK restart=OK close=OK leases=ZERO\nok\n" +
  "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n";
const BASE_KEYS = ["PATH", "LD_LIBRARY_PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "TZ"];

describe("freeze", () => {
  test("cargo emission controls are file-only, not compilation", () => {
    for (const mutation of [
      "valid",
      "wrong_feature",
      "wrong_source",
      "duplicate",
      "failed_build",
      "foreign_path",
      "symlink_in_deps",
      "not_elf",
    ]) {
      const root = tempRoot();
      const binary = join(root, "turso-target/debug/deps/fixture-libtest");
      mkdirSync(join(root, "turso-target/debug/deps"), { recursive: true });
      writeFileSync(binary, "\x7fELFfile-only fixture, never executed");
      if (mutation === "symlink_in_deps") {
        // Inside deps after resolution; only the final-component check refuses it.
        renameSync(binary, binary + ".real");
        symlinkSync(binary + ".real", binary);
      }
      if (mutation === "not_elf") writeFileSync(binary, "not an ELF test binary");
      mkdirSync(join(root, "fvoci-sqlite"));
      writeFileSync(
        join(root, "fvoci-sqlite/consumer-inputs.json"),
        "pure metadata fixture, not native proof",
      );
      const artifact: Record<string, unknown> = {
        reason: "compiler-artifact",
        target: {
          kind: ["lib"],
          name: "fvoci_server",
          src_path: join(process.cwd(), "src", "lib.rs"),
        },
        profile: { test: true },
        features: mutation === "wrong_feature" ? ["db-tests", "api-schema"] : ["db-tests"],
        executable: mutation === "foreign_path" ? "/foreign/test" : binary,
      };
      if (mutation === "wrong_source")
        (artifact.target as Record<string, unknown>).src_path = "/foreign/src/lib.rs";
      const emissions = [
        ...Array<unknown>(mutation === "duplicate" ? 2 : 1).fill(artifact),
        { reason: "build-finished", success: mutation !== "failed_build" },
      ];
      writeFileSync(
        join(root, "turso-compile.json"),
        emissions.map((value) => JSON.stringify(value)).join("\n"),
      );
      const freeze = () => {
        freezeCompiledTest(A, { RUNNER_TEMP: root }, { sourceDigest: () => "pure source fixture" });
      };
      if (mutation === "valid") {
        freeze();
        const manifest = JSON.parse(
          readFileSync(join(root, "turso-connection-build.json"), "utf8"),
        ) as Record<string, unknown>;
        expect(manifest.sha).toBe(A);
        expect(manifest.source_digest).toBe("pure source fixture");
        expect(readFileSync(join(root, "turso-connection-libtest"))).toEqual(readFileSync(binary));
      } else {
        expect(freeze).toThrow(new AdmissionError("COMPILED_TEST_BINDING_FAILED"));
        expect(readdirSync(root)).not.toContain("turso-connection-build.json");
      }
    }
  });
});

const listing = DIAGNOSTIC_UNIT_NAME + ": test\n\n1 test, 0 benchmarks\n";
const success =
  "running 1 test\ntest " +
  DIAGNOSTIC_UNIT_NAME +
  " ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n";

describe("diagnostic unit", () => {
  // Ambient DB/provider/GitHub values must never reach the unit.
  const unitEnv = (root: string): Env => ({
    PATH: "/usr/bin:/bin",
    RUNNER_TEMP: root,
    FVOCI_LIBSQL_URL: "FAKE_PRIVATE_TOKEN",
    FVOCI_LIBSQL_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN",
    GITHUB_TOKEN: "FAKE_PRIVATE_TOKEN",
    FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
  });

  test("the exact current unit runs without credentials before the secret step", async () => {
    const { root } = frozen();
    const run = seams([ok(listing), ok(success)]);
    const verdict = await runDiagnosticUnit(A, unitEnv(root), run.seams);
    expect(verdict).toEqual({
      lines: ["TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN"],
      code: null,
    });
    expect(run.calls.map((call) => call.argv)).toEqual([
      [join(root, "turso-connection-libtest"), DIAGNOSTIC_UNIT_NAME, "--list", "--exact"],
      [join(root, "turso-connection-libtest"), DIAGNOSTIC_UNIT_NAME, "--exact", "--test-threads=1"],
    ]);
    for (const call of run.calls) expect(call.env).toEqual({ PATH: "/usr/bin:/bin" });
  });

  test("no match, duplicate, wrong unit, benchmark or list failure refuses", async () => {
    for (const [given, status] of [
      ["0 tests, 0 benchmarks\n", 0],
      [listing + listing, 0],
      [listing.replace(DIAGNOSTIC_UNIT_NAME, TEST_NAME), 0],
      [listing.replace(": test", ": benchmark"), 0],
      [listing + "other::test: test\n", 0],
      [listing, 1],
    ] as const) {
      const { root } = frozen();
      const run = seams([ok(given + "FAKE_PRIVATE_TOKEN\n", status)]);
      expect(await refusal(runDiagnosticUnit(A, unitEnv(root), run.seams))).toBe(
        "TURSO_DIAGNOSTIC_UNIT_SELECTION_FAILED",
      );
      expect(run.calls.length).toBe(1);
    }
  });

  test("wrong source, binary, native or cargo binding refuses before the run", async () => {
    for (const field of [
      "sha",
      "source_digest",
      "binary_sha256",
      "native_input_sha256",
      "cargo_output_sha256",
    ]) {
      const fixture = frozen();
      fixture.manifest[field] = "wrong";
      fixture.write();
      const run = seams([]);
      expect(await refusal(runDiagnosticUnit(A, unitEnv(fixture.root), run.seams))).toBe(
        "COMPILED_TEST_BINDING_FAILED",
      );
      expect(run.calls).toEqual([]);
    }
    const notElf = frozen();
    writeFileSync(join(notElf.root, "turso-connection-libtest"), "not ELF");
    notElf.manifest.binary_sha256 = sha256("not ELF");
    notElf.write();
    expect(await refusal(runDiagnosticUnit(A, unitEnv(notElf.root), seams([]).seams))).toBe(
      "COMPILED_TEST_BINDING_FAILED",
    );
    const linked = frozen();
    renameSync(
      join(linked.root, "turso-connection-libtest"),
      join(linked.root, "fixture-original"),
    );
    symlinkSync(
      join(linked.root, "fixture-original"),
      join(linked.root, "turso-connection-libtest"),
    );
    const run = seams([]);
    expect(await refusal(runDiagnosticUnit(A, unitEnv(linked.root), run.seams))).toBe(
      "COMPILED_TEST_BINDING_FAILED",
    );
    expect(run.calls).toEqual([]);
  });

  test("source and binary changes during children cannot make PASS", async () => {
    for (const stage of [1, 2]) {
      const { root } = frozen();
      const mutate = (call: number) => {
        if (call === stage)
          writeFileSync(join(root, "turso-connection-libtest"), "\x7fELFchanged after start");
        return ok(call === 1 ? listing : success);
      };
      const run = seams([mutate, mutate]);
      expect(await refusal(runDiagnosticUnit(A, unitEnv(root), run.seams))).toBe(
        "COMPILED_TEST_BINDING_FAILED",
      );
      expect(run.calls.length).toBe(stage);
    }
    const { root } = frozen();
    const digest = { value: "fixture" };
    const run = seams(
      [
        () => {
          digest.value = "changed";
          return ok(listing);
        },
      ],
      digest,
    );
    expect(await refusal(runDiagnosticUnit(A, unitEnv(root), run.seams))).toBe(
      "COMPILED_TEST_BINDING_FAILED",
    );
    expect(run.calls.length).toBe(1);
  });

  test("exactly one real result and status are required, without raw echo", async () => {
    for (const [output, status] of [
      [success, 1],
      [success.replace("1 passed", "0 passed"), 0],
      [success.replace("0 failed", "1 failed"), 0],
      [success.replace("0 ignored", "1 ignored"), 0],
      [success.replace(DIAGNOSTIC_UNIT_NAME, TEST_NAME), 0],
      [success.replace(" ... ok", " ... ignored"), 0],
      [success + success, 0],
      ["SDK FAKE_PRIVATE_TOKEN\n", 0],
    ] as const) {
      const { root } = frozen();
      const run = seams([ok(listing), ok(output + "FAKE_PRIVATE_TOKEN\n", status)]);
      expect(await runDiagnosticUnit(A, unitEnv(root), run.seams)).toEqual({
        lines: [],
        code: "TURSO_DIAGNOSTIC_UNIT_FAILED",
      });
      expect(run.calls.length).toBe(2);
    }
    const { root } = frozen();
    const run = seams([ok(listing + "FAKE_PRIVATE_TOKEN\n"), ok(success + "FAKE_PRIVATE_TOKEN\n")]);
    expect((await runDiagnosticUnit(A, unitEnv(root), run.seams)).lines).toEqual([
      "TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN",
    ]);
  });
});

describe("primary consumers", () => {
  const connectionInputs: Inputs = { phase: "connection", destructive: false };
  const inventoryInputs: Inputs = { phase: "inventory", destructive: false };
  const resetInputs: Inputs = { phase: "reset", destructive: true };

  test("connection: exact command, restricted env and zero-test denial", async () => {
    const { root } = frozen();
    const run = seams([ok(connection)]);
    const verdict = await runPrimary(
      "connection",
      A,
      connectionInputs,
      primaryEnv(root),
      run.seams,
    );
    expect(verdict.lines).toContain("TURSO_CONNECTION_PASS tests=1 ignored=0");
    expect(run.calls[0]?.argv.slice(1)).toEqual([
      TEST_NAME,
      "--ignored",
      "--exact",
      "--test-threads=1",
      "--nocapture",
    ]);
    const child = run.calls[0]?.env ?? {};
    expect(child).not.toHaveProperty("UNRELATED_FAKE_CREDENTIAL");
    expect(child.FVOCI_DATABASE_BACKEND).toBe("libsql-remote");
    expect(child.FVOCI_LIBSQL_AUTH_TOKEN).toBe("FAKE_PRIVATE_TOKEN");
    expect(child).not.toHaveProperty("FVOCI_TEST_TURSO_AUTH_TOKEN");
    for (const [raw, status] of [
      [connection.replace("1 passed", "0 passed"), 0],
      [connection.replace("0 ignored", "1 ignored"), 0],
      [connection, 1],
      [connection.replace("rollback=OK", "rollback=FAILED"), 0],
    ] as const) {
      const failed = await runPrimary(
        "connection",
        A,
        connectionInputs,
        primaryEnv(root),
        seams([ok(raw + "FAKE_PRIVATE_BODY_NEVER_PRINT", status)]).seams,
      );
      expect(failed.code).toBe("TURSO_CONNECTION_FAILED");
      expect(failed.lines.join("\n")).not.toContain("FAKE_PRIVATE_BODY");
    }
    const none = seams([]);
    expect(
      await refusal(
        runPrimary("connection", "b".repeat(40), connectionInputs, primaryEnv(root), none.seams),
      ),
    ).toBe("COMPILED_TEST_BINDING_FAILED");
    expect(none.calls).toEqual([]);
  });

  test("the published workflow env drives each phase with the right credential names", async () => {
    for (const [phase, destructive, allow, output] of [
      ["connection", false, "false", connection],
      ["migration", true, "true", migration],
      ["inventory", false, "false", inventorySuccess()],
      ["reset", true, "true", resetSuccess()],
    ] as const) {
      const { root } = frozen();
      const run = seams([ok(output)]);
      const env = primaryEnv(root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
      const verdict = await runPrimary(phase, A, { phase, destructive }, env, run.seams);
      expect(verdict.code).toBeNull();
      const child = run.calls[0]?.env ?? {};
      const [present, absent] =
        phase === "reset"
          ? [
              ["FVOCI_TEST_TURSO_DATABASE_URL", "FVOCI_TEST_TURSO_AUTH_TOKEN"],
              ["FVOCI_LIBSQL_URL", "FVOCI_LIBSQL_AUTH_TOKEN"],
            ]
          : [
              ["FVOCI_LIBSQL_URL", "FVOCI_LIBSQL_AUTH_TOKEN"],
              ["FVOCI_TEST_TURSO_DATABASE_URL", "FVOCI_TEST_TURSO_AUTH_TOKEN"],
            ];
      expect(child[present[0] as string]).toBe(URL);
      expect(child[present[1] as string]).toBe("FAKE_PRIVATE_TOKEN");
      for (const key of absent) expect(child).not.toHaveProperty(key);
    }
  });

  test("migration exports exactly four flags only after confirmation", async () => {
    const fixture = frozen();
    const inputs = { phase: "migration", destructive: true };
    const env = primaryEnv(fixture.root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" });
    const run = seams([ok(migration)]);
    await runPrimary("migration", A, inputs, env, run.seams);
    expect(run.calls[0]?.argv.slice(1)).toEqual([
      MIGRATION_TEST_NAME,
      "--ignored",
      "--exact",
      "--test-threads=1",
      "--nocapture",
    ]);
    const child = run.calls[0]?.env ?? {};
    expect(child.FVOCI_TEST_TURSO_MIGRATION_SELECTED).toBe("1");
    expect(child.FVOCI_TEST_TURSO_PHASE).toBe("migration");
    expect(child.FVOCI_TEST_TURSO_DESTRUCTIVE).toBe("true");
    expect(child.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE).toBe("true");
    expect(child).not.toHaveProperty("FVOCI_TEST_TURSO_CONNECTION_SELECTED");
    expect(child).not.toHaveProperty("UNRELATED_FAKE_CREDENTIAL");
    const none = seams([]);
    expect(
      await refusal(
        runPrimary(
          "migration",
          A,
          inputs,
          { ...env, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false" },
          none.seams,
        ),
      ),
    ).toBe("DESTRUCTIVE_NOT_ALLOWED");
    writeFileSync(join(fixture.root, "fvoci-sqlite/consumer-inputs.json"), "changed input");
    expect(await refusal(runPrimary("migration", A, inputs, env, none.seams))).toBe(
      "COMPILED_TEST_BINDING_FAILED",
    );
    expect(none.calls).toEqual([]);
    expect(await refusal(runPrimary("connection", A, inputs, env, none.seams))).toBe(
      "WRONG_CONSUMER_PHASE",
    );
    expect(await refusal(runPrimary("migration", A, connectionInputs, env, none.seams))).toBe(
      "WRONG_CONSUMER_PHASE",
    );
  });

  test("a single exact inventory child has only the maintained environment", async () => {
    const { root } = frozen();
    const env = primaryEnv(root, {
      FVOCI_TEST_TURSO_MIGRATION_SELECTED: "0",
      FVOCI_TEST_TURSO_PHASE: "migration",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
    });
    const before = { ...env };
    const run = seams([ok(inventorySuccess())]);
    const verdict = await runPrimary("inventory", A, inventoryInputs, env, run.seams);
    expect(verdict.lines).toContain("TURSO_INVENTORY_PASS tests=1 ignored=0");
    expect(run.calls.length).toBe(1);
    expect(run.calls[0]?.argv).toEqual([
      join(root, "turso-connection-libtest"),
      INVENTORY_TEST_NAME,
      "--ignored",
      "--exact",
      "--test-threads=1",
      "--nocapture",
    ]);
    // Ambient migration authority is never inherited; the parent env is untouched.
    expect(run.calls[0]?.env).toEqual({
      ...Object.fromEntries(BASE_KEYS.map((key) => [key, env[key] as string])),
      FVOCI_DATABASE_BACKEND: "libsql-remote",
      FVOCI_LIBSQL_URL: URL,
      FVOCI_LIBSQL_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN",
      FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
      FVOCI_TEST_TURSO_PHASE: "inventory",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "false",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false",
    });
    expect(env).toEqual(before);
    expect(verdict.lines.join("\n")).not.toContain("FAKE_PRIVATE_TOKEN");
  });

  test("inventory: false flags, backend and wrong phase refuse before the child", async () => {
    const { root } = frozen();
    const none = seams([]);
    for (const allow of ["true", "FALSE", "", "FAKE_PRIVATE_TOKEN", undefined]) {
      const env = primaryEnv(root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
      expect(await refusal(runPrimary("inventory", A, inventoryInputs, env, none.seams))).toBe(
        "INVENTORY_MUST_BE_READ_ONLY",
      );
      expect(
        await refusal(
          runPrimary("inventory", A, { ...inventoryInputs, destructive: true }, env, none.seams),
        ),
      ).toBe("INVENTORY_MUST_BE_READ_ONLY");
    }
    expect(
      await refusal(
        runPrimary(
          "inventory",
          A,
          { ...inventoryInputs, destructive: true },
          primaryEnv(root),
          none.seams,
        ),
      ),
    ).toBe("INVENTORY_MUST_BE_READ_ONLY");
    for (const value of ["true", 1, null]) {
      expect(
        await refusal(
          runPrimary(
            "inventory",
            A,
            { ...inventoryInputs, destructive: value },
            primaryEnv(root),
            none.seams,
          ),
        ),
      ).toBe("INVALID_BOOLEAN");
    }
    expect(
      await refusal(
        runPrimary(
          "inventory",
          A,
          inventoryInputs,
          primaryEnv(root, { FVOCI_DATABASE_BACKEND: "sqlite" }),
          none.seams,
        ),
      ),
    ).toBe("BACKEND_SELECTOR_REQUIRED");
    for (const phase of ["migration", "connection", "unknown"]) {
      expect(
        await refusal(
          runPrimary("inventory", A, { ...inventoryInputs, phase }, primaryEnv(root), none.seams),
        ),
      ).toBe("WRONG_CONSUMER_PHASE");
    }
    expect(none.calls).toEqual([]);
  });

  test("inventory: a mixed or missing allow gate refuses before binding or child", async () => {
    for (const destructive of [false, true]) {
      for (const allow of [undefined, "false", "true", "FALSE", "", "0", "False"]) {
        if (!destructive && allow === "false") continue; // the positive exact tuple runs elsewhere
        // No freeze receipt exists: reaching the binding would be a generic failure.
        const root = tempRoot();
        const none = seams([]);
        const env = primaryEnv(root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
        expect(
          await refusal(
            runPrimary("inventory", A, { ...inventoryInputs, destructive }, env, none.seams),
          ),
        ).toBe("INVENTORY_MUST_BE_READ_ONLY");
        expect(none.calls).toEqual([]);
      }
    }
  });

  for (const [phase, inputs, allow, output] of [
    ["inventory", inventoryInputs, "false", inventorySuccess()],
    ["reset", resetInputs, "true", resetSuccess()],
  ] as const) {
    test(phase + ": the complete frozen binding and ELF controls precede the child", async () => {
      for (const key of [
        "sha",
        "source_digest",
        "binary_sha256",
        "native_input_sha256",
        "cargo_output_sha256",
      ]) {
        const fixture = frozen();
        fixture.manifest[key] = "FAKE_PRIVATE_TOKEN";
        fixture.write();
        const none = seams([]);
        const env = primaryEnv(fixture.root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
        expect(await refusal(runPrimary(phase, A, inputs, env, none.seams))).toBe(
          "COMPILED_TEST_BINDING_FAILED",
        );
        expect(none.calls).toEqual([]);
      }
      for (const mutation of ["elf", "symlink"]) {
        const fixture = frozen();
        const binary = join(fixture.root, "turso-connection-libtest");
        if (mutation === "elf") {
          writeFileSync(binary, "not ELF FAKE_PRIVATE_TOKEN");
          fixture.manifest.binary_sha256 = sha256("not ELF FAKE_PRIVATE_TOKEN");
          fixture.write();
        } else {
          renameSync(binary, join(fixture.root, "fixture-original"));
          symlinkSync(join(fixture.root, "fixture-original"), binary);
        }
        const none = seams([]);
        const env = primaryEnv(fixture.root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
        expect(await refusal(runPrimary(phase, A, inputs, env, none.seams))).toBe(
          "COMPILED_TEST_BINDING_FAILED",
        );
        expect(none.calls).toEqual([]);
      }
    });

    test(phase + ": a binding change during the child never emits PASS", async () => {
      for (const mutation of ["source", "binary", "native", "cargo", "manifest"]) {
        const { root } = frozen();
        const digest = { value: "fixture" };
        const paths: Record<string, string> = {
          binary: "turso-connection-libtest",
          native: "fvoci-sqlite/consumer-inputs.json",
          cargo: "turso-compile.json",
          manifest: "turso-connection-build.json",
        };
        const run = seams(
          [
            () => {
              // A trailing space keeps the manifest JSON identical but changes the receipt.
              if (mutation === "source") digest.value = "changed";
              else appendFileSync(join(root, paths[mutation] as string), " ");
              return ok(output);
            },
          ],
          digest,
        );
        const env = primaryEnv(root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
        expect(await refusal(runPrimary(phase, A, inputs, env, run.seams))).toBe(
          "COMPILED_TEST_BINDING_FAILED",
        );
        expect(run.calls.length).toBe(1);
      }
    });

    test(phase + ": a failed child over a changed binding discloses nothing", async () => {
      // Output of a child whose binary no longer matches the receipt is never parsed.
      const failed = phase === "inventory" ? inventoryFailure() : output;
      for (const mutation of ["binary", "native", "cargo", "manifest", "source"]) {
        const { root } = frozen();
        const digest = { value: "fixture" };
        const paths: Record<string, string> = {
          binary: "turso-connection-libtest",
          native: "fvoci-sqlite/consumer-inputs.json",
          cargo: "turso-compile.json",
        };
        const run = seams(
          [
            () => {
              if (mutation === "source") digest.value = "changed";
              else if (mutation === "manifest")
                appendFileSync(join(root, "turso-connection-build.json"), " ");
              else writeFileSync(join(root, paths[mutation] as string), "FAKE_PRIVATE_TOKEN");
              return ok(failed + "FAKE_PRIVATE_TOKEN\n", 1);
            },
          ],
          digest,
        );
        const env = primaryEnv(root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
        expect(await refusal(runPrimary(phase, A, inputs, env, run.seams))).toBe(
          "COMPILED_TEST_BINDING_FAILED",
        );
        expect(run.calls.length).toBe(1);
      }
    });
  }

  test("a failed inventory still refuses and discloses only closed facts", async () => {
    const { root } = frozen();
    const run = seams([
      ok(
        inventoryFailure(
          undefined,
          undefined,
          undefined,
          undefined,
          'Error: "FAKE_PRIVATE_TOKEN"\n',
        ),
        1,
      ),
    ]);
    const verdict = await runPrimary("inventory", A, inventoryInputs, primaryEnv(root), run.seams);
    expect(verdict.code).toBe("TURSO_INVENTORY_FAILED");
    expect(verdict.lines.join("\n")).toContain("primary=INVENTORY_SCHEMA_REFUSED");
    expect(verdict.lines.join("\n")).not.toContain("FAKE_PRIVATE_TOKEN");
    expect(run.calls.length).toBe(1);
    const status = seams([ok(inventorySuccess() + "FAKE_PRIVATE_TOKEN", 1)]);
    expect(
      (await runPrimary("inventory", A, inventoryInputs, primaryEnv(root), status.seams)).code,
    ).toBe("TURSO_INVENTORY_FAILED");
  });

  test("reset requires manual confirmation and the allow gate before the child", async () => {
    for (const [destructive, allow] of [
      [false, "false"],
      [false, "true"],
      [true, "false"],
      [true, "TRUE"],
      [true, ""],
    ] as const) {
      const { root } = frozen();
      const none = seams([]);
      const env = primaryEnv(root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow });
      expect(
        await refusal(runPrimary("reset", A, { ...resetInputs, destructive }, env, none.seams)),
      ).toBe("DESTRUCTIVE_NOT_ALLOWED");
      expect(none.calls).toEqual([]);
    }
    for (const phase of ["connection", "migration", "inventory", "unknown"]) {
      expect(
        await refusal(runPrimary("reset", A, { ...resetInputs, phase }, {}, seams([]).seams)),
      ).toBe("WRONG_CONSUMER_PHASE");
    }
  });

  test("reset child is a separate exact body with the current binding and no secret echo", async () => {
    const { root } = frozen();
    const env = primaryEnv(root, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" });
    const run = seams([ok(resetSuccess())]);
    const verdict = await runPrimary("reset", A, resetInputs, env, run.seams);
    expect(run.calls[0]?.argv).toEqual([
      join(root, "turso-connection-libtest"),
      RESET_TEST_NAME,
      "--ignored",
      "--exact",
      "--test-threads=1",
      "--nocapture",
    ]);
    expect(run.calls[0]?.env).toEqual({
      ...Object.fromEntries(BASE_KEYS.map((key) => [key, env[key] as string])),
      FVOCI_DATABASE_BACKEND: "libsql-remote",
      FVOCI_TEST_TURSO_DATABASE_URL: URL,
      FVOCI_TEST_TURSO_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN",
      FVOCI_TEST_TURSO_RESET_SELECTED: "1",
      FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1",
      FVOCI_TEST_TURSO_PHASE: "migration",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
    });
    expect(verdict.lines).toContain("TURSO_RESET_PASS tests=1 ignored=0");
    expect(verdict.lines.join("\n")).not.toContain("FAKE_PRIVATE_TOKEN");
  });
});

describe("UI consumer handoff", () => {
  const uiEnv: Env = {
    FVOCI_DATABASE_BACKEND: "libsql-remote",
    FVOCI_LIBSQL_URL: URL,
    FVOCI_LIBSQL_AUTH_TOKEN: "FAKE_FIXTURE_TOKEN",
    FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false",
  };

  test("remote baseline keeps the exact source binding", async () => {
    let loaded = 0;
    const consume = () => {
      loaded += 1;
      return Promise.resolve(() => Promise.resolve());
    };
    expect(
      await refusal(
        runUi("ui-baseline", A, { phase: "ui-baseline", destructive: false }, uiEnv, consume),
      ),
    ).toBe("UI_REVIEWED_SOURCE_REQUIRED");
    expect(loaded).toBe(0);
  });

  test("ack binds both observed digests; UiError codes pass through, other errors do not", async () => {
    const env = { ...uiEnv, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" };
    const base = { phase: "ui-ack", destructive: true, ui_source_sha: A };
    const never = () => Promise.reject(new Error("must not load"));
    expect(
      await refusal(runUi("ui-ack", A, { ...base, ui_target_sha256: "a".repeat(64) }, env, never)),
    ).toBe("UI_CURRENT_DATASET_BINDING_REQUIRED");
    expect(
      await refusal(
        runUi("ui-ack", A, { ...base, ui_baseline_sha256: "a".repeat(64) }, env, never),
      ),
    ).toBe("UI_CURRENT_TARGET_BINDING_REQUIRED");
    const bound = { ...base, ui_baseline_sha256: "a".repeat(64), ui_target_sha256: "b".repeat(64) };
    const seen: unknown[] = [];
    await runUi("ui-ack", A, bound, env, () =>
      Promise.resolve((phase: string, inputs: Inputs) => {
        seen.push(phase, inputs);
        return Promise.resolve();
      }),
    );
    expect(seen).toEqual(["ui-ack", bound]);
    class UiError extends Error {
      override name = "UiError";
    }
    const fails = (error: Error) => () => Promise.resolve(() => Promise.reject(error));
    expect(
      await refusal(runUi("ui-ack", A, bound, env, fails(new UiError("UI_ACTUAL_BROWSER_FAILED")))),
    ).toBe("UI_ACTUAL_BROWSER_FAILED");
    // A non-UiError stays an internal failure: main() reports ADMISSION_FAILED.
    let caught: unknown;
    try {
      await runUi("ui-ack", A, bound, env, fails(new Error("FAKE_PRIVATE_TOKEN")));
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(Error);
    expect(caught).not.toBeInstanceOf(AdmissionError);
  });
});

describe("child capture", () => {
  test("exact allowlisted env, write-order stdout+stderr, exit code and signal", async () => {
    const env = await runChild(["/usr/bin/env"], { PATH: "/usr/bin:/bin", A: "1" });
    expect(env.status).toBe(0);
    expect(env.output.split("\n").filter(Boolean).sort()).toEqual(["A=1", "PATH=/usr/bin:/bin"]);
    const merged = await runChild(
      ["/bin/sh", "-c", "printf a; printf b >&2; printf c; exit 3"],
      {},
    );
    expect(merged).toEqual({ status: 3, output: "abc" });
    const killed = await runChild(["/bin/sh", "-c", "kill -TERM $$"], {});
    expect(killed.status).toBe(-15);
    const bytes = await runChild(["/usr/bin/printf", "\\357\\273\\277x\\377"], {
      PATH: "/usr/bin",
    });
    expect(bytes.output).toBe("\ufeffx\ufffd");
  });
});

describe("CLI", () => {
  const cli = (args: string[], env: Record<string, string>) => {
    const result = Bun.spawnSync([process.execPath, "--no-env-file", GUARD, ...args], {
      cwd: REPO,
      env,
      stdout: "pipe",
      stderr: "pipe",
    });
    return [result.exitCode, result.stdout.toString(), result.stderr.toString()];
  };
  const head = () =>
    Bun.spawnSync(["git", "rev-parse", "HEAD"], { cwd: REPO }).stdout.toString().trim();

  test("reviewed-branch push admits source only and never consumes", () => {
    const root = tempRoot();
    writeFileSync(join(root, "event.json"), '{"inputs": {}}');
    const env = {
      PATH: process.env.PATH ?? "",
      GITHUB_EVENT_PATH: join(root, "event.json"),
      GITHUB_EVENT_NAME: "push",
      GITHUB_REPOSITORY: "AISFlow/fvoci",
      GITHUB_REF: REVIEWED_REF,
      GITHUB_SHA: head(),
    };
    expect(cli(["--admit"], env)).toEqual([
      0,
      "BOOTSTRAP_SOURCE_ADMISSION_OK_RUNTIME_NOT_RUN\n",
      "",
    ]);
    expect(cli(["--consume"], env)).toEqual([78, "", "SECRET_MODE_REQUIRES_MANUAL\n"]);
  });

  test("real CLI: missing secret and parse failure are fixed codes without input echo", () => {
    const root = tempRoot();
    const env = {
      PATH: process.env.PATH ?? "",
      GITHUB_EVENT_NAME: "workflow_dispatch",
      GITHUB_REPOSITORY: "AISFlow/fvoci",
      GITHUB_REF: "refs/heads/main",
      GITHUB_SHA: head(),
      FVOCI_DATABASE_BACKEND: "libsql-remote",
      GITHUB_EVENT_PATH: join(root, "event.json"),
    };
    writeFileSync(
      env.GITHUB_EVENT_PATH,
      JSON.stringify({ inputs: { phase: "connection", destructive: "false" } }),
    );
    expect(cli(["--consume"], env)).toEqual([78, "", "MISSING_SECRET\n"]);
    writeFileSync(env.GITHUB_EVENT_PATH, '{"FAKE_SECRET_SENTINEL_NEVER_REAL":');
    expect(cli(["--consume"], env)).toEqual([78, "", "ADMISSION_FAILED\n"]);
    expect(cli([], env)).toEqual([78, "", "EXPLICIT_MODE_REQUIRED\n"]);
  });

  test("main routes each mode to one effect and admission writes the Environment id", async () => {
    const root = tempRoot();
    writeFileSync(
      join(root, "event.json"),
      JSON.stringify({ inputs: { phase: "connection", destructive: "false" } }),
    );
    const env: Env = {
      GITHUB_EVENT_PATH: join(root, "event.json"),
      GITHUB_EVENT_NAME: "workflow_dispatch",
      GITHUB_REPOSITORY: "AISFlow/fvoci",
      GITHUB_REF: "refs/heads/main",
      GITHUB_SHA: A,
      GITHUB_OUTPUT: join(root, "output"),
    };
    const writes: string[] = [];
    const out = process.stdout.write.bind(process.stdout);
    const err = process.stderr.write.bind(process.stderr);
    process.stdout.write = (chunk: string) => writes.push("1:" + chunk) > 0;
    process.stderr.write = (chunk: string) => writes.push("2:" + chunk) > 0;
    try {
      const fetched: string[] = [];
      const effects = (body: string) => ({
        checkoutSha: () => A,
        fetchEnvironment: async () => {
          fetched.push(body);
          const { parseEnvironmentBody } = await import("./guard-io.ts");
          return parseEnvironmentBody(new TextEncoder().encode(body));
        },
        seams: seams([]).seams,
      });
      expect(await main(["--admit"], env, effects('{"name":"fvoci-turso-test","id":123}'))).toBe(0);
      expect(readFileSync(join(root, "output"), "utf8")).toBe("environment_id=123\n");
      expect(await main(["--admit"], env, effects('{"name":"fvoci-turso-test","id":0}'))).toBe(78);
      const unavailable = {
        ...effects(""),
        fetchEnvironment: () => Promise.reject(new Error("FAKE_PRIVATE_BODY_NEVER_PRINT")),
      };
      expect(await main(["--admit"], env, unavailable)).toBe(78);
      expect(
        await main(["--diagnostic-unit"], { ...env, GITHUB_SHA: "b".repeat(40) }, effects("")),
      ).toBe(78);
      expect(fetched.length).toBe(2);
    } finally {
      process.stdout.write = out;
      process.stderr.write = err;
    }
    expect(writes).toEqual([
      "1:ENVIRONMENT_ADMISSION_OK_RUNTIME_NOT_RUN\n",
      "2:ENVIRONMENT_POLICY_DENIED\n",
      "2:ENVIRONMENT_METADATA_UNAVAILABLE\n",
      "2:CHECKOUT_MISMATCH\n",
    ]);
  });
});

describe("main routing", () => {
  const eventFile = (root: string, inputs: Record<string, string>) => {
    const path = join(root, "event.json");
    writeFileSync(path, JSON.stringify({ inputs }));
    return path;
  };
  const github = (root: string, inputs: Record<string, string>, row: string[]): Env => {
    const [event, repository, ref, sha] = row as [string, string, string, string];
    return {
      GITHUB_EVENT_PATH: eventFile(root, inputs),
      GITHUB_EVENT_NAME: event,
      GITHUB_REPOSITORY: repository,
      GITHUB_REF: ref,
      GITHUB_SHA: sha,
    };
  };

  /** Runs main with every other effect refusing; returns exit, writes and stray calls. */
  async function route(argv: string[], env: Env, run: ReturnType<typeof seams>) {
    const stray: string[] = [];
    const writes: string[] = [];
    const out = process.stdout.write.bind(process.stdout);
    const err = process.stderr.write.bind(process.stderr);
    process.stdout.write = (chunk: string) => writes.push("1:" + chunk) > 0;
    process.stderr.write = (chunk: string) => writes.push("2:" + chunk) > 0;
    try {
      const code = await main(argv, env, {
        checkoutSha: () => A,
        fetchEnvironment: () => {
          stray.push("metadata");
          return Promise.reject(new Error("unexpected metadata read"));
        },
        uiConsume: () => {
          stray.push("ui");
          return Promise.reject(new Error("unexpected UI consumer"));
        },
        seams: run.seams,
      });
      return { code, writes, stray };
    } finally {
      process.stdout.write = out;
      process.stderr.write = err;
    }
  }

  test("--diagnostic-unit runs only the unit, and only for a trusted manual dispatch", async () => {
    for (const [event, sha, expected] of [
      ["workflow_dispatch", A, "1:TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN\n"],
      ["push", A, "2:SECRET_MODE_REQUIRES_MANUAL\n"],
      ["workflow_dispatch", "b".repeat(40), "2:CHECKOUT_MISMATCH\n"],
    ] as const) {
      const { root } = frozen();
      const env = {
        PATH: "/usr/bin:/bin",
        RUNNER_TEMP: root,
        FVOCI_LIBSQL_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN",
        ...github(root, { phase: "connection", destructive: "false" }, [
          event,
          "AISFlow/fvoci",
          REVIEWED_REF,
          sha,
        ]),
      };
      const run = seams([ok(listing), ok(success)]);
      const result = await route(["--diagnostic-unit"], env, run);
      expect(result.code).toBe(expected.startsWith("1:") ? 0 : 78);
      expect(result.writes).toEqual([expected]);
      expect(result.stray).toEqual([]);
      expect(run.calls.map((call) => call.argv[1])).toEqual(
        expected.startsWith("1:") ? [DIAGNOSTIC_UNIT_NAME, DIAGNOSTIC_UNIT_NAME] : [],
      );
    }
  });

  test("--consume routes inventory only from a trusted, read-only manual dispatch", async () => {
    const PASS = "1:TURSO_INVENTORY_PASS tests=1 ignored=0\n";
    for (const [row, destructive, expected] of [
      [["workflow_dispatch", "AISFlow/fvoci", "refs/heads/main", A], "false", PASS],
      [["workflow_dispatch", "AISFlow/fvoci", REVIEWED_REF, A], "false", PASS],
      [["push", "AISFlow/fvoci", REVIEWED_REF, A], "false", "SECRET_MODE_REQUIRES_MANUAL"],
      [["pull_request", "AISFlow/fvoci", "refs/heads/main", A], "false", "UNTRUSTED_DISPATCH"],
      [
        ["workflow_dispatch", "attacker/fvoci", "refs/heads/main", A],
        "false",
        "UNTRUSTED_DISPATCH",
      ],
      [
        ["workflow_dispatch", "AISFlow/fvoci", "refs/heads/topic", A],
        "false",
        "UNTRUSTED_DISPATCH",
      ],
      [
        ["workflow_dispatch", "AISFlow/fvoci", "refs/heads/main", "b".repeat(40)],
        "false",
        "CHECKOUT_MISMATCH",
      ],
      [
        ["workflow_dispatch", "AISFlow/fvoci", "refs/heads/main", A],
        "true",
        "INVENTORY_MUST_BE_READ_ONLY",
      ],
    ] as const) {
      const { root } = frozen();
      const env = {
        ...primaryEnv(root),
        ...github(root, { phase: "inventory", destructive }, [...row]),
      };
      const run = seams([ok(inventorySuccess())]);
      const result = await route(["--consume"], env, run);
      expect(result.stray).toEqual([]);
      expect(result.writes.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
      if (expected === PASS) {
        expect(result.code).toBe(0);
        expect(result.writes).toContain(PASS);
        expect(result.writes.filter((write) => write.startsWith("2:"))).toEqual([]);
        expect(run.calls.map((call) => call.argv[1])).toEqual([INVENTORY_TEST_NAME]);
      } else {
        expect(result.code).toBe(78);
        expect(result.writes).toEqual(["2:" + expected + "\n"]);
        expect(run.calls).toEqual([]);
      }
    }
  });
});

describe("reset plan source contract", () => {
  test("the literal drop plan matches the current schema in child-before-parent FK order", () => {
    const source = readFileSync(join(REPO, "src/db/turso_test.rs"), "utf8");
    const literal = (
      source.split("const RESET_DROP_STATEMENTS: [&str; 126] = [")[1] as string
    ).split("];")[0] as string;
    const statements = literal
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "")
      .map((line) => JSON.parse(line.endsWith(",") ? line.slice(0, -1) : line) as string);
    const tables = new Map<string, Set<string>>();
    const triggers = new Set<string>();
    // Source fixture for the maintained literal DDL grammar, never a production
    // parser or a runtime catalog validation substitute.
    const directory = join(REPO, "migrations/sqlite/060");
    for (const name of readdirSync(directory)
      .filter((file) => file.endsWith(".sql"))
      .sort()) {
      const sql = readFileSync(join(directory, name), "utf8");
      for (const match of sql.matchAll(/CREATE TABLE (\w+)\s*\(/g)) {
        const body = sql.slice(match.index, sql.indexOf(";", match.index + match[0].length));
        const parents = new Set(
          [...body.matchAll(/REFERENCES (\w+)/g)].map((reference) => reference[1] as string),
        );
        parents.delete(match[1] as string);
        tables.set(match[1] as string, parents);
      }
      for (const match of sql.matchAll(/CREATE TRIGGER (\w+)/g)) triggers.add(match[1] as string);
    }
    const strip = (statement: string, prefix: string) => statement.slice(prefix.length, -2);
    const dropTriggers = statements
      .slice(0, 27)
      .map((statement) => strip(statement, 'DROP TRIGGER IF EXISTS "'));
    const dropTables = statements
      .slice(27)
      .map((statement) => strip(statement, 'DROP TABLE IF EXISTS "'));
    expect(statements.length).toBe(126);
    expect(new Set(dropTriggers).size).toBe(27);
    expect(new Set(dropTriggers)).toEqual(triggers);
    expect(new Set(dropTables).size).toBe(99);
    expect(new Set(dropTables)).toEqual(new Set(tables.keys()));
    expect(dropTables.at(-1)).toBe("schema_migrations");
    for (const [child, parents] of tables) {
      for (const parent of parents)
        expect(dropTables.indexOf(child)).toBeLessThan(dropTables.indexOf(parent));
    }
    expect(statements.join("")).not.toContain("PRAGMA");
  });
});
