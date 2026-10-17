import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { spawn } from "bun";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, writeSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { write } from "../selected-backend-ci/io.ts";
import { UiError } from "./ui-common.ts";
import { BLOCKED, localServerStart, type LocalStartSteps } from "./ui-container.ts";
import { Captured, exitedChild, fakeScope, must } from "./ui-fakes.ts";
import { start } from "./ui-native.ts";
import { poll, type Child } from "./ui-processes.ts";
import type { Manifest } from "./ui-record.ts";
import {
  START_DIAGNOSTIC_INPUT_CAP,
  START_GATE_TEXTS,
  serverStartDiagnostic,
  type StartAllocation,
} from "./ui-start-diagnostic.ts";

const START_MISSING =
  "fvoci: ENCRYPTION_KEYS is not set (see the env example)\n" +
  "fvoci: ENCRYPTION_ACTIVE_KEY_ID is not set (see the env example)\n" +
  "fvoci: not starting; fix .env and run docker compose up -d again\n";

let directory: string;
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "fvoci-ui-start-"));
  delete process.env.FVOCI_SELECTED_EXECUTION_MODE;
});
afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
});

function record(
  raw: string | Uint8Array,
  code: unknown = 2,
  now = 102,
  started = 100,
  deadline = 110,
  allocation: StartAllocation = "unknown",
) {
  const log = join(directory, "server.private.log");
  writeFileSync(log, raw);
  return serverStartDiagnostic(
    () => code,
    log,
    started,
    deadline,
    () => now,
    allocation,
  );
}

describe("start diagnostic", () => {
  test("exit two with the exact missing keyring yields a bounded public record", () => {
    const result = record(START_MISSING);
    expect(result).toEqual({
      originalFailure: "UI_SERVER_START_FAILED",
      diagnosticStatus: "qualified",
      processState: "exited",
      exitCode: 2,
      elapsedMs: 2000,
      allocation: "unknown",
      phase: "prepare-config",
      category: "missing-encryption-keyring",
    });
    expect(Buffer.byteLength(JSON.stringify(result))).toBeLessThanOrEqual(2048);
    expect(record(START_MISSING.replaceAll("\n", "\r\n"))).toEqual(result);
  });

  test("a real DB-free child exit and a live deadline snapshot", async () => {
    for (const exiting of [true, false]) {
      const log = join(directory, "server.private.log");
      writeFileSync(log, "");
      const child = spawn(
        exiting
          ? ["sh", "-c", "printf '%s' \"$0\" >&2; exit 2", START_MISSING]
          : ["sh", "-c", "exec sleep 30"],
        { stdout: "ignore", stderr: Bun.file(log), env: { PATH: process.env.PATH ?? "" } },
      ) as unknown as Child;
      try {
        if (exiting) expect(await child.exited).toBe(2);
        const result = serverStartDiagnostic(
          () => poll(child),
          log,
          100,
          110,
          () => 110,
        );
        expect(result.processState).toBe(exiting ? "exited" : "deadline");
        expect(result.exitCode).toBe(exiting ? 2 : null);
        expect(result.category).toBe(exiting ? "missing-encryption-keyring" : null);
      } finally {
        if (poll(child) === null) child.kill(15);
        await child.exited;
      }
    }
  });

  test("deadline, exit, signal and unknown observations", () => {
    for (const [code, now, state, exitCode] of [
      [null, 111, "deadline", null],
      [-15, 105, "exited", -15],
      [1, 105, "exited", 1],
      [null, 105, "unclassified", null],
      ["PRIVATE", 120, "unclassified", null],
    ] as const) {
      const result = record("", code, now);
      expect([result.processState, result.exitCode]).toEqual([state, exitCode]);
      expect(result.diagnosticStatus).toBe("unqualified");
      expect(result.phase).toBeNull();
      expect(result.category).toBeNull();
      expect(result.elapsedMs).toBeGreaterThanOrEqual(0);
      expect(result.elapsedMs).toBeLessThanOrEqual(10000);
      expect(JSON.stringify(result)).not.toContain("PRIVATE");
    }
  });

  test("unknown, malformed, oversize and injected lines never leak", () => {
    const remote =
      'Error: "remote normal startup refused (REMOTE_SCHEMA_GATE_REFUSED, gate GATE_VALIDATION_FAILED, settlement no-write-opened): remote startup gate refused: schema validation failed"\n';
    const encoder = new TextEncoder();
    const raws: (string | Uint8Array)[] = [
      "PRIVATE_TOKEN libsql://private.invalid/private\n",
      START_MISSING + "PRIVATE_TOKEN\n",
      START_MISSING.slice(0, -1),
      START_MISSING + "x".repeat(START_DIAGNOSTIC_INPUT_CAP),
      START_MISSING.replaceAll(" is not set", " PRIVATE_TOKEN is not set"),
      START_MISSING.replaceAll("\n", "\v"),
      new Uint8Array([...encoder.encode(START_MISSING), 0xff, 0x0a]),
      remote.replace("GATE_VALIDATION_FAILED", "PRIVATE_TOKEN"),
      remote.replace("no-write-opened", "commit-unknown"),
      remote.replace("schema validation failed", "schema validation failed PRIVATE_TOKEN"),
      remote + START_MISSING,
    ];
    for (const raw of raws) {
      const result = record(raw);
      expect(result.diagnosticStatus).toBe("unqualified");
      expect(result.phase).toBeNull();
      expect(result.category).toBeNull();
      expect(JSON.stringify(result)).not.toContain("PRIVATE");
      expect(JSON.stringify(result)).not.toContain("libsql://");
    }
  });

  test("closed remote codes, gates, settlements and the full display", () => {
    const samples = [
      [
        "REMOTE_CONNECT_REFUSED",
        "GATE_ENDPOINT_SHAPE",
        "no-write-opened",
        "remote libSQL primary connect refused (TLS endpoint and token required)",
        "remote-connect-refused",
      ],
      [
        "REMOTE_SCHEMA_GATE_REFUSED",
        "GATE_VALIDATION_FAILED",
        "no-write-opened",
        "remote startup gate refused: schema validation failed",
        "remote-schema-gate-refused",
      ],
      [
        "REMOTE_SCHEMA_GATE_REFUSED",
        "GATE_CATALOG_DIFFERS",
        "writes-may-have-committed",
        "remote schema gate refused after migration steps committed: SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused",
        "remote-schema-gate-refused",
      ],
      [
        "REMOTE_MIGRATION_STEP_FAILED",
        "none",
        "rollback-confirmation-withheld",
        "remote migration step 12 failed; rollback confirmation withheld: driver error withheld",
        "remote-migration-step-failed",
      ],
      [
        "REMOTE_MIGRATION_STEP_FAILED",
        "none",
        "cleanup-unconfirmed",
        "remote migration step 12 failed; cleanup unconfirmed, admission quarantined: driver error withheld",
        "remote-migration-step-failed",
      ],
      [
        "REMOTE_MIGRATION_COMMIT_UNKNOWN",
        "none",
        "commit-unknown",
        "remote migration step 12 commit outcome is unknown; settlement receipt retained; rerun resumes from the ledger",
        "remote-migration-commit-unknown",
      ],
      [
        "REMOTE_MIGRATION_CANCELLED",
        "none",
        "cancel-checkpoint-settled",
        "remote migration cancelled after 12 settled step(s)",
        "remote-migration-cancelled",
      ],
      [
        "REMOTE_DRAIN_FAILED",
        "none",
        "drain-failed",
        "remote stream drain failed at close",
        "remote-drain-failed",
      ],
      [
        "REMOTE_SCHEMA_GATE_REFUSED",
        "none",
        "drain-failed",
        "remote startup gate refused: driver error withheld; remote stream drain failed at close",
        "remote-schema-gate-refused",
      ],
    ] as const;
    for (const [code, gate, settlement, display, category] of samples)
      for (const phase of ["prepare-remote", "server-remote"]) {
        const tuple =
          "(" + code + ", gate " + gate + ", settlement " + settlement + "): " + display;
        const line =
          phase === "prepare-remote"
            ? "fvoci: preparation failed; the server does not start: remote preparation refused " +
              tuple
            : 'Error: "remote normal startup refused ' + tuple + '"';
        const result = record(line + "\n");
        expect([result.diagnosticStatus, result.phase, result.category]).toEqual([
          "qualified",
          phase,
          category,
        ]);
        expect(record(line + " PRIVATE_TOKEN\n").diagnosticStatus).toBe("unqualified");
      }
    for (const [gate, texts] of Object.entries(START_GATE_TEXTS))
      for (const item of texts) {
        const line =
          'Error: "remote normal startup refused (REMOTE_SCHEMA_GATE_REFUSED, gate ' +
          gate +
          ", settlement no-write-opened): remote startup gate refused: " +
          item +
          '"\n';
        expect(record(line).category).toBe("remote-schema-gate-refused");
      }
    for (const value of ["2147483648", "-2147483649", "0012", "-0"]) {
      const line =
        "fvoci: preparation failed; the server does not start: remote preparation refused (REMOTE_MIGRATION_STEP_FAILED, gate none, settlement rollback-confirmation-withheld): remote migration step " +
        value +
        " failed; rollback confirmation withheld: driver error withheld\n";
      expect(record(line).diagnosticStatus).toBe("unqualified");
    }
    const cancelled =
      "fvoci: preparation failed; the server does not start: remote preparation refused (REMOTE_MIGRATION_CANCELLED, gate none, settlement cancel-checkpoint-settled): remote migration cancelled after 18446744073709551616 settled step(s)\n";
    expect(record(cancelled).diagnosticStatus).toBe("unqualified");
  });

  test("real full formatter milestones identify only a completed interval at a live deadline", () => {
    for (const ansi of [false, true]) {
      const lines = progressLines(ansi);
      for (const [index, phase] of [
        "prepared",
        "main-gate-complete",
        "attachment-transfer-loaded",
        "identity-settings-loaded",
      ].entries()) {
        // Other ordinary startup messages do not manufacture progress or a cause.
        const result = record(
          lines.slice(0, index + 1).join("\n") + "\nunrelated startup observation\n",
          null,
          110,
          100,
          110,
          "on",
        );
        expect(result).toEqual({
          originalFailure: "UI_SERVER_START_FAILED",
          diagnosticStatus: "unqualified",
          processState: "deadline",
          exitCode: null,
          elapsedMs: 10000,
          allocation: "on",
          phase,
          category: null,
        });
      }
      expect(record(lines.join("\r\n") + "\r\n").phase).toBe("identity-settings-loaded");
    }
  });

  test("closed transfer and identity fields accept actual producer values only", () => {
    const lines = progressLines();
    const transfer = must(lines[2]),
      identity = must(lines[3]);
    expect(record(transfer.replace('"proxy"', '"presigned"') + "\n").phase).toBe(
      "attachment-transfer-loaded",
    );
    expect(
      record(identity.replace("true", "false").replace("=2", "=18446744073709551615") + "\n").phase,
    ).toBe("identity-settings-loaded");
    for (const wrong of [
      transfer.replace('"proxy"', '"PRIVATE_TOKEN"'),
      transfer.replace('"proxy"', "proxy"),
      transfer.replace("mode=", "unknown="),
      identity.replace("true", "1"),
      identity.replace("=2", "=02"),
      identity.replace("=2", "=-1"),
      identity.replace("=2", "=18446744073709551616"),
      identity + " unknown=PRIVATE_TOKEN",
    ]) {
      const result = record(must(lines[0]) + "\n" + wrong + "\n");
      expect(result.phase).toBeNull();
      expect(JSON.stringify(result)).not.toContain("PRIVATE");
    }
  });

  test("foreign prefixes, controls, truncation, cap and inconsistent order stay unknown", () => {
    const lines = progressLines(),
      marker = must(lines[1]);
    for (const raw of [
      "no marker\n",
      marker.replace("fvoci_server", "foreign_server") + "\n",
      marker.replace(" INFO", " WARN") + "\n",
      marker.replace("2026-10-17T", "PRIVATE_TOKEN ") + "\n",
      marker.replace("T00:", "T99:") + "\n",
      "PRIVATE_TOKEN " + marker + "\n",
      marker + " PRIVATE_TOKEN\n",
      "\x1b[2J" + marker + "\n",
      marker.replace(" INFO", "\x1b[32m INFO\x1b[0m") + "\n",
      marker.replace("disabled", "dis\tabled") + "\n",
      marker + "\x7f\n",
      marker + "\x00\n",
      lines.join("\n"),
      lines.join("\n") + "\nx".repeat(START_DIAGNOSTIC_INPUT_CAP),
      [lines[1], lines[0]].join("\n") + "\n",
      [lines[0], lines[0]].join("\n") + "\n",
    ]) {
      const result = record(raw, null, 110);
      expect(result.phase).toBeNull();
      expect(result.category).toBeNull();
      expect(result.diagnosticStatus).toBe("unqualified");
      expect(JSON.stringify(result)).not.toContain("PRIVATE");
      expect(JSON.stringify(result)).not.toContain("\x1b");
    }
    expect(
      record("x".repeat(START_DIAGNOSTIC_INPUT_CAP - marker.length - 2) + "\n" + marker + "\n")
        .phase,
    ).toBe("main-gate-complete");
  });

  test("a completed closed error takes precedence over a progress interval", () => {
    const error =
      'Error: "remote normal startup refused (REMOTE_DRAIN_FAILED, gate none, settlement drain-failed): remote stream drain failed at close"\n';
    const result = record(must(progressLines()[0]) + "\n" + error);
    expect([result.diagnosticStatus, result.phase, result.category]).toEqual([
      "qualified",
      "server-remote",
      "remote-drain-failed",
    ]);
    expect(
      record(must(progressLines()[0]) + "\n" + error + "PRIVATE_TOKEN\n").diagnosticStatus,
    ).toBe("unqualified");
  });

  test("allocation is source-owned and never inferred from log or arbitrary caller text", () => {
    for (const allocation of [
      "off",
      "restart",
      "auth-on",
      "restore-destination-restart",
      "products-firefox-off",
      "boundary-webkit-on",
      "keycloak-prepare-wrong-secret",
    ] as const)
      expect(record("", null, 110, 100, 110, allocation).allocation).toBe(allocation);
    for (const invalid of [
      "PRIVATE_TOKEN",
      "on\n",
      { toString: () => "on", token: "PRIVATE_TOKEN" },
    ]) {
      const result = record("", null, 110, 100, 110, invalid as StartAllocation);
      expect(result.allocation).toBe("unknown");
      expect(JSON.stringify(result)).not.toContain("PRIVATE");
    }
  });
});

// tracing-subscriber full format: fixed source target, message and ordered fields.
function progressLines(ansi = false): string[] {
  const prefix = ansi
    ? "\x1b[2m2026-10-17T00:00:00.123456Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mfvoci_server\x1b[0m\x1b[2m:\x1b[0m "
    : "2026-10-17T00:00:00.123456Z  INFO fvoci_server: ";
  const field = (name: string) => (ansi ? "\x1b[3m" + name + "\x1b[0m\x1b[2m=\x1b[0m" : name + "=");
  return [
    "fvoci: prepared; starting the server",
    prefix + "meilisearch disabled (FVOCI_MEILI_URL unset)",
    prefix + "attachment transfer mode " + field("mode") + '"proxy"',
    prefix +
      "identity settings loaded " +
      field("encryption_keys") +
      "true " +
      field("oidc_providers") +
      "2",
  ];
}

// The scope's spawn writes the producer's lines into the child's log fd.
const writingSpawn =
  (events: string[]) => (_args: string[], _label: string, options: { stdout?: unknown }) => {
    writeSync(options.stdout as number, START_MISSING);
    events.push("spawn");
    return exitedChild(2);
  };

describe("start failure ordering", () => {
  test("a live deadline publishes its completed interval before a failing cleanup writes more", async () => {
    const manifest = {
      binaries: { "fvoci-migrate": { path: "/never-executed" } },
    } as unknown as Manifest;
    const events: string[] = [],
      output = new Captured();
    let fd = -1;
    const child = exitedChild(0);
    Object.assign(child, { exitCode: null });
    const { scope } = fakeScope({
      spawn: (_args, _label, options) => {
        fd = options.stdout as number;
        writeSync(fd, progressLines().slice(0, 2).join("\n") + "\n");
        return child;
      },
      output: {
        out: (line) => {
          events.push("diagnostic");
          output.out(line);
        },
        err: (line) => {
          output.err(line);
        },
      },
      finish: () => {
        events.push("cleanup");
        writeSync(fd, must(progressLines()[3]) + "\n");
        throw new UiError("UI_PROCESS_CLOSURE_FAILED");
      },
    });
    let ticks = 0;
    let caught: unknown;
    try {
      await start(scope, manifest, {}, directory, false, () => (ticks++ === 0 ? 100 : 110), "off");
    } catch (error) {
      caught = error;
    }
    expect((caught as Error).message).toBe("UI_SERVER_START_FAILED");
    expect(events).toEqual(["diagnostic", "cleanup"]);
    const published = JSON.parse(must(output.stdout[0])) as Record<string, unknown>;
    expect([
      published.allocation,
      published.processState,
      published.phase,
      published.diagnosticStatus,
      published.category,
      published.elapsedMs,
    ]).toEqual(["off", "deadline", "main-gate-complete", "unqualified", null, 10000]);
    const receipt = JSON.parse(
      readFileSync(join(directory, "start-failure.private.json"), "utf8"),
    ) as Record<string, unknown>;
    expect(receipt.startDiagnostic).toEqual(published);
    expect(receipt.cleanupErrors).toEqual(["UI_PROCESS_CLOSURE_FAILED"]);
    expect(readFileSync(join(directory, "server.private.log"), "utf8")).toContain(
      "identity settings loaded",
    );
  });

  test("hosted: the diagnostic precedes cleanup and survives cleanup, receipt and stdout faults", async () => {
    const manifest = {
      binaries: { "fvoci-migrate": { path: "/never-executed" } },
    } as unknown as Manifest;
    for (const fault of ["none", "finish", "receipt", "stdout", "all"]) {
      const run = mkdtempSync(join(directory, "run-"));
      const events: string[] = [];
      const output = new Captured();
      output.failOut = fault === "stdout" || fault === "all";
      const { scope } = fakeScope({
        spawn: writingSpawn([]),
        output: {
          out: (line) => {
            events.push("diagnostic");
            output.out(line);
          },
          err: (line) => {
            output.err(line);
          },
        },
        finish: () => {
          events.push("cleanup");
          if (fault === "finish" || fault === "all") throw new UiError("UI_PROCESS_CLOSURE_FAILED");
          return 0;
        },
        write: (path, value) => {
          events.push("receipt");
          if (fault === "receipt" || fault === "all") throw new Error("PRIVATE_RECEIPT");
          write(path, value);
        },
      });
      let caught: unknown;
      try {
        await start(scope, manifest, {}, run, false, () => 100, "restore-source");
      } catch (error) {
        caught = error;
      }
      expect((caught as Error).message).toBe("UI_SERVER_START_FAILED");
      expect(events).toEqual(["diagnostic", "cleanup", "receipt"]);
      if (!output.failOut) {
        const published = JSON.parse(output.stdout[0] as string) as Record<string, unknown>;
        expect([published.exitCode, published.category]).toEqual([2, "missing-encryption-keyring"]);
        expect(published.allocation).toBe("restore-source");
        expect(Buffer.byteLength(output.stdout.join("\n"))).toBeLessThanOrEqual(2048);
      }
      if (fault !== "receipt" && fault !== "all") {
        const receipt = JSON.parse(
          readFileSync(join(run, "start-failure.private.json"), "utf8"),
        ) as Record<string, Record<string, unknown>>;
        expect(must(receipt.startDiagnostic).exitCode).toBe(2);
        expect(receipt.originalFailure as unknown).toBe("UI_SERVER_START_FAILED");
      }
      expect(output.all).not.toContain("PRIVATE");
    }
  });

  test("local: the diagnostic is published before owned cleanup and the receipt is kept", async () => {
    for (const failing of [false, true]) {
      const run = mkdtempSync(join(directory, "local-"));
      const events: string[] = [];
      const output = new Captured();
      const { scope } = fakeScope({
        spawn: writingSpawn([]),
        output: {
          out: (line) => {
            events.push("diagnostic");
            output.out(line);
          },
          err: (line) => {
            output.err(line);
          },
        },
      });
      const steps: LocalStartSteps = {
        publish: () =>
          Promise.resolve([
            "a".repeat(64),
            { qualification: BLOCKED, cgroupCaps: "not-observed" } as never,
            {},
          ]),
        release: () =>
          Promise.resolve([
            { maintenance: {}, retirement: {} } as never,
            null as unknown as number,
          ]),
        cleanup: () => {
          events.push("cleanup");
          return Promise.resolve(failing ? ["UI_PROCESS_CLOSURE_FAILED"] : []);
        },
        clock: () => 100,
      };
      let caught: unknown;
      try {
        await localServerStart(scope, { binaries: {} }, {}, run, false, steps);
      } catch (error) {
        caught = error;
      }
      expect((caught as Error).message).toBe("UI_SERVER_START_FAILED");
      expect(events).toEqual(["diagnostic", "cleanup"]);
      const receipt = JSON.parse(
        readFileSync(join(run, "start-failure.private.json"), "utf8"),
      ) as Record<string, unknown>;
      expect(receipt.startDiagnostic).toEqual(JSON.parse(output.stdout[0] as string));
      expect((receipt.startDiagnostic as Record<string, unknown>).exitCode).toBe(2);
      expect(receipt.cleanupErrors).toEqual(failing ? ["UI_PROCESS_CLOSURE_FAILED"] : []);
      expect(output.all).not.toContain("PRIVATE");
    }
  });
});
