import { describe, expect, test, spyOn } from "bun:test";
import * as fs from "node:fs";
import {
  openSync,
  closeSync,
  renameSync,
  symlinkSync,
  readFileSync,
  mkdtempSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  finalRunIdentity,
  finalRuntimeImage,
  recheckFinalBrowsers,
  sealFinalBrowsers,
  finalTrustFile,
  recheckFinalTrust,
} from "./ui-record.ts";
import { image as fixtureImage } from "../selected-backend-ci/drivers/sqlite.ts";
import { containerArgv } from "./ui-container.ts";
import { digest } from "../selected-backend-ci/io.ts";
import { admitSerialBaseline, type SerialExecution } from "./ui-flow.ts";
import {
  BACKGROUND_TABLES,
  validateSerialObservation,
  assertPreserved,
  type Audit,
  type SerialPolicy,
  assertRoomClosure,
  roomRetirement,
  captureRoomLog,
} from "./ui-audit.ts";
import { get, record, type Record_ } from "./ui-common.ts";
import { preparedKeycloakAppEnvironment } from "./ui-final.ts";
import type { Scope, Child } from "./ui-processes.ts";

const policy: SerialPolicy = { revisionKeep: 200, revisionSnapshotIntervalHours: 24 };
const n = (value: bigint | number) => ["integer", String(value)];
const hash = (value: unknown) => digest(JSON.stringify(value));

/** Actual native DTO shape; the contract inventory is checked against native source. */
function observation(initial = false): Record_ {
  const operations: Record<string, unknown[]> = {
    event_sequence: [[n(1), n(0)]],
    collab_fence_counter: [[n(1), n(1)]],
    maintenance_job_claims: Array.from({ length: 9 }, (_, i) => [
      n(i + 1),
      ["null"],
      n(0),
      ["null"],
    ]),
    collab_room_fences: [],
    task_collab_room_fences: [],
    events: [],
    users: [],
    workspaces: [],
    outbox_consumers: [],
  };
  const outbox = (initial ? [] : ["github", "mail", "notifications", "push", "webhooks"]).map(
    (consumer) => {
      const row = [["text", consumer], n(0), ["null"], ["null"], n(1000)];
      (operations.outbox_consumers as unknown[]).push(row.slice(0, 4));
      return {
        consumer,
        immutableSha256: hash([["text", consumer]]),
        rowSha256: hash(row),
        lastSeq: "0",
        leaseOwner: null,
        leaseUntilMicros: null,
        updatedAtMicros: "1000",
      };
    },
  );
  const prints: Record<string, Record<string, number>> = {};
  for (const [table, rows] of Object.entries(operations)) {
    prints[table] = Object.fromEntries(rows.map((row) => [hash(row), 1]));
  }
  prints.memberships = {};
  for (const table of BACKGROUND_TABLES) prints[table] ??= {};
  prints.outbox_consumers = Object.fromEntries(outbox.map((r) => [r.rowSha256, 1]));
  const allocations = Object.fromEntries(
    Object.entries(prints).map(([table, rows]) => [
      table,
      Object.fromEntries(Object.keys(rows).map((key) => [key, {}])),
    ]),
  );
  const native = readFileSync(new URL("../../src/db/e2e_fixture.rs", import.meta.url), "utf8");
  const fixed = native.slice(
    native.indexOf("const SERIAL_BACKGROUND_READS:"),
    native.indexOf("async fn serial_checks("),
  );
  const kinds = [...fixed.matchAll(/\("([a-z-]+)",/g)].map((match) => match[1]);
  const start = native.indexOf("async fn serial_checks(");
  const rooms = native.slice(start, native.indexOf("let mut background", start));
  kinds.push(...[...rooms.matchAll(/Some\("([a-z-]+)"\)/g)].map((match) => match[1]));
  kinds.push("automatic-revision-retention");
  expect(kinds).toHaveLength(26);
  return {
    dataset: {
      schema: 1,
      backend: "libsql-remote",
      schemaCurrent: true,
      lineage: "fvoci-sqlite-060",
      ledger: Array.from({ length: 12 }, (_, i) => [n(i + 1)]),
      schemaSha256: "a".repeat(64),
      startupHazards: 0,
      liveOutboxLeases: 0,
      operations,
      fingerprints: prints,
      allocations,
      rows: Object.values(prints).reduce(
        (sum, rows) => sum + Object.values(rows).reduce((a, b) => a + b, 0),
        0,
      ),
    },
    checks: {
      schema: 1,
      clockMicros: "1000000",
      coverageMicros: "2400000000",
      horizonEndMicros: "2401000000",
      policy,
      policySha256: hash(policy),
      policyBinding: "required-from-sealed-server",
      roomClosureBinding: "required-normal-release-and-parent-retirement",
      eventHead: "0",
      outbox,
      relayCompletion: initial ? "unknown-registry" : "complete",
      users: [],
      memberships: [],
      owners: Object.fromEntries(
        ["maintenance_job_claims", "collab_room_fences", "task_collab_room_fences"].map((table) => [
          table,
          operations[table],
        ]),
      ),
      background: kinds.map((kind) => ({
        kind,
        interpretation: "eligible",
        eligibility: "clear",
        now: 0,
        throughHorizon: 0,
      })),
      revisionCandidates: [],
      automaticRevisionRanks: [],
    },
  };
}

function restartPair(): { before: Record_; after: Record_; audit: Audit } {
  const before = observation(),
    after = structuredClone(before);
  const claims = get(after, "dataset", "operations", "maintenance_job_claims") as unknown[][];
  for (const row of claims) if ([1, 8, 9].includes(Number(get(row, 0, 1)))) row[2] = n(1);
  record(after, "dataset", "fingerprints").maintenance_job_claims = Object.fromEntries(
    claims.map((row) => [hash(row), 1]),
  );
  record(after, "dataset", "allocations").maintenance_job_claims = Object.fromEntries(
    claims.map((row) => [hash(row), {}]),
  );
  const identity = { pid: 123, startTicks: "456" },
    targetSha256 = "a".repeat(64);
  const audit: Audit = {
    namespaces: [],
    actors: {},
    workspaces: {},
    documents: {},
    serverStarts: 1,
    targetSha256,
    observedFences: [],
    servers: [
      {
        identity,
        targetSha256,
        receipts: [8, 9, 1].flatMap((key) =>
          ["prepared", "acquired", "released"].map((outcome) => ({
            schema: 1,
            pid: identity.pid,
            key,
            ownerSha256: String(key).padEnd(64, "a"),
            outcome,
            generation: outcome === "prepared" ? null : "1",
          })),
        ),
      },
    ],
    serial: {
      before: validateSerialObservation(before, policy),
      after: validateSerialObservation(after, policy),
      policy,
    },
  };
  return { before, after, audit };
}

// Real private files/descriptors; process identity and sealed ELF are synthetic DTO fixtures.
function roomFixture(directory: string, log: string) {
  const identity = { pid: 123, startTicks: "456" },
    child = { pid: 123 } as Child;
  const state = { retired: false },
    allocation = { process: child, label: "server", closed: false, forced: false };
  const scope = {
    closure: () => state.retired,
    allocations: [allocation],
    entries: new Map([["123:456", { identity, allocation: 0 }]]),
    retired: () => state.retired,
  } as unknown as Scope;
  const elf = join(directory, "fvoci-server");
  writeFileSync(elf, "synthetic-ELF-bytes");
  const manifest = {
    sourceInputs: { source: "a".repeat(40), tree: "b".repeat(40), files: {} },
    binaries: {
      "fvoci-server": { path: elf, sha256: digest(readFileSync(elf)), features: [], profile: {} },
    },
  };
  const fd = openSync(log, "r+");
  const origin = captureRoomLog(scope, manifest, log, fd, child, identity, "d".repeat(64));
  allocation.closed = true;
  state.retired = true;
  return { scope, manifest, fd, origin, state };
}

describe("final native serial admission", () => {
  test("room history refuses substituted valid inode/symlink, closed FD and foreign origins", () => {
    for (const mutation of [
      "inode",
      "symlink",
      "closed",
      "source",
      "tree",
      "elf",
      "target",
      "pid",
      "ticks",
    ] as const) {
      const directory = mkdtempSync(join(tmpdir(), "room-origin-")),
        log = join(directory, "server.private.log");
      writeFileSync(log, "actual producer\n", { mode: 0o600 });
      const f = roomFixture(directory, log);
      let closed = false;
      try {
        const other = join(directory, "other.private.log");
        writeFileSync(
          other,
          "FVOCI_ROOM_RELEASE " +
            JSON.stringify({
              schema: 1,
              kind: "document",
              workspaceId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
              targetId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
              ownerSha256: "c".repeat(64),
              fence: "1",
              expiresAtMicros: "1000",
              pid: 123,
              commit: "confirmed",
            }),
          { mode: 0o600 },
        );
        if (mutation === "inode" || mutation === "symlink") {
          renameSync(log, join(directory, "actual.private.log"));
          if (mutation === "inode") renameSync(other, log);
          else symlinkSync(other, log);
        } else if (mutation === "closed") {
          closeSync(f.fd);
          closed = true;
        } else if (mutation === "source") f.origin.source = "e".repeat(40);
        else if (mutation === "tree") f.origin.tree = "e".repeat(40);
        else if (mutation === "elf") writeFileSync(f.origin.executable.path, "changed-ELF");
        else if (mutation === "target") f.origin.targetSha256 = "e".repeat(64);
        else if (mutation === "pid") f.origin.retiredIdentity = { pid: 124, startTicks: "456" };
        else f.origin.retiredIdentity = { pid: 123, startTicks: "457" };
        expect(
          () => roomRetirement(f.scope, f.manifest, f.origin, "d".repeat(64)),
          mutation,
        ).toThrow();
      } finally {
        if (!closed) closeSync(f.fd);
        rmSync(directory, { recursive: true, force: true });
      }
    }
  });
  test("bounded actual-FD read preserves producer offset and rejects mutation during reading", () => {
    const directory = mkdtempSync(join(tmpdir(), "room-read-")),
      log = join(directory, "server.private.log");
    writeFileSync(log, "producer bytes\n", { mode: 0o600 });
    const f = roomFixture(directory, log);
    try {
      const byte = Buffer.alloc(1);
      fs.readSync(f.fd, byte, 0, 1, null);
      expect(roomRetirement(f.scope, f.manifest, f.origin, "d".repeat(64)).receipts).toEqual([]);
      fs.readSync(f.fd, byte, 0, 1, null);
      expect(byte.toString()).toBe("r");
      const actualRead = fs.readSync;
      let changed = false;
      const intercepted = spyOn(fs, "readSync").mockImplementation(((
        fd: number,
        bytes: Uint8Array,
        offset: number,
        length: number,
        position: number | null,
      ) => {
        const count = actualRead(fd, bytes, offset, length, position);
        if (!changed && count > 0 && fs.fstatSync(fd).ino === f.origin.log.ino) {
          changed = true;
          writeFileSync(log, "changed bytes!\n");
        }
        return count;
      }) as typeof fs.readSync);
      try {
        expect(() => roomRetirement(f.scope, f.manifest, f.origin, "d".repeat(64))).toThrow(
          "UI_SERIAL_ROOM_LOG_CHANGED",
        );
      } finally {
        intercepted.mockRestore();
      }
      expect(changed).toBe(true);
    } finally {
      closeSync(f.fd);
      rmSync(directory, { recursive: true, force: true });
    }
  });
  test("prepared app excludes fixture authority and exact system trust is sealed", () => {
    const parent = {
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true",
      FVOCI_TEST_TURSO_DESTRUCTIVE: "true",
      FVOCI_LIBSQL_URL: "synthetic-private",
      SSL_CERT_FILE: "/etc/ssl/certs/ca-certificates.crt",
    };
    expect(preparedKeycloakAppEnvironment(parent)).toEqual({
      FVOCI_LIBSQL_URL: parent.FVOCI_LIBSQL_URL,
      SSL_CERT_FILE: parent.SSL_CERT_FILE,
    });
    expect(parent.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE).toBe("true");
    const trust = finalTrustFile();
    expect(() => {
      recheckFinalTrust(trust);
    }).not.toThrow();
    for (const changed of [
      { ...trust, sha256: "e".repeat(64) },
      { ...trust, path: "/foreign" },
      { ...trust, uid: 1 },
    ])
      expect(() => {
        recheckFinalTrust(changed);
      }).toThrow("UI_FINAL_TRUST_CHANGED");
  });
  test("expired retained rooms require exact confirmed native release and parent retirement", () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-final-room-"));
    let logFd: number | undefined;
    try {
      const log = join(directory, "server.private.log");
      const receipt = {
        schema: 1,
        kind: "document",
        workspaceId:
          "a".repeat(8) +
          "-" +
          "a".repeat(4) +
          "-" +
          "a".repeat(4) +
          "-" +
          "a".repeat(4) +
          "-" +
          "a".repeat(12),
        targetId:
          "b".repeat(8) +
          "-" +
          "b".repeat(4) +
          "-" +
          "b".repeat(4) +
          "-" +
          "b".repeat(4) +
          "-" +
          "b".repeat(12),
        ownerSha256: digest(Buffer.from("c".repeat(32), "hex")),
        fence: "1",
        expiresAtMicros: "1000000",
        pid: 123,
        commit: "confirmed",
      };
      writeFileSync(log, "FVOCI_ROOM_RELEASE " + JSON.stringify(receipt) + "\n", { mode: 0o600 });
      const fixture = roomFixture(directory, log);
      const { scope, manifest, origin, state } = fixture;
      logFd = fixture.fd;
      const witness = roomRetirement(scope, manifest, origin, "d".repeat(64));
      const value = observation();
      const rows = [
        [
          ["blob", "a".repeat(32)],
          ["blob", "b".repeat(32)],
          ["blob", "c".repeat(32)],
          n(1),
          n(1000000),
        ],
      ];
      record(value, "dataset", "operations").collab_room_fences = rows;
      record(value, "checks", "owners").collab_room_fences = rows;
      record(value, "dataset", "fingerprints").collab_room_fences = { [hash(rows[0])]: 1 };
      record(value, "dataset", "allocations").collab_room_fences = { [hash(rows[0])]: {} };
      record(value, "dataset").rows = Number(get(value, "dataset", "rows")) + 1;
      const observed = validateSerialObservation(value, policy);
      expect(() => {
        assertRoomClosure(observed, "d".repeat(64));
      }).toThrow("UI_SERIAL_ROOM_RELEASE_UNWITNESSED");
      observed.roomHistory = [witness];
      expect(() => {
        assertRoomClosure(observed, "d".repeat(64));
      }).not.toThrow();
      for (const field of [
        "ownerSha256",
        "fence",
        "expiresAtMicros",
        "targetId",
        "workspaceId",
        "pid",
        "commit",
      ]) {
        const changed = structuredClone(observed);
        Reflect.set(
          record(get(changed, "roomHistory", 0, "receipts", 0)),
          field,
          field === "pid" ? 124 : field === "fence" || field === "expiresAtMicros" ? "2" : "wrong",
        );
        expect(() => {
          assertRoomClosure(changed, "d".repeat(64));
        }).toThrow();
      }
      expect(() => {
        assertRoomClosure(observed, "e".repeat(64));
      }).toThrow("UI_SERIAL_ROOM_HISTORY_REFUSED");
      // Two real successful normal commits may release the same owner/fence.
      writeFileSync(
        log,
        [{ ...receipt, expiresAtMicros: "999000" }, receipt]
          .map((item) => "FVOCI_ROOM_RELEASE " + JSON.stringify(item))
          .join("\n"),
      );
      observed.roomHistory = [roomRetirement(scope, manifest, origin, "d".repeat(64))];
      expect(() => {
        assertRoomClosure(observed, "d".repeat(64));
      }).not.toThrow();
      state.retired = false;
      expect(() => roomRetirement(scope, manifest, origin, "d".repeat(64))).toThrow(
        "UI_SERIAL_ROOM_RETIREMENT_REQUIRED",
      );
      state.retired = true;
      writeFileSync(log, "FVOCI_ROOM_RELEASE " + JSON.stringify({ ...receipt, pid: 124 }));
      expect(() => roomRetirement(scope, manifest, origin, "d".repeat(64))).toThrow(
        "UI_SERIAL_ROOM_RECEIPT_REFUSED",
      );
      writeFileSync(log, "FVOCI_ROOM_RELEASE " + JSON.stringify({ ...receipt, extra: true }));
      expect(() => roomRetirement(scope, manifest, origin, "d".repeat(64))).toThrow(
        "UI_SERIAL_ROOM_RECEIPT_REFUSED",
      );
      record(value, "checks").roomClosureBinding = "expired-only";
      expect(() => validateSerialObservation(value, policy)).toThrow(
        "UI_SERIAL_ROOM_CLOSURE_BINDING_REQUIRED",
      );
    } finally {
      if (logFd !== undefined) closeSync(logFd);
      rmSync(directory, { recursive: true, force: true });
    }
  });
  test("hosted image metadata and exact nonroot identity are bound without a local grant", () => {
    const item = {
      Id: "sha256:" + "a".repeat(64),
      Architecture: process.arch === "x64" ? "amd64" : process.arch,
      Os: "linux",
      RepoDigests: ["ubuntu@" + fixtureImage.slice(fixtureImage.indexOf("@") + 1)],
      Config: { Env: ["PATH=/usr/bin:/bin"] },
    };
    const runtime = finalRuntimeImage([item]);
    const args = containerArgv(
      "owned-final",
      "/launcher",
      "/capsule",
      "/fvoci-current/bin/fvoci-migrate",
      [],
      ["--start"],
      { finalRuntime: runtime },
    );
    expect(args[args.indexOf("--user") + 1]).toBe(
      String(process.getuid?.()) + ":" + String(process.getgid?.()),
    );
    expect(args).toContain(runtime.imageId);
    expect(() => finalRuntimeImage([{ ...item, Architecture: "wrong" }])).toThrow(
      "UI_FINAL_RUNTIME_IMAGE_REFUSED",
    );
    expect(() =>
      finalRuntimeImage([{ ...item, Config: { Env: ["DATABASE_PASSWORD=PRIVATE_CANARY"] } }]),
    ).toThrow("UI_FINAL_RUNTIME_IMAGE_ENV_REFUSED");
    expect(() =>
      containerArgv(
        "owned-final",
        "/launcher",
        "/capsule",
        "/fvoci-current/bin/fvoci-migrate",
        [],
        ["--start"],
        { finalRuntime: { ...runtime, uid: 0 } },
      ),
    ).toThrow("UI_FINAL_RUNTIME_USER_CHANGED");
  });
  test("retained final audit permits only witnessed maintenance and exact full relay rows", () => {
    const { before, after, audit } = restartPair();
    expect(() => {
      assertPreserved(get(before, "dataset"), get(after, "dataset"), audit);
    }).not.toThrow();
    const changed = structuredClone(audit);
    record(changed.servers[0]).identity = { pid: 124, startTicks: "456" };
    expect(() => {
      assertPreserved(get(before, "dataset"), get(after, "dataset"), changed);
    }).toThrow();
  });
  test("a prior foreign business row is never covered by the serial counter exception", () => {
    const { before, after, audit } = restartPair();
    const foreign = "f".repeat(64);
    for (const v of [before, after]) {
      record(v, "dataset", "fingerprints", "documents")[foreign] = 1;
      record(v, "dataset", "allocations", "documents")[foreign] = {
        workspaces: [],
        actors: [],
        events: [],
      };
      record(v, "dataset").rows = Number(get(v, "dataset", "rows")) + 1;
    }
    audit.serial = {
      before: validateSerialObservation(before, policy),
      after: validateSerialObservation(after, policy),
      policy,
    };
    expect(() => {
      assertPreserved(get(before, "dataset"), get(after, "dataset"), audit);
    }).not.toThrow();
    Reflect.deleteProperty(record(after, "dataset", "fingerprints", "documents"), foreign);
    Reflect.deleteProperty(record(after, "dataset", "allocations", "documents"), foreign);
    record(after, "dataset").rows = Number(get(after, "dataset", "rows")) - 1;
    expect(() => {
      assertPreserved(get(before, "dataset"), get(after, "dataset"), audit);
    }).toThrow("UI_PREEXISTING_ROW_CHANGED");
  });
  test("final freeze binds the real main source, run, attempt and authenticated job", () => {
    const source = { source: "a".repeat(40), tree: "b".repeat(40), files: {} };
    const environment = {
      GITHUB_ACTIONS: "true",
      CI: "true",
      GITHUB_JOB: "turso-ui",
      GITHUB_REPOSITORY: "AISFlow/fvoci",
      GITHUB_REF: "refs/heads/main",
      GITHUB_EVENT_NAME: "workflow_dispatch",
      GITHUB_SHA: source.source,
      GITHUB_RUN_ID: "123",
      GITHUB_RUN_ATTEMPT: "1",
    };
    expect(finalRunIdentity(source, environment)).toMatchObject({
      runId: "123",
      attempt: "1",
      job: "turso-ui",
    });
    for (const [key, value] of Object.entries({
      GITHUB_JOB: "other",
      GITHUB_REF: "refs/heads/topic",
      GITHUB_SHA: "c".repeat(40),
      GITHUB_RUN_ATTEMPT: "0",
      GITHUB_EVENT_NAME: "push",
      GITHUB_REPOSITORY: "fork/fvoci",
    }))
      expect(() => finalRunIdentity(source, { ...environment, [key]: value })).toThrow(
        "UI_FINAL_JOB_BINDING_REQUIRED",
      );
  });
  test("full native horizon and exact empty initial or complete retained registry", () => {
    const first = observation(true),
      retained = observation();
    expect(validateSerialObservation(first, policy, true).dataset).toEqual(
      record(first, "dataset"),
    );
    expect(validateSerialObservation(retained, policy).dataset).toEqual(
      record(retained, "dataset"),
    );
    expect(() => validateSerialObservation(first, policy)).toThrow();
  });
  test("native observations retain integer precision beyond 2^53", () => {
    const value = observation();
    Object.assign(record(value, "checks"), {
      clockMicros: "9007199254740993",
      horizonEndMicros: "9007201654740993",
    });
    expect(() => validateSerialObservation(value, policy)).not.toThrow();
  });
  test("missing, blocked, unknown and shortened native facts fail closed", () => {
    const mutations: Array<(value: Record_) => void> = [
      (v) => {
        record(v, "checks").coverageMicros = "1";
      },
      (v) => {
        record(v, "checks").horizonEndMicros = "2401000001";
      },
      (v) => {
        record(v, "checks").policySha256 = "b".repeat(64);
      },
      (v) => {
        record(v, "checks").background = [];
      },
      (v) => {
        record(get(v, "checks", "background", 0)).eligibility = "unknown";
      },
      (v) => {
        record(get(v, "checks", "background", 0)).throughHorizon = 1;
      },
      (v) => {
        record(v, "checks").revisionCandidates = [{ dueNow: false, dueThroughHorizon: true }];
      },
      (v) => {
        record(v, "dataset").rows = 0;
      },
      (v) => {
        record(v, "dataset").ledger = [];
      },
      (v) => {
        record(v, "checks").owners = {};
      },
    ];
    for (const mutate of mutations) {
      const v = observation();
      mutate(v);
      expect(() => validateSerialObservation(v, policy)).toThrow();
    }
  });
  test("exact full relay row hash, immutable name, null leases and head equality", () => {
    for (const [field, value] of Object.entries({
      lastSeq: "1",
      leaseOwner: "foreign",
      leaseUntilMicros: "1000",
      updatedAtMicros: "1000001",
      rowSha256: "c".repeat(64),
      immutableSha256: "d".repeat(64),
      consumer: "search",
    })) {
      const v = observation();
      record(get(v, "checks", "outbox", 0))[field] = value;
      expect(() => validateSerialObservation(v, policy), field).toThrow();
    }
    const v = observation();
    (get(v, "checks", "outbox") as unknown[]).pop();
    expect(() => validateSerialObservation(v, policy)).toThrow();
  });
  test("counter full row columns and multiplicity are bound independently of fingerprints", () => {
    const v = observation();
    record(v, "dataset", "fingerprints", "event_sequence")["b".repeat(64)] = 1;
    record(v, "dataset", "allocations", "event_sequence")["b".repeat(64)] = {};
    record(v, "dataset").rows = Number(get(v, "dataset", "rows")) + 1;
    expect(() => validateSerialObservation(v, policy)).toThrow(
      "UI_SERIAL_COUNTER_FINGERPRINT_CHANGED",
    );
  });
  test("standalone admission stays fresh-only and serial admission requires the complete predecessor", () => {
    const v = validateSerialObservation(observation(), policy);
    const execution: SerialExecution = { before: v, policy };
    expect(() => {
      admitSerialBaseline(v.dataset);
    }).toThrow("UI_EXISTING_BACKGROUND_WORK_REFUSED");
    expect(() => {
      admitSerialBaseline(v.dataset, execution);
    }).not.toThrow();
    const changed = structuredClone(v.dataset);
    changed.schemaSha256 = "f".repeat(64);
    expect(() => {
      admitSerialBaseline(changed, execution);
    }).toThrow("UI_SERIAL_PREDECESSOR_CHANGED");
  });
  test("each chosen engine and every shared runtime input are sealed", () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-final-browser-seal-"));
    try {
      const paths = {
        chromium: join(directory, "chromium"),
        firefox: join(directory, "firefox"),
        webkit: join(directory, "webkit"),
      };
      for (const [engine, path] of Object.entries(paths)) writeFileSync(path, engine);
      const shared = join(directory, "shared-runtime.so");
      writeFileSync(shared, "runtime");
      const sealed = sealFinalBrowsers(directory, paths);
      expect(() => {
        recheckFinalBrowsers(sealed);
      }).not.toThrow();
      writeFileSync(shared, "changed");
      expect(() => {
        recheckFinalBrowsers(sealed);
      }).toThrow("UI_FINAL_BROWSER_RUNTIME_CHANGED");
      writeFileSync(shared, "runtime");
      writeFileSync(join(directory, "unsealed-input"), "extra");
      expect(() => {
        recheckFinalBrowsers(sealed);
      }).toThrow("UI_FINAL_BROWSER_RUNTIME_CHANGED");
      expect(() => sealFinalBrowsers(directory, { ...paths, webkit: paths.chromium })).toThrow(
        "UI_FINAL_BROWSER_ENGINE_ALIAS_REFUSED",
      );
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
