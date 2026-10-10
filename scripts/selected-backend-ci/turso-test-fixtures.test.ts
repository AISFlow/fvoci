import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  FIXED_LAUNCHER,
  UiError,
  admitPublishedConfig,
  assertPreserved,
  baselineFailureDiagnostic,
  baselinePassLine,
  capsuleText,
  containerArgv,
  serverStartDiagnostic,
  startupBlockers,
  startupRemoteCause,
} from "./turso-ui.ts";
import {
  AdmissionError,
  API_ROOT,
  DIAGNOSTIC_UNIT_NAME,
  ENVIRONMENT,
  INVENTORY_PRIMARY_CODES,
  INVENTORY_TEST_NAME,
  MIGRATION_CLOSE_CODES,
  MIGRATION_FK_PROOF_KINDS,
  MIGRATION_PRIMARY_CODES,
  MIGRATION_TEST_NAME,
  PHASES,
  RESET_TEST_NAME,
  REVIEWED_REF,
  TEST_NAME,
  UI_REVIEWED_REF,
  defaultIO,
  freezeCompiledTest,
  inventoryResult,
  main,
  migrationResult,
  pyDumpsSorted,
  requireImplemented,
  resetResult,
  runConnection,
  runDiagnosticUnit,
  runInventory,
  runMigration,
  runReset,
  runUi,
  validateDispatch,
  validateEnvironment,
  validateTarget,
  type GuardIO,
  type SpawnResult,
} from "./turso-test-guard.ts";

const HOST = "isolated-owner.aws-us-east-1.turso.io";
const SHA = "a".repeat(40);
const repo = join(import.meta.dir, "../..");
const context = { event_name: "workflow_dispatch", repository: "AISFlow/fvoci", ref: "refs/heads/main", sha: SHA };
const inputs = { phase: "connection", destructive: false };
const settings = { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false" };
const secrets = { FVOCI_TEST_TURSO_DATABASE_URL: "libsql://" + HOST, FVOCI_TEST_TURSO_AUTH_TOKEN: "FAKE_FIXTURE_TOKEN_NEVER_REAL" };

function harness(partial: Partial<GuardIO> = {}) {
  const out: string[] = [];
  const err: string[] = [];
  const io = defaultIO({
    env: {},
    sourceDigest: () => "fixture",
    cwd: process.cwd(),
    ...partial,
    print: partial.print ?? ((line) => out.push(line + "\n")),
    eprint: partial.eprint ?? ((line) => err.push(line + "\n")),
  });
  return { io, out, err };
}

async function denied(code: string, fn: () => unknown) {
  try {
    await fn();
  } catch (error) {
    expect(error).toBeInstanceOf(AdmissionError);
    expect((error as Error).message).toBe(code);
    return;
  }
  throw new Error("expected " + code);
}

function encode(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

function spawnOf(stdout: string, status = 0): GuardIO["spawn"] {
  return async () => ({ status, stdout: encode(stdout) });
}

test("python-compatible sorted digest encoding", () => {
  expect(pyDumpsSorted({ b: "1", a: "2" })).toBe('{"a": "2", "b": "1"}');
  expect(pyDumpsSorted({ 한글: "x", b: "1" })).toBe('{"b": "1", "\\ud55c\\uae00": "x"}');
});

test("dispatch trust, checkout, phase and destructive gates", async () => {
  expect(validateDispatch(context, inputs, SHA)).toBe("connection");
  expect(validateDispatch({ ...context, event_name: "push", ref: REVIEWED_REF }, {}, SHA)).toBe("connection");
  expect(validateDispatch({ ...context, ref: REVIEWED_REF }, inputs, SHA)).toBe("connection");
  for (const [event, ref] of [["push", "refs/heads/main"], ["push", "refs/heads/topic"], ["workflow_dispatch", "refs/heads/topic"]] as const) {
    await denied("UNTRUSTED_DISPATCH", () => validateDispatch({ ...context, event_name: event, ref }, inputs, SHA));
  }
  const ui = { ...context, ref: UI_REVIEWED_REF };
  expect(validateDispatch(ui, { phase: "ui-baseline", destructive: false }, SHA)).toBe("ui-baseline");
  expect(validateDispatch(ui, { phase: "ui-ack", destructive: true }, SHA)).toBe("ui-ack");
  await denied("UNTRUSTED_DISPATCH", () => validateDispatch({ ...ui, event_name: "push" }, inputs, SHA));
  await denied("UNTRUSTED_DISPATCH", () => validateDispatch({ ...ui, event_name: "pull_request" }, inputs, SHA));
  await denied("UNTRUSTED_DISPATCH", () => validateDispatch({ ...ui, repository: "attacker/fvoci" }, { phase: "ui-baseline", destructive: false }, SHA));
  await denied("CHECKOUT_MISMATCH", () => validateDispatch(ui, { phase: "ui-baseline", destructive: false }, "b".repeat(40)));
  for (const phase of PHASES) {
    if (phase === "ui-baseline" || phase === "ui-ack") continue;
    const destructive = phase !== "connection" && phase !== "inventory";
    await denied("UI_REF_PHASE_REQUIRED", () => validateDispatch(ui, { phase, destructive }, SHA));
  }
  for (const [key, value] of [["event_name", "pull_request"], ["event_name", "pull_request_target"], ["repository", "attacker/fvoci"], ["ref", "refs/heads/topic"], ["ref", "refs/tags/main"]] as const) {
    await denied("UNTRUSTED_DISPATCH", () => validateDispatch({ ...context, [key]: value }, inputs, SHA));
  }
  for (const [sha, checkout] of [["", ""], [SHA, "b".repeat(40)], ["main", "main"]] as const) {
    await denied("CHECKOUT_MISMATCH", () => validateDispatch({ ...context, sha }, inputs, checkout));
  }
  for (const value of ["false", "true", 0, 1, null]) {
    await denied("INVALID_BOOLEAN", () => validateDispatch(context, { ...inputs, destructive: value }, SHA));
  }
  await denied("UNKNOWN_PHASE", () => validateDispatch(context, { ...inputs, phase: "echo FAKE" }, SHA));
  await denied("UNKNOWN_PHASE", () => requireImplemented("echo FAKE"));
  for (const phase of ["crud", "transactions", "persistence", "restore"]) await denied("NOT_IMPLEMENTED", () => requireImplemented(phase));
  expect(requireImplemented("connection")).toBeUndefined();
  expect(requireImplemented("migration")).toBeUndefined();
  expect(requireImplemented("inventory")).toBeUndefined();
});

test("target shape, secrets and destructive confirmation stay closed", async () => {
  expect(validateTarget(inputs, {}, secrets)).toBe("connection");
  expect(validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_DATABASE_URL: "https://" + HOST + "/" })).toBe("connection");
  const original = JSON.stringify([inputs, settings, secrets]);
  validateTarget(inputs, settings, secrets);
  expect(JSON.stringify([inputs, settings, secrets])).toBe(original);
  for (const key of Object.keys(secrets)) {
    for (const value of ["", null]) await denied("MISSING_SECRET", () => validateTarget(inputs, settings, { ...secrets, [key]: value }));
  }
  for (const url of [
    "http://" + HOST, "file:///tmp/database", "libsql://localhost", "libsql://127.0.0.1", "libsql://other.example.org",
    "libsql://user:password@" + HOST, "https://" + HOST + ":443", "https://" + HOST + "/replica",
    "https://" + HOST + "?token=FAKE", "https://" + HOST + "#FAKE", "https://" + HOST + "?", "https://" + HOST + "#",
    "https://" + HOST + "\\@wrong.turso.io", "https://[broken",
  ]) {
    await denied("INVALID_PRIMARY_URL", () => validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_DATABASE_URL: url }));
  }
  for (const host of ["localhost", "127.0.0.1", "owner.example.org", "owner.turso.io.evil.org", "OWNER.turso.io", "turso.io"]) {
    await denied("INVALID_PRIMARY_URL", () => validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_DATABASE_URL: "https://" + host }));
  }
  for (const token of ["FAKE\nTOKEN", " FAKE", "x".repeat(16385)]) {
    await denied("INVALID_SECRET_FORMAT", () => validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_AUTH_TOKEN: token }));
  }
  await denied("CONNECTION_MUST_BE_READ_ONLY", () => validateTarget({ ...inputs, destructive: true }, settings, secrets));
  for (const [flag, allow] of [[false, "true"], [true, "false"], [true, "TRUE"], [true, ""]] as const) {
    await denied("DESTRUCTIVE_NOT_ALLOWED", () => validateTarget({ ...inputs, phase: "crud", destructive: flag }, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow }, secrets));
  }
  expect(validateTarget({ ...inputs, phase: "crud", destructive: true }, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" }, secrets)).toBe("crud");
  const printed: string[] = [];
  const quiet = harness({ print: (line) => printed.push(line), eprint: (line) => printed.push(line) });
  await denied("INVALID_PRIMARY_URL", () => validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_DATABASE_URL: "https://FAKE_SECRET_SENTINEL@wrong.turso.io" }));
  expect(printed.join("")).toBe("");
  expect(quiet.out.join("")).toBe("");
  for (const phase of PHASES) {
    if (phase === "connection" || phase === "inventory" || phase === "ui-baseline") continue;
    await denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", () => validateDispatch(context, { phase, destructive: false }, SHA));
    expect(validateDispatch(context, { phase, destructive: true }, SHA)).toBe(phase);
    if (phase !== "migration" && phase !== "reset" && phase !== "ui-ack") await denied("NOT_IMPLEMENTED", () => requireImplemented(phase));
  }
  const migration = { phase: "migration", destructive: true };
  expect(validateDispatch(context, migration, SHA)).toBe("migration");
  await denied("SECRET_MODE_REQUIRES_MANUAL", () => validateDispatch({ ...context, event_name: "push", ref: REVIEWED_REF }, migration, SHA));
  await denied("DESTRUCTIVE_NOT_ALLOWED", () => validateTarget(migration, {}, secrets));
  expect(validateTarget(migration, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" }, secrets)).toBe("migration");
});

test("environment policy accepts only the preexisting positive id", async () => {
  const environment = { name: ENVIRONMENT, id: 123, deployment_branch_policy: null };
  expect(validateEnvironment(environment)).toBeUndefined();
  for (const changed of [{}, { ...environment, id: 0 }, { ...environment, name: "other" }, { ...environment, id: true }]) {
    await denied("ENVIRONMENT_POLICY_DENIED", () => validateEnvironment(changed));
  }
  expect(API_ROOT).toBe("https://api.github.com/repos/AISFlow/fvoci/environments/fvoci-turso-test");
});

test("migration receipt and diagnostic echo only closed static codes", () => {
  expect(MIGRATION_PRIMARY_CODES).toEqual(new Set([
    "BEGIN_FAILED", "CLOSE_FAILED", "COMMIT_UNCONFIRMED", "CURRENT_APPLY_FAILED", "CURRENT_GATE_FAILED",
    "CURRENT_GATE_MISMATCH", "CURRENT_LINEAGE_CHANGED", "DATA_DECODE_FAILED", "DATA_QUERY_FAILED",
    "DATA_WRITE_FAILED", "DATA_WRITE_MISMATCH", "DDL_FAILED", "DEFER_PRAGMA_REFUSED", "FENCE_BASELINE_NOT_EMPTY",
    "FENCE_ROW_UNBOUND", "FENCE_WRITE_FAILED", "FK_DECODE_FAILED", "FK_FAILURE_MISSING", "FK_QUERY_FAILED",
    "FK_ROLLBACK_PREFIX_CHANGED", "FOREIGN_KEYS_NOT_ONE", "GENERATION_WRITE_FAILED", "GENERATION_WRITE_MISMATCH",
    "INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED", "LEASES_NOT_ZERO", "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH",
    "LITERAL_QUERY_FAILED", "NEGATIVE_REFUSAL_NOT_CONFIRMED", "NEGATIVE_ROLLBACK_CHANGED_CURRENT", "NEGATIVE_WRITE_FAILED",
    "NOT_FK_ONLY", "PARENT_PRESENT", "PREFIX_APPLY_FAILED", "PREFIX_RECEIPTS_CHANGED", "PREFIX_VALIDATION_FAILED",
    "PRESERVED_DATA_MISMATCH", "RECONNECT_FAILED", "RESTART_APPLY_FAILED", "RESTART_RECEIPTS_OR_SCHEMA_CHANGED",
    "ROLLBACK_UNCONFIRMED", "SCHEMA_VALIDATION_FAILED", "SEED_DECODE_FAILED", "SEED_MISMATCH", "SEED_QUERY_FAILED",
    "UNEXPECTED_TARGET_DATA", "WITNESS_DECODE_FAILED", "WITNESS_MISMATCH", "WITNESS_QUERY_FAILED", "WRONG_BACKEND",
    "WRONG_FK_FAILURE",
  ]));
  expect(MIGRATION_CLOSE_CODES).toEqual(new Set(["CLOSE_FAILED", "LEASES_NOT_ZERO"]));
  expect(MIGRATION_FK_PROOF_KINDS).toEqual(new Set(["EXTENDED", "SAME_WRITER_PRIMARY_HRANA"]));
  const valid = "test " + MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK fk_proof=EXTENDED current=OK restart=OK close=OK leases=ZERO\nok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n";
  const failure = "test " + MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=FAILED prefix=NOT_CONFIRMED fk_rollback=NOT_CONFIRMED fk_proof=NOT_CONFIRMED current=NOT_CONFIRMED restart=NOT_CONFIRMED close=OK leases=ZERO\nFAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out;\n";
  const { io, out } = harness();
  migrationResult({ status: 0 }, valid, io);
  expect(out.join("")).toContain("TURSO_MIGRATION_FK_PROOF kind=EXTENDED\n");
  expect(out.join("")).toContain("TURSO_MIGRATION_PASS tests=1 ignored=0");
  expect(out.join("")).toContain("TURSO_MIGRATION_RECEIPT OK OK OK EXTENDED OK OK OK ZERO\n");
  const same = harness();
  migrationResult({ status: 0 }, valid.replace("fk_proof=EXTENDED", "fk_proof=SAME_WRITER_PRIMARY_HRANA"), same.io);
  expect(same.out.join("")).toContain("TURSO_MIGRATION_FK_PROOF kind=SAME_WRITER_PRIMARY_HRANA\n");
  const cases: Array<[string, string]> = [
    [valid.replace("1 passed", "0 passed"), "TURSO_MIGRATION_FAILED"],
    [valid.replace("fk_proof=EXTENDED", "fk_proof=NOT_CONFIRMED"), "TURSO_MIGRATION_FAILED"],
    [valid.replace(" fk_proof=EXTENDED", ""), "TURSO_MIGRATION_RECEIPT_MISSING"],
    [valid.replace("fk_proof=EXTENDED", "fk_proof=EXTENDED witness=PROVEN"), "TURSO_MIGRATION_RECEIPT_MISSING"],
    [valid.replace("fk_proof=EXTENDED", "fk_proof=GENERIC_19"), "TURSO_MIGRATION_RECEIPT_MISSING"],
    [valid.replace("fk_proof=EXTENDED", "fk_proof=extended"), "TURSO_MIGRATION_RECEIPT_MISSING"],
    [valid.replace("fk_proof=EXTENDED", "fk_proof=EXTENDED_FAKE_PRIVATE_TOKEN"), "TURSO_MIGRATION_RECEIPT_MISSING"],
    [valid + valid, "TURSO_MIGRATION_RECEIPT_MISSING"],
    [valid.replace(MIGRATION_TEST_NAME, TEST_NAME), "TURSO_MIGRATION_FAILED"],
    [valid.replace("close=OK", "close=FAILED"), "TURSO_MIGRATION_FAILED"],
  ];
  for (const [text, code] of cases) {
    const captured = harness();
    expect(() => migrationResult({ status: 0 }, text + "FAKE_PRIVATE_TOKEN_NEVER_PRINT", captured.io)).toThrow(AdmissionError);
    try {
      migrationResult({ status: 0 }, text + "FAKE_PRIVATE_TOKEN_NEVER_PRINT", captured.io);
    } catch (error) {
      expect((error as Error).message).toBe(code);
    }
    expect(captured.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
  }
  const failed = harness();
  expect(() => migrationResult({ status: 1 }, valid, failed.io)).toThrow("TURSO_MIGRATION_FAILED");
  for (const primary of MIGRATION_PRIMARY_CODES) {
    for (const close of ["OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"]) {
      const receipt = close === "OK" ? failure : failure.replace("close=OK", "close=FAILED");
      const diagnostic = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close + "\n";
      const captured = harness();
      expect(() => migrationResult({ status: 1 }, receipt + diagnostic + "SDK FAKE_PRIVATE_TOKEN\n", captured.io)).toThrow("TURSO_MIGRATION_FAILED");
      expect(captured.out.join("")).toContain("TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close + "\n");
      expect(captured.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
      expect(captured.out.join("")).not.toContain("TURSO_MIGRATION_PASS");
    }
  }
  const known = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK\n";
  for (const diagnostic of [known + known, known.replace("SCHEMA_VALIDATION_FAILED", "FAKE_PRIVATE_TOKEN"), known.replace("SCHEMA_VALIDATION_FAILED", "OK"), known.trimEnd() + " FAKE_PRIVATE_TOKEN\n"]) {
    const captured = harness();
    expect(() => migrationResult({ status: 1 }, failure + diagnostic, captured.io)).toThrow("TURSO_MIGRATION_FAILED");
    expect(captured.out.join("")).not.toContain("TURSO_MIGRATION_DIAGNOSTIC");
    expect(captured.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
  }
  for (const separator of ["\r", "\v", "\f", "\u2028", "\u2029"]) {
    const captured = harness();
    expect(() => migrationResult({ status: 1 }, failure + known.trimEnd() + separator + "FAKE_PRIVATE_TOKEN\n", captured.io)).toThrow("TURSO_MIGRATION_FAILED");
    expect(captured.out.join("")).not.toContain("TURSO_MIGRATION_DIAGNOSTIC");
  }
  const passed = harness();
  migrationResult({ status: 0 }, valid + "SDK FAKE_PRIVATE_TOKEN\n", passed.io);
  expect(passed.out.join("")).toContain("TURSO_MIGRATION_PASS tests=1 ignored=0");
  expect(passed.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
});

function inventorySuccess(classification = "CURRENT", prefix = 12) {
  return "\nrunning 1 test\ntest " + INVENTORY_TEST_NAME + " ... FVOCI_TURSO_INVENTORY_RECEIPT classification=" + classification
    + " prefix=" + prefix + " schema_sha256=" + "a".repeat(64) + " rollback=OK close=OK leases=ZERO\nok\n\n"
    + "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n";
}

function inventoryFailure(primary = "INVENTORY_SCHEMA_REFUSED", rollback = "OK", close = "OK", leases = "ZERO", harnessText?: string) {
  const receiptRollback = { OK: "OK", NOT_STARTED: "NOT_STARTED", ROLLBACK_UNCONFIRMED: "FAILED" }[rollback];
  const returned = primary !== "OK" ? primary : rollback !== "OK" ? rollback : close !== "OK" ? close : leases === "FAILED" ? "LEASES_NOT_ZERO" : "INVENTORY_DISCLOSURE_REFUSED";
  const body = harnessText ?? 'Error: "' + returned + '"\n';
  return "\nrunning 1 test\ntest " + INVENTORY_TEST_NAME + " ... FVOCI_TURSO_INVENTORY_RECEIPT classification=REFUSED prefix=NONE schema_sha256=NONE rollback="
    + receiptRollback + " close=" + (close === "OK" ? "OK" : "FAILED") + " leases=" + leases + "\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC primary="
    + primary + " rollback=" + rollback + " close=" + close + " leases=" + leases + "\nFVOCI_TURSO_INVENTORY_RETURN\n" + body
    + "FAILED\n\nfailures:\n\nfailures:\n    " + INVENTORY_TEST_NAME + "\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n";
}

test("inventory success and failure frames disclose only closed facts", () => {
  expect([...INVENTORY_PRIMARY_CODES].sort()).toEqual([
    "BEGIN_FAILED", "CURRENT_LINEAGE_CHANGED", "FK_DECODE_FAILED", "FK_QUERY_FAILED", "FOREIGN_KEYS_NOT_ONE",
    "INVENTORY_DECODE_FAILED", "INVENTORY_HASH_INVALID", "INVENTORY_PREFIX_REFUSED", "INVENTORY_QUERY_FAILED",
    "INVENTORY_SCHEMA_REFUSED", "INVENTORY_SNAPSHOT_MISMATCH", "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH",
    "LITERAL_QUERY_FAILED", "WRONG_BACKEND", "WRONG_PRODUCT_BACKEND",
  ]);
  for (let prefix = 0; prefix < 13; prefix++) {
    const classification = prefix === 0 ? "BLANK" : prefix === 12 ? "CURRENT" : "PREFIX";
    for (const ending of ["\n", "\r\n"]) {
      const captured = harness();
      inventoryResult({ status: 0 }, inventorySuccess(classification, prefix).replaceAll("\n", ending), captured.io);
      expect(captured.out.join("")).toBe("TURSO_INVENTORY_RECEIPT classification=" + classification + " prefix=" + prefix + " schema_sha256=" + "a".repeat(64) + " rollback=OK close=OK leases=ZERO\nTURSO_INVENTORY_PASS tests=1 ignored=0\n");
    }
  }
  const valid = inventorySuccess();
  for (const changed of ["", valid.replace("1 passed", "0 passed"), valid.replace(INVENTORY_TEST_NAME, MIGRATION_TEST_NAME), valid + "SDK FAKE_PRIVATE_TOKEN\n", valid.replaceAll("\n", "\r")]) {
    const captured = harness();
    expect(() => inventoryResult({ status: 0 }, changed, captured.io)).toThrow("TURSO_INVENTORY_FAILED");
    expect(captured.out.join("")).toBe("");
  }
  for (const code of INVENTORY_PRIMARY_CODES) {
    const rollbacks = code === "BEGIN_FAILED" || code === "WRONG_PRODUCT_BACKEND" ? ["NOT_STARTED"] : ["OK", "ROLLBACK_UNCONFIRMED"];
    for (const rollback of rollbacks) {
      for (const close of ["OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"]) {
        for (const leases of ["ZERO", "FAILED"]) {
          const captured = harness();
          expect(() => inventoryResult({ status: 1 }, inventoryFailure(code, rollback, close, leases), captured.io)).toThrow("TURSO_INVENTORY_FAILED");
          expect(captured.out.join("")).toContain("TURSO_INVENTORY_DIAGNOSTIC primary=" + code + " rollback=" + rollback + " close=" + close + " leases=" + leases + "\n");
          expect(captured.out.join("")).not.toContain("TURSO_INVENTORY_PASS");
        }
      }
    }
  }
  for (const [primary, rollback] of [["OK", "OK"], ["OK", "NOT_STARTED"], ["BEGIN_FAILED", "OK"]] as const) {
    const captured = harness();
    expect(() => inventoryResult({ status: 1 }, inventoryFailure(primary, rollback), captured.io)).toThrow("TURSO_INVENTORY_FAILED");
    expect(captured.out.join("")).toBe("");
  }
  const privateFrame = harness();
  expect(() => inventoryResult({ status: 1 }, inventoryFailure("INVENTORY_SCHEMA_REFUSED", "OK", "OK", "ZERO", 'Error: "FAKE_PRIVATE_TOKEN"\n'), privateFrame.io)).toThrow("TURSO_INVENTORY_FAILED");
  expect(privateFrame.out.join("")).toContain("primary=INVENTORY_SCHEMA_REFUSED");
  expect(privateFrame.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
  const forged = harness();
  expect(() => inventoryResult({ status: 1 }, inventoryFailure("INVENTORY_SCHEMA_REFUSED", "OK", "OK", "ZERO", "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=DDL_FAILED close=OK\n"), forged.io)).toThrow("TURSO_INVENTORY_FAILED");
  expect(forged.out.join("")).toBe("");
});

function resetSuccess() {
  return "\nrunning 1 test\ntest " + RESET_TEST_NAME + " ... FVOCI_TURSO_RESET_RECEIPT primary=OK rollback=NOT_STARTED commit=RETURNED_OK blank=CONFIRMED steps=126 close=OK drain=LOCAL_OK leases=ZERO\nok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n";
}

test("reset success and failure receipts never become a pass from opaque text", () => {
  const captured = harness();
  resetResult({ status: 0 }, resetSuccess(), captured.io);
  expect(captured.out.join("")).toBe("TURSO_RESET_RECEIPT primary=OK rollback=NOT_STARTED commit=RETURNED_OK blank=CONFIRMED steps=126 close=OK drain=LOCAL_OK leases=ZERO\nTURSO_RESET_PASS tests=1 ignored=0\n");
  for (const text of [resetSuccess().replace("steps=126", "steps=125"), resetSuccess() + "FAKE_PRIVATE_TOKEN\n", resetSuccess().replace(RESET_TEST_NAME, MIGRATION_TEST_NAME)]) {
    const failed = harness();
    expect(() => resetResult({ status: 0 }, text, failed.io)).toThrow("TURSO_RESET_FAILED");
    expect(failed.out.join("")).toBe("");
  }
  const fields = "primary=RESET_SCHEMA_REFUSED rollback=RETURNED_OK commit=NOT_STARTED blank=NOT_RUN steps=0 close=OK drain=LOCAL_OK leases=ZERO";
  const text = "\nrunning 1 test\ntest " + RESET_TEST_NAME + " ... FVOCI_TURSO_RESET_RECEIPT " + fields + "\n\nFVOCI_TURSO_RESET_RETURN\nError: FAKE_PRIVATE_TOKEN\nFAILED\n\nfailures:\n\nfailures:\n    " + RESET_TEST_NAME + "\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n";
  const disclosed = harness();
  expect(() => resetResult({ status: 1 }, text, disclosed.io)).toThrow("TURSO_RESET_FAILED");
  expect(disclosed.out.join("")).toBe("TURSO_RESET_FAILURE " + fields + "\n");
  expect(disclosed.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
  const hidden = harness();
  expect(() => resetResult({ status: 1 }, text.replace("primary=", "primary=PRIVATE_"), hidden.io)).toThrow("TURSO_RESET_FAILED");
  expect(hidden.out.join("")).toBe("");
});

function frozen(calls: SpawnResult[]) {
  const root = mkdtempSync(join(tmpdir(), "fvoci-turso-guard-"));
  const binary = join(root, "turso-connection-libtest");
  writeFileSync(binary, Buffer.from([0x7f, 0x45, 0x4c, 0x46, 0x70]));
  mkdirSync(join(root, "fvoci-sqlite"));
  const native = join(root, "fvoci-sqlite", "consumer-inputs.json");
  writeFileSync(native, "pure metadata fixture, not native proof");
  const cargo = join(root, "turso-compile.json");
  writeFileSync(cargo, "pure retained compile-output binding");
  const { createHash } = require("node:crypto") as typeof import("node:crypto");
  const digest = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");
  const manifest = { sha: SHA, source_digest: "fixture", binary_sha256: digest(binary), native_input_sha256: digest(native), cargo_output_sha256: digest(cargo) };
  writeFileSync(join(root, "turso-connection-build.json"), JSON.stringify(manifest));
  let index = 0;
  const seen: string[][] = [];
  const envs: Record<string, string>[] = [];
  const spawn: GuardIO["spawn"] = async (args, env) => {
    seen.push(args);
    envs.push(env);
    const next = calls[Math.min(index, calls.length - 1)]!;
    index += 1;
    return next;
  };
  return { root, spawn, seen, envs, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

test("diagnostic unit runs one exact test without credential environment", async () => {
  const listing = DIAGNOSTIC_UNIT_NAME + ": test\n\n1 test, 0 benchmarks\n";
  const success = "running 1 test\ntest " + DIAGNOSTIC_UNIT_NAME + " ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n";
  const fixture = frozen([{ status: 0, stdout: encode(listing) }, { status: 0, stdout: encode(success) }]);
  try {
    const { io, out } = harness({
      env: { PATH: "/usr/bin", RUNNER_TEMP: fixture.root, FVOCI_LIBSQL_URL: "FAKE_PRIVATE_TOKEN", GITHUB_TOKEN: "FAKE_PRIVATE_TOKEN" },
      spawn: fixture.spawn,
    });
    await runDiagnosticUnit(SHA, io);
    expect(out.join("")).toBe("TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN\n");
    expect(fixture.seen[0]).toEqual([join(fixture.root, "turso-connection-libtest"), DIAGNOSTIC_UNIT_NAME, "--list", "--exact"]);
    expect(fixture.seen[1]).toEqual([join(fixture.root, "turso-connection-libtest"), DIAGNOSTIC_UNIT_NAME, "--exact", "--test-threads=1"]);
    expect(fixture.envs[0]).toEqual({ PATH: "/usr/bin" });
    expect(JSON.stringify(fixture.envs)).not.toContain("FAKE_PRIVATE_TOKEN");
  } finally {
    fixture.cleanup();
  }
  const bad = frozen([{ status: 0, stdout: encode("0 tests, 0 benchmarks\nFAKE_PRIVATE_TOKEN\n") }]);
  try {
    const captured = harness({ env: { PATH: "/usr/bin", RUNNER_TEMP: bad.root }, spawn: bad.spawn });
    await denied("TURSO_DIAGNOSTIC_UNIT_SELECTION_FAILED", () => runDiagnosticUnit(SHA, captured.io));
    expect(captured.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
    expect(bad.seen.length).toBe(1);
  } finally {
    bad.cleanup();
  }
});

test("freeze copies one current libtest and refuses drifted compiler output", () => {
  const root = mkdtempSync(join(tmpdir(), "fvoci-turso-freeze-"));
  try {
    const binary = join(root, "turso-target", "debug", "deps", "fixture-libtest");
    mkdirSync(join(root, "turso-target", "debug", "deps"), { recursive: true });
    mkdirSync(join(root, "fvoci-sqlite"));
    writeFileSync(binary, Buffer.from([0x7f, 0x45, 0x4c, 0x46, 0x66]));
    writeFileSync(join(root, "fvoci-sqlite", "consumer-inputs.json"), "pure metadata fixture, not native proof");
    const artifact = { reason: "compiler-artifact", target: { kind: ["lib"], name: "fvoci_server", src_path: join(process.cwd(), "src", "lib.rs") }, profile: { test: true }, features: ["db-tests"], executable: binary };
    writeFileSync(join(root, "turso-compile.json"), JSON.stringify(artifact) + "\n" + JSON.stringify({ reason: "build-finished", success: true }) + "\n");
    const { io } = harness({ env: { RUNNER_TEMP: root } });
    freezeCompiledTest(SHA, io);
    expect(readFileSync(join(root, "turso-connection-libtest")).equals(readFileSync(binary))).toBe(true);
    const manifest = JSON.parse(readFileSync(join(root, "turso-connection-build.json"), "utf8"));
    expect(manifest.sha).toBe(SHA);
    expect(manifest.source_digest).toBe("fixture");
    writeFileSync(join(root, "turso-compile.json"), JSON.stringify({ ...artifact, features: ["db-tests", "api-schema"] }) + "\n" + JSON.stringify({ reason: "build-finished", success: true }));
    rmSync(join(root, "turso-connection-libtest"));
    rmSync(join(root, "turso-connection-build.json"));
    expect(() => freezeCompiledTest(SHA, io)).toThrow("COMPILED_TEST_BINDING_FAILED");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("primary children receive only the phase environment and fail closed", async () => {
  const connection = "test " + TEST_NAME + " ... FVOCI_TURSO_RECEIPT primary=OK rollback=OK close=OK leases=ZERO\nok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n";
  const migration = "test " + MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK fk_proof=EXTENDED current=OK restart=OK close=OK leases=ZERO\nok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n";
  const baseEnv = {
    PATH: "/usr/bin", LD_LIBRARY_PATH: "/fixture/lib", SSL_CERT_FILE: "/fixture/cert", SSL_CERT_DIR: "/fixture/certs", TZ: "UTC",
    FVOCI_DATABASE_BACKEND: "libsql-remote", FVOCI_TEST_TURSO_DATABASE_URL: secrets.FVOCI_TEST_TURSO_DATABASE_URL,
    FVOCI_TEST_TURSO_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN", UNRELATED_FAKE_CREDENTIAL: "never forwarded",
  };
  const connectionRun = frozen([{ status: 0, stdout: encode(connection) }]);
  try {
    const captured = harness({ env: { ...baseEnv, RUNNER_TEMP: connectionRun.root, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false" }, spawn: connectionRun.spawn });
    await runConnection(SHA, inputs, captured.io);
    expect(captured.out.join("")).toContain("TURSO_CONNECTION_PASS tests=1 ignored=0");
    expect(connectionRun.seen[0]!.slice(1)).toEqual([TEST_NAME, "--ignored", "--exact", "--test-threads=1", "--nocapture"]);
    expect(connectionRun.envs[0]!.FVOCI_LIBSQL_AUTH_TOKEN).toBe("FAKE_PRIVATE_TOKEN");
    expect(connectionRun.envs[0]).not.toHaveProperty("FVOCI_TEST_TURSO_AUTH_TOKEN");
    expect(connectionRun.envs[0]).not.toHaveProperty("UNRELATED_FAKE_CREDENTIAL");
    expect(captured.out.join("")).not.toContain("FAKE_PRIVATE");
    connectionRun.seen.length = 0;
    const zero = harness({ env: captured.io.env, spawn: async () => ({ status: 0, stdout: encode(connection.replace("1 passed", "0 passed") + "FAKE_PRIVATE_BODY_NEVER_PRINT") }) });
    await denied("TURSO_CONNECTION_FAILED", () => runConnection(SHA, inputs, zero.io));
    expect(zero.out.join("")).not.toContain("FAKE_PRIVATE_BODY");
  } finally {
    connectionRun.cleanup();
  }
  const migrationRun = frozen([{ status: 0, stdout: encode(migration) }]);
  try {
    const captured = harness({
      env: { ...baseEnv, RUNNER_TEMP: migrationRun.root, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" },
      spawn: migrationRun.spawn,
    });
    await runMigration(SHA, { phase: "migration", destructive: true }, captured.io);
    expect(migrationRun.envs[0]).toMatchObject({
      FVOCI_TEST_TURSO_MIGRATION_SELECTED: "1", FVOCI_TEST_TURSO_PHASE: "migration",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "true", FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
    });
    expect(migrationRun.envs[0]).not.toHaveProperty("FVOCI_TEST_TURSO_CONNECTION_SELECTED");
    await denied("WRONG_CONSUMER_PHASE", () => runConnection(SHA, { phase: "migration", destructive: true }, captured.io));
  } finally {
    migrationRun.cleanup();
  }
  const inventoryRun = frozen([{ status: 0, stdout: encode(inventorySuccess()) }]);
  try {
    const captured = harness({
      env: { ...baseEnv, RUNNER_TEMP: inventoryRun.root, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false", FVOCI_TEST_TURSO_PHASE: "migration", FVOCI_TEST_TURSO_DESTRUCTIVE: "true" },
      spawn: inventoryRun.spawn,
    });
    await runInventory(SHA, { phase: "inventory", destructive: false }, captured.io);
    expect(captured.out.join("")).toContain("TURSO_INVENTORY_PASS tests=1 ignored=0");
    expect(inventoryRun.envs[0]).toMatchObject({ FVOCI_TEST_TURSO_PHASE: "inventory", FVOCI_TEST_TURSO_DESTRUCTIVE: "false", FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false" });
    expect(inventoryRun.envs[0]).not.toHaveProperty("FVOCI_TEST_TURSO_DATABASE_URL");
    expect(inventoryRun.envs[0]).not.toHaveProperty("UNRELATED_FAKE_CREDENTIAL");
    const refused = harness({ env: { ...captured.io.env, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" }, spawn: async () => { throw new Error("spawned"); } });
    await denied("INVENTORY_MUST_BE_READ_ONLY", () => runInventory(SHA, { phase: "inventory", destructive: false }, refused.io));
  } finally {
    inventoryRun.cleanup();
  }
  const resetRun = frozen([{ status: 0, stdout: encode(resetSuccess()) }]);
  try {
    const captured = harness({ env: { ...baseEnv, RUNNER_TEMP: resetRun.root, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" }, spawn: resetRun.spawn });
    await runReset(SHA, { phase: "reset", destructive: true }, captured.io);
    expect(captured.out.join("")).toContain("TURSO_RESET_PASS tests=1 ignored=0");
    expect(resetRun.envs[0]!.FVOCI_TEST_TURSO_AUTH_TOKEN).toBe("FAKE_PRIVATE_TOKEN");
    expect(resetRun.envs[0]).not.toHaveProperty("FVOCI_LIBSQL_URL");
    expect(resetRun.envs[0]).not.toHaveProperty("FVOCI_LIBSQL_AUTH_TOKEN");
    expect(captured.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
  } finally {
    resetRun.cleanup();
  }
});

test("cli admits bootstrap without secrets and redacts parse failures", () => {
  const sha = Bun.spawnSync(["git", "rev-parse", "HEAD"], { stdout: "pipe" }).stdout.toString().trim();
  const directory = mkdtempSync(join(tmpdir(), "fvoci-turso-cli-"));
  try {
    const event = join(directory, "event.json");
    writeFileSync(event, '{"inputs": {}}');
    const env = { PATH: process.env.PATH ?? "", GITHUB_EVENT_PATH: event, GITHUB_EVENT_NAME: "push", GITHUB_REPOSITORY: "AISFlow/fvoci", GITHUB_REF: REVIEWED_REF, GITHUB_SHA: sha };
    const admit = Bun.spawnSync(["bun", "scripts/selected-backend-ci/turso-test-guard.ts", "--admit"], { env, stdout: "pipe", stderr: "pipe" });
    expect([admit.exitCode, admit.stdout.toString(), admit.stderr.toString()]).toEqual([0, "BOOTSTRAP_SOURCE_ADMISSION_OK_RUNTIME_NOT_RUN\n", ""]);
    const consume = Bun.spawnSync(["bun", "scripts/selected-backend-ci/turso-test-guard.ts", "--consume"], { env, stdout: "pipe", stderr: "pipe" });
    expect([consume.exitCode, consume.stdout.toString(), consume.stderr.toString()]).toEqual([78, "", "SECRET_MODE_REQUIRES_MANUAL\n"]);
    const manual = { ...env, GITHUB_EVENT_NAME: "workflow_dispatch", GITHUB_REF: "refs/heads/main" };
    writeFileSync(event, JSON.stringify({ inputs: { phase: "connection", destructive: "false" } }));
    const missing = Bun.spawnSync(["bun", "scripts/selected-backend-ci/turso-test-guard.ts", "--consume"], { env: { ...manual, FVOCI_DATABASE_BACKEND: "libsql-remote" }, stdout: "pipe", stderr: "pipe" });
    expect([missing.exitCode, missing.stdout.toString(), missing.stderr.toString()]).toEqual([78, "", "MISSING_SECRET\n"]);
    writeFileSync(event, '{"FAKE_SECRET_SENTINEL_NEVER_REAL":');
    const broken = Bun.spawnSync(["bun", "scripts/selected-backend-ci/turso-test-guard.ts", "--consume"], { env: manual, stdout: "pipe", stderr: "pipe" });
    expect([broken.exitCode, broken.stdout.toString(), broken.stderr.toString()]).toEqual([78, "", "ADMISSION_FAILED\n"]);
    const mode = Bun.spawnSync(["bun", "scripts/selected-backend-ci/turso-test-guard.ts"], { env, stdout: "pipe", stderr: "pipe" });
    expect([mode.exitCode, mode.stderr.toString()]).toEqual([78, "EXPLICIT_MODE_REQUIRED\n"]);
    expect(mode.stdout.toString()).not.toContain("FAKE_SECRET");
    expect(broken.stderr.toString()).not.toContain("FAKE_SECRET");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("explicit modes route before secret consumers", async () => {
  const directory = mkdtempSync(join(tmpdir(), "fvoci-turso-route-"));
  try {
    const event = join(directory, "event.json");
    writeFileSync(event, JSON.stringify({ inputs: { phase: "connection", destructive: "false" } }));
    let unit = 0;
    let consumer = 0;
    const { io } = harness({
      env: { GITHUB_EVENT_PATH: event, GITHUB_EVENT_NAME: "workflow_dispatch", GITHUB_REPOSITORY: "AISFlow/fvoci", GITHUB_REF: REVIEWED_REF, GITHUB_SHA: SHA },
      gitRevParse: () => SHA,
    });
    const routed = defaultIO({
      ...io,
      spawn: async () => { consumer += 1; return { status: 0, stdout: new Uint8Array() }; },
    });
    const original = runDiagnosticUnit;
    const calls = { unit: 0 };
    const result = await main(["--diagnostic-unit"], {
      ...routed,
      spawn: async () => { calls.unit += 1; return { status: 0, stdout: encode(DIAGNOSTIC_UNIT_NAME + ": test\n") }; },
    });
    expect(result === 0 || result === 78).toBe(true);
    unit = calls.unit;
    expect(consumer).toBe(0);
    const quiet = { print: () => {}, eprint: () => {}, gitRevParse: () => SHA };
    const pushed = await main(["--consume"], { env: { ...routed.env, GITHUB_EVENT_NAME: "push" }, ...quiet });
    expect(pushed).toBe(78);
    let spawned = 0;
    const unitOnPush = await main(["--diagnostic-unit"], {
      env: { ...routed.env, GITHUB_EVENT_NAME: "push" },
      ...quiet,
      spawn: async () => { spawned += 1; return { status: 0, stdout: new Uint8Array() }; },
    });
    expect(unitOnPush).toBe(78);
    expect(spawned).toBe(0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("workflow keeps manual dispatch, exact sha checkout and pre-secret order", () => {
  const workflow = readFileSync(join(repo, ".github/workflows/turso-test.yml"), "utf8");
  expect(workflow).toContain("options: [connection, crud, transactions, migration, inventory, reset, persistence, restore, ui-ack, ui-baseline]");
  expect(workflow).toContain("        default: connection\n");
  expect(workflow).toContain("        type: boolean\n        default: false\n");
  expect(workflow).toContain("  group: fvoci-turso-test-database\n  cancel-in-progress: false\n");
  expect(workflow).toContain("permissions:\n  contents: read\n");
  expect(workflow.split("persist-credentials: false").length - 1).toBe(3);
  expect(workflow.split("ref: ${{ github.sha }}").length - 1).toBe(3);
  expect(workflow.split("github.repository == 'AISFlow/fvoci'").length - 1).toBe(3);
  expect(workflow.split("environment: fvoci-turso-test").length - 1).toBe(2);
  const guard = "turso-test-guard.py";
  const freeze = workflow.indexOf(guard + " --freeze");
  const unit = workflow.indexOf(guard + " --diagnostic-unit");
  const secret = workflow.indexOf("      - name: Real primary selected phase");
  expect(freeze).toBeGreaterThan(0);
  expect(freeze).toBeLessThan(unit);
  expect(unit).toBeLessThan(secret);
  expect(workflow.split(guard + " --diagnostic-unit").length - 1).toBe(1);
  expect(workflow.slice(freeze, secret)).not.toContain("secrets.");
  expect(workflow.slice(0, secret)).not.toContain("secrets.");
  const connection = workflow.split("  turso-connection:", 2)[1]!.split("  turso-ui:", 2)[0]!;
  expect(connection).not.toContain(UI_REVIEWED_REF);
  expect(workflow.split("jobs:", 2)[0]).not.toContain(UI_REVIEWED_REF);
  const prefix = "set -euo pipefail\nprintf 'CARGO_TARGET_DIR=%s/turso-target\\n' \"$RUNNER_TEMP\" >> \"$GITHUB_ENV\"\n";
  expect(workflow).toContain(prefix.trimEnd().split("\n")[1]);
});

test("reset drop order matches the current sqlite schema", () => {
  const source = readFileSync(join(repo, "src/db/turso_test.rs"), "utf8");
  const literal = source.split("const RESET_DROP_STATEMENTS: [&str; 126] = [", 2)[1]!.split("];", 2)[0]!;
  const statements = literal.split("\n").map((line) => line.trim()).filter(Boolean).map((line) => JSON.parse(line.replace(/,$/, "")) as string);
  const tables = new Map<string, Set<string>>();
  const triggers = new Set<string>();
  const files = new Bun.Glob("*.sql").scanSync(join(repo, "migrations/sqlite/060"));
  const names = [...files].sort();
  for (const name of names) {
    const sql = readFileSync(join(repo, "migrations/sqlite/060", name), "utf8");
    for (const match of sql.matchAll(/CREATE TABLE (\w+)\s*\(/g)) {
      const body = sql.slice(match.index ?? 0, sql.indexOf(";", match.index ?? 0));
      const refs = new Set([...body.matchAll(/REFERENCES (\w+)/g)].map((item) => item[1]!).filter((item) => item !== match[1]));
      tables.set(match[1]!, refs);
    }
    for (const match of sql.matchAll(/CREATE TRIGGER (\w+)/g)) triggers.add(match[1]!);
  }
  const dropTriggers = statements.slice(0, 27).map((statement) => statement.slice('DROP TRIGGER IF EXISTS "'.length, -2));
  const dropTables = statements.slice(27).map((statement) => statement.slice('DROP TABLE IF EXISTS "'.length, -2));
  expect(statements.length).toBe(126);
  expect(new Set(dropTriggers)).toEqual(triggers);
  expect(new Set(dropTables)).toEqual(new Set(tables.keys()));
  expect(dropTables.at(-1)).toBe("schema_migrations");
  for (const [child, parents] of tables) {
    for (const parent of parents) expect(dropTables.indexOf(child)).toBeLessThan(dropTables.indexOf(parent));
  }
  expect(statements.join("")).not.toContain("PRAGMA");
});

test("ui cli stays on fixed codes and the launcher is the closed shell program", () => {
  expect(FIXED_LAUNCHER.length).toBe(394);
  expect(FIXED_LAUNCHER).not.toContain("invented");
  expect(FIXED_LAUNCHER.endsWith("exec \"$@\"\n")).toBe(true);
  const directory = mkdtempSync(join(tmpdir(), "fvoci-turso-ui-cli-"));
  try {
    const env = { PATH: process.env.PATH ?? "", RUNNER_TEMP: directory, FVOCI_LIBSQL_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN" };
    const missing = Bun.spawnSync(["bun", "scripts/selected-backend-ci/turso-ui.ts"], { env, stdout: "pipe", stderr: "pipe" });
    expect([missing.exitCode, missing.stdout.toString(), missing.stderr.toString()]).toEqual([78, "", "UI_EXPLICIT_MODE_REQUIRED\n"]);
    const hosted = Bun.spawnSync(["bun", "scripts/selected-backend-ci/turso-ui.ts", "--record-before"], { env, stdout: "pipe", stderr: "pipe" });
    expect([hosted.exitCode, hosted.stdout.toString(), hosted.stderr.toString()]).toEqual([78, "", "UI_HOSTED_ALLOCATION_REQUIRED\n"]);
    expect(hosted.stderr.toString()).not.toContain("FAKE_PRIVATE_TOKEN");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("ui capsule and docker argv keep secrets out of the published command", () => {
  const environment = { FVOCI_LIBSQL_URL: "libsql://" + HOST, FVOCI_LIBSQL_AUTH_TOKEN: "invented-token" };
  const text = capsuleText(environment);
  expect(text).toContain("invented-token");
  const argv = containerArgv("fvoci-tui-aa", "/host/launcher.sh", "/host/native-env.sh", "/fvoci-current/bin/fvoci-e2e-fixture", [], ["baseline"]);
  expect(argv.join("\n")).not.toContain("invented-token");
  expect(argv).not.toContain("--env-file");
  expect(argv).not.toContain("-e");
  admitPublishedConfig(argv, ["PATH=/usr/bin"], ["invented-token"]);
  expect(() => admitPublishedConfig(argv, ["FVOCI_LIBSQL_AUTH_TOKEN=invented-token"], ["invented-token"])).toThrow(UiError);
  expect(() => capsuleText({ ...environment, OTHER_TOKEN: "FAKE_PRIVATE_TOKEN" })).toThrow("UI_DOCKER_ENV_SECRET_REFUSED");
});

test("baseline and server diagnostics project closed facts only", () => {
  const packet = {
    nativeOutcome: {
      operation: "failed", rollback: "not-attempted", commit: "not-attempted",
      baselineFailure: { phase: "schema-check", category: "database", private: "FAKE_PRIVATE_TOKEN" },
    },
    lifecycleDrain: "unconfirmed",
    drainOutcome: "failed",
  };
  const refused = baselineFailureDiagnostic(packet);
  expect(refused.diagnosticStatus).toBe("refused");
  expect(JSON.stringify(refused)).not.toContain("FAKE_PRIVATE_TOKEN");
  const qualified = baselineFailureDiagnostic({
    nativeOutcome: { operation: "failed", rollback: "not-attempted", commit: "not-attempted", baselineFailure: { phase: "schema-check", category: "database" } },
    lifecycleDrain: "unconfirmed", drainOutcome: "failed",
  });
  expect(qualified.diagnosticStatus).toBe("qualified");
  expect(qualified.originalFailure).toBe("TURSO_UI_BASELINE_FAILED");
  const keyring = Buffer.from("fvoci: ENCRYPTION_KEYS is not set (see the env example)\nfvoci: ENCRYPTION_ACTIVE_KEY_ID is not set (see the env example)\n");
  expect(serverStartDiagnostic({ poll: () => 2 }, keyring, 0, 1, 0.2).category).toBe("missing-encryption-keyring");
  const hidden = serverStartDiagnostic({ poll: () => 2 }, Buffer.from("libsql://FAKE_PRIVATE_TOKEN\n"), 0, 1, 0.2);
  expect(hidden.diagnosticStatus).toBe("unqualified");
  expect(JSON.stringify(hidden)).not.toContain("FAKE_PRIVATE_TOKEN");
  expect(startupRemoteCause("not a producer line FAKE_PRIVATE_TOKEN")).toBeNull();
  const empty = { fingerprints: Object.fromEntries([...Array(0)]), startupHazards: 0, liveOutboxLeases: 0, ledger: [1], schemaSha256: "a", lineage: "fvoci-sqlite-060" };
  const fingerprints: Record<string, Record<string, number>> = {};
  for (const table of ["documents", "tasks", "attachments", "attachment_object_cleanups", "revisions", "events", "outbox_consumers", "outbox_failures", "processed_events", "notifications", "notification_prefs", "ics_tokens", "magic_tokens", "github_deliveries", "github_install_states", "github_installations", "github_issue_links", "import_jobs", "import_deferred_events", "push_deliveries", "push_subscriptions", "webhook_deliveries", "webhooks"]) fingerprints[table] = {};
  const baseline = { ...empty, fingerprints };
  expect(startupBlockers(baseline)).toEqual([]);
  fingerprints.documents = { d: 1 };
  expect(startupBlockers({ ...baseline, fingerprints })).toEqual(["documents"]);
  expect(() => assertPreserved(baseline, { ...baseline, ledger: [2] })).toThrow("UI_CURRENT_LEDGER_CHANGED");
  expect(baselinePassLine("a".repeat(40), "b".repeat(64), "c".repeat(64), 4, false, true)).toBe(
    "TURSO_UI_BASELINE_PASS source=" + "a".repeat(40) + " baseline_sha256=" + "b".repeat(64) + " target_sha256=" + "c".repeat(64) + " rows=4 setup_needed=false startup_admissible=true",
  );
});

test("ui consume is refused before a database when the reviewed source does not match", async () => {
  const { io } = harness({
    env: {
      FVOCI_DATABASE_BACKEND: "libsql-remote",
      FVOCI_LIBSQL_URL: secrets.FVOCI_TEST_TURSO_DATABASE_URL,
      FVOCI_LIBSQL_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false",
    },
  });
  await denied("UI_REVIEWED_SOURCE_REQUIRED", () => runUi(SHA, { phase: "ui-baseline", destructive: false, ui_source_sha: "b".repeat(40) }, io));
  expect(io.print).toBeTypeOf("function");
  const captured = harness({ env: io.env });
  await denied("UI_REVIEWED_SOURCE_REQUIRED", () => runUi(SHA, { phase: "ui-baseline", destructive: false, ui_source_sha: "b".repeat(40) }, captured.io));
  expect(captured.out.join("")).not.toContain("FAKE_PRIVATE_TOKEN");
});
