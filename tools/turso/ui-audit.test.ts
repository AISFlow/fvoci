import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  AUDITED_COUNTERS,
  BACKGROUND_TABLES,
  OFF,
  assertPreserved,
  baselineFailureDiagnostic,
  maintenanceReceipts,
  reportCases,
  startupBlockers,
  type Audit,
} from "./ui-audit.ts";
import { UiError, type Record_ } from "./ui-common.ts";
import { parseJson } from "../selected-backend-ci/io.ts";
import { clone, must, parsed } from "./ui-fakes.ts";

const number = (n: number | string) => ["integer", String(n)];
const namespace = "tui-" + "a".repeat(20);
const workspace = ["blob", "b".repeat(32)],
  actor = ["blob", "c".repeat(32)];

// Mirrors the original counter_records fixture: one owner, one workspace, one
// event, two room fences, two server generations of maintenance receipts.
function counterRecords(): [Record_, Record_, Audit] {
  const original: Record_ = {
    ledger: [],
    schemaSha256: "a".repeat(64),
    lineage: "fvoci-sqlite-060",
    startupHazards: 0,
    liveOutboxLeases: 0,
    fingerprints: Object.fromEntries(BACKGROUND_TABLES.map((t) => [t, {}])),
    operations: {
      event_sequence: [[number(1), number(0)]],
      collab_fence_counter: [[number(1), number(1)]],
      maintenance_job_claims: [1, 2, 3, 4, 5, 6, 7, 8, 9].map((k) => [
        number(k),
        ["null"],
        number(0),
        ["null"],
      ]),
      events: [],
      users: [],
      workspaces: [],
      collab_room_fences: [],
      task_collab_room_fences: [],
      outbox_consumers: [],
    },
  };
  const fingerprints = original.fingerprints as Record<string, Record<string, number>>;
  for (const t of AUDITED_COUNTERS) fingerprints[t] = { ["d".repeat(64)]: 1 };
  fingerprints.users = { ["e".repeat(64)]: 1 };
  const after = clone(original);
  const afterPrints = after.fingerprints as Record<string, Record<string, number>>;
  for (const t of AUDITED_COUNTERS) afterPrints[t] = { ["f".repeat(64)]: 1 };
  const op = after.operations as Record<string, unknown[][]>;
  must((op.event_sequence as unknown[][])[0])[1] = number(1);
  op.events = [[number(1), workspace, actor]];
  op.users = [[actor, ["text", namespace + "-owner@example.invalid"], ["null"]]];
  op.workspaces = [[workspace, ["text", namespace], ["text", "team"]]];
  must((op.collab_fence_counter as unknown[][])[0])[1] = number(3);
  const fence = [
    workspace,
    ["blob", "1".repeat(32)],
    ["blob", "2".repeat(32)],
    number(2),
    number(1000),
  ];
  op.collab_room_fences = [fence];
  for (const row of op.maintenance_job_claims as unknown[][])
    if ([1, 8, 9].includes(Number((row[0] as string[])[1]))) row[2] = number(2);
  op.outbox_consumers = ["notifications", "mail", "push", "webhooks", "github"].map((name) => [
    ["text", name],
    number(1),
    ["null"],
    ["null"],
  ]);
  const firstFence = clone(fence);
  firstFence[3] = number(1);
  fingerprints.workspaces = {};
  afterPrints.workspaces = { ["1".repeat(64)]: 1 };
  must(afterPrints.users)["2".repeat(64)] = 1;
  must(afterPrints.events)["3".repeat(64)] = 1;
  original.allocations = Object.fromEntries(Object.keys(fingerprints).map((t) => [t, {}]));
  after.allocations = Object.fromEntries(Object.keys(afterPrints).map((t) => [t, {}]));
  const refs = (own: string | null = null, spaces: string[] = [], actors: string[] = []) => ({
    workspaces: spaces,
    actors,
    events: [],
    self: own,
    consumer: null,
  });
  const allocations = after.allocations as Record<string, Record<string, unknown>>;
  must(allocations.users)["2".repeat(64)] = refs(actor[1]);
  must(allocations.workspaces)["1".repeat(64)] = refs(workspace[1]);
  must(allocations.events)["3".repeat(64)] = refs(
    "4".repeat(32),
    [must(workspace[1])],
    [must(actor[1])],
  );
  const servers = [1, 2].map((generation) => {
    const identity = { pid: 100 + generation, startTicks: String(200 + generation) };
    const receipts: Record_[] = [];
    for (const key of [8, 9, 1]) {
      const owner = String(generation) + String(key) + "a".repeat(62);
      for (const outcome of ["prepared", "acquired", "released"])
        receipts.push({
          schema: 1,
          pid: identity.pid,
          key,
          ownerSha256: owner,
          generation: outcome === "prepared" ? null : String(generation),
          outcome,
        });
    }
    return { identity, targetSha256: "a".repeat(64), receipts };
  });
  const audit: Audit = {
    namespaces: [namespace],
    actors: { [must(actor[1])]: namespace + "-owner@example.invalid" },
    workspaces: { [must(workspace[1])]: namespace },
    targetSha256: "a".repeat(64),
    servers,
    serverStarts: 2,
    observedFences: [firstFence, fence],
  };
  return [original, after, audit];
}
const ops = (value: Record_) => value.operations as Record<string, unknown[][]>;

describe("preservation audit", () => {
  test("preexisting exact rows and ledger cannot be relaxed to counts", () => {
    const original = {
      ledger: [["integer", "9007199254740993"]],
      schemaSha256: "a".repeat(64),
      lineage: "fvoci-sqlite-060",
      fingerprints: { users: { ["b".repeat(64)]: 1 } },
    };
    const added = clone(original);
    (added.fingerprints.users as Record<string, number>)["c".repeat(64)] = 1;
    assertPreserved(original, added);
    for (const mutated of [
      { ...added, ledger: [["integer", "9007199254740992"]] },
      { ...added, schemaSha256: "d".repeat(64) },
      { ...added, fingerprints: { users: { ["c".repeat(64)]: 2 } } },
      { ...added, fingerprints: { users: { ["b".repeat(64)]: 1 }, extra: {} } },
    ])
      expect(() => {
        assertPreserved(original, mutated);
      }).toThrow(UiError);
  });

  test("only correlated counter deltas preserve existing business rows", () => {
    const [original, after, audit] = counterRecords();
    assertPreserved(original, after, audit);
    const mutations: ((a: Record_) => void)[] = [
      (a) => {
        (a.fingerprints as Record<string, Record_>).users = {};
      },
      (a) => (must(must(ops(a).event_sequence)[0])[1] = number(2)),
      (a) => (must(must(ops(a).events)[0])[2] = ["null"]),
      (a) => (must(must(ops(a).events)[0])[1] = ["blob", "9".repeat(32)]),
      (a) => (must(must(ops(a).collab_fence_counter)[0])[1] = number(4)),
      (a) => (must(must(ops(a).collab_fence_counter)[0])[1] = number("9223372036854775807")),
      (a) => (must(must(ops(a).maintenance_job_claims)[0])[2] = number(3)),
      (a) => (must(must(ops(a).maintenance_job_claims)[1])[2] = number(1)),
      (a) => (must(must(ops(a).maintenance_job_claims)[0])[1] = ["blob", "3".repeat(32)]),
      (a) => must(ops(a).maintenance_job_claims).pop(),
      (a) => (must(must(ops(a).outbox_consumers)[0])[2] = ["blob", "3".repeat(32)]),
      (a) => (a.startupHazards = 1),
    ];
    for (const mutate of mutations) {
      const wrong = clone(after);
      mutate(wrong);
      expect(() => {
        assertPreserved(original, wrong, audit);
      }).toThrow(UiError);
    }
    expect(() => {
      assertPreserved(original, after);
    }).toThrow(UiError);
    expect(() => {
      assertPreserved(original, after, { ...audit, observedFences: [] });
    }).toThrow(UiError);
  });

  test("background preflight keeps populated users but refuses old work", () => {
    const [original] = counterRecords();
    expect(startupBlockers(original)).toEqual([]);
    for (const table of BACKGROUND_TABLES) {
      const wrong = clone(original);
      (wrong.fingerprints as Record<string, Record_>)[table] = { ["f".repeat(64)]: 1 };
      expect(startupBlockers(wrong)).toEqual([table]);
    }
    for (const key of ["startupHazards", "liveOutboxLeases"])
      expect(startupBlockers({ ...original, [key]: 1 })).toContain("existing-owner-or-deletion");
  });

  test("each foreign new actor, workspace, unobserved room and process claim is refused", () => {
    const [before, after, audit] = counterRecords();
    assertPreserved(before, after, audit);
    const defects: Record<string, (a: Record_, proof: Audit) => void> = {
      user: (a) =>
        must(ops(a).users).push([
          ["blob", "9".repeat(32)],
          ["text", "foreign@example.invalid"],
          ["null"],
        ]),
      workspace: (a) =>
        must(ops(a).workspaces).push([
          ["blob", "9".repeat(32)],
          ["text", "foreign"],
          ["text", "team"],
        ]),
      "room-owner": (a) => (must(must(ops(a).collab_room_fences)[0])[2] = ["blob", "9".repeat(32)]),
      "owner-hash": (_, p) =>
        (must((must(p.servers[0]).receipts as Record_[])[1]).ownerSha256 = "9".repeat(64)),
      process: (_, p) => (must((must(p.servers[0]).receipts as Record_[])[1]).pid = 999),
      target: (_, p) => (must(p.servers[0]).targetSha256 = "9".repeat(64)),
      missing: (_, p) => (must(p.servers[0]).receipts as Record_[]).pop(),
      generation: (_, p) => (must((must(p.servers[0]).receipts as Record_[])[1]).generation = "9"),
      key: (_, p) => (must((must(p.servers[0]).receipts as Record_[])[1]).key = 2),
      finish: (_, p) =>
        (must((must(p.servers[0]).receipts as Record_[])[2]).outcome = "release-error"),
    };
    for (const [name, defect] of Object.entries(defects)) {
      const a = clone(after),
        proof = clone(audit);
      defect(a, proof);
      expect(() => {
        assertPreserved(before, a, proof);
      }, name).toThrow(UiError);
    }
  });

  test("malformed shapes refuse instead of reading missing values", () => {
    const [before, after, audit] = counterRecords();
    const wrong = clone(after);
    must(ops(wrong).users)[0] = [];
    expect(() => {
      assertPreserved(before, wrong, audit);
    }).toThrow();
    const missing = clone(after);
    delete (missing.allocations as Record_).users;
    expect(() => {
      assertPreserved(before, missing, audit);
    }).toThrow();
  });
});

describe("maintenance receipts", () => {
  test("only exact receipt records of the server pid are read from the log", () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-ui-log-"));
    try {
      const log = join(directory, "server.private.log");
      const record = {
        schema: 1,
        pid: 7,
        key: 8,
        ownerSha256: "a".repeat(64),
        generation: null,
        outcome: "prepared",
      };
      writeFileSync(
        log,
        "noise\nprefix FVOCI_E2E_MAINTENANCE_RECEIPT " + JSON.stringify(record) + " trailing\r\n",
      );
      expect(maintenanceReceipts(log, { pid: 7, startTicks: "1" }, "t").receipts).toEqual([record]);
      for (const bad of [
        { ...record, pid: 8 },
        { ...record, key: 2 },
        { ...record, extra: 1 },
        { ...record, ownerSha256: "A".repeat(64) },
      ]) {
        writeFileSync(log, "FVOCI_E2E_MAINTENANCE_RECEIPT " + JSON.stringify(bad) + "\n");
        expect(() => maintenanceReceipts(log, { pid: 7, startTicks: "1" }, "t")).toThrow(
          "UI_MAINTENANCE_RECEIPT_REFUSED",
        );
      }
      writeFileSync(
        log,
        ("FVOCI_E2E_MAINTENANCE_RECEIPT " + JSON.stringify(record) + "\n").repeat(31),
      );
      expect(() => maintenanceReceipts(log, { pid: 7, startTicks: "1" }, "t")).toThrow(
        "UI_MAINTENANCE_RECEIPT_CAP_REFUSED",
      );
      writeFileSync(log, Buffer.from([0x46, 0xff, 0x0a]));
      expect(() => maintenanceReceipts(log, { pid: 7, startTicks: "1" }, "t")).toThrow();
    } finally {
      rmSync(directory, { recursive: true });
    }
  });
});

describe("browser report", () => {
  test("receipts require every actual case once without skips or retries", () => {
    const titles = Array.from({ length: 8 }, (_, i) => "case " + String(i));
    const report = {
      config: { workers: 1, metadata: { selectedBackend: "libsql-remote" } },
      errors: [],
      stats: { expected: 8, unexpected: 0, flaky: 0, skipped: 0 },
      suites: [
        {
          specs: titles.map((title) => ({
            file: OFF,
            ok: true,
            title,
            tests: [
              {
                expectedStatus: "passed",
                results: [{ status: "passed", retry: 0, errors: [], attachments: [] }],
              },
            ],
          })),
        },
      ],
    };
    expect(reportCases(report, OFF, titles)).toHaveLength(8);
    const specs = (r: typeof report) => must(r.suites[0]).specs;
    const mutations: Record<string, (r: typeof report) => void> = {
      retry: (r) => (must(must(must(specs(r)[0]).tests[0]).results[0]).retry = 1),
      skip: (r) => (r.stats.skipped = 1),
      missing: (r) => specs(r).pop(),
      duplicate: (r) => (specs(r)[1] = must(specs(r)[0])),
    };
    for (const [name, mutate] of Object.entries(mutations)) {
      const wrong = clone(report);
      mutate(wrong);
      expect(() => reportCases(wrong, OFF, titles), name).toThrow(UiError);
    }
  });
});

const baselinePacket = (): Record_ => ({
  originalFailure: "TURSO_UI_BASELINE_FAILED",
  nativeOutcome: {
    operation: "failed",
    rollback: "unknown",
    commit: "not-attempted",
    baselineFailure: { phase: "row-hash", category: "protocol" },
    baselineRollbackFailure: { phase: "rollback", category: "request" },
  },
  lifecycleDrain: "unconfirmed",
  drainOutcome: "failed",
  rows: ["PRIVATE_ROW_CANARY"],
  token: "PRIVATE_AUTH_CANARY",
});
const facts = {
  expectedCount: 99,
  actualCount: 99,
  setEqual: false,
  actualOnlyCount: 1,
  expectedOnlyCount: 1,
  actualOnlyUnderscoreCount: 0,
  orderEqual: false,
  firstMismatchIndex: 1,
  actualMismatchExpectedIndex: 2,
};
const outcome = (packet: Record_) => packet.nativeOutcome as Record_;
const withComparison = (comparison: unknown, extra: Record_ = {}) => {
  const packet = baselinePacket();
  outcome(packet).baselineFailure = {
    phase: "table-contract",
    category: "protocol",
    tableComparison: comparison,
    ...extra,
  };
  return parsed(packet);
};
const project = (packet: Record_) => baselineFailureDiagnostic(packet);

describe("baseline failure projection", () => {
  test("keeps first leaf and independent finish only", () => {
    const packet = parsed(baselinePacket());
    const diagnostic = project(packet);
    expect(diagnostic).toEqual({
      originalFailure: "TURSO_UI_BASELINE_FAILED",
      diagnosticStatus: "qualified",
      baselineFailure: { phase: "row-hash", category: "protocol" },
      baselineRollbackFailure: { phase: "rollback", category: "request" },
      nativeOutcome: { operation: "failed", rollback: "unknown", commit: "not-attempted" },
      lifecycleDrain: "unconfirmed",
      drainOutcome: "failed",
    });
    expect(JSON.stringify(diagnostic)).not.toContain("PRIVATE_");
    delete outcome(packet).baselineFailure;
    outcome(packet).operation = "confirmed";
    const second = project(packet);
    expect(second.baselineFailure).toBeNull();
    expect((second.baselineRollbackFailure as Record_).phase).toBe("rollback");
    expect((second.nativeOutcome as Record_).operation).toBe("confirmed");
  });

  test("missing or forged native classification never invents a cause", () => {
    const packet = baselinePacket();
    delete outcome(packet).baselineFailure;
    delete outcome(packet).baselineRollbackFailure;
    const diagnostic = project(parsed(packet));
    expect(diagnostic.diagnosticStatus).toBe("missing");
    expect(diagnostic.baselineFailure).toBeNull();
    for (const [field, value] of [
      ["phase", "PRIVATE_ENDPOINT_CANARY"],
      ["category", "PRIVATE_TOKEN_CANARY"],
      ["phase", []],
      ["category", {}],
      ["unexpected", "PRIVATE_ROW_CANARY"],
    ] as const) {
      const forged = baselinePacket();
      (outcome(forged).baselineFailure as Record_)[field] = value;
      const result = project(parsed(forged));
      expect(result.diagnosticStatus).toBe("refused");
      expect(result.baselineFailure).toBeNull();
      expect(JSON.stringify(result)).not.toContain("PRIVATE_");
    }
    for (const field of ["operation", "rollback", "commit"]) {
      const forged = baselinePacket();
      outcome(forged)[field] = "PRIVATE_STATE_CANARY";
      const result = project(parsed(forged));
      expect(result.diagnosticStatus).toBe("refused");
      expect((result.nativeOutcome as Record_)[field]).toBeNull();
      expect(JSON.stringify(result)).not.toContain("PRIVATE_");
    }
    for (const field of ["lifecycleDrain", "drainOutcome"]) {
      const forged = baselinePacket();
      forged[field] = "PRIVATE_STATE_CANARY";
      const result = project(parsed(forged));
      expect(result.diagnosticStatus).toBe("refused");
      expect(result[field]).toBeNull();
    }
  });

  test("table contract keeps only bounded comparison facts", () => {
    const diagnostic = project(withComparison(facts));
    expect(diagnostic.diagnosticStatus).toBe("qualified");
    expect((diagnostic.baselineFailure as Record_).tableComparison).toEqual(facts);
    expect((diagnostic.nativeOutcome as Record_).rollback).toBe("unknown");
    expect(diagnostic.baselineRollbackFailure).toEqual({ phase: "rollback", category: "request" });
    expect(JSON.stringify(diagnostic)).not.toContain("PRIVATE_");
    for (const [
      expected,
      actual,
      mismatch,
      actualExpected,
      actualOnly,
      expectedOnly,
      underscore,
    ] of [
      [99, 98, 98, null, 0, 1, 0],
      [99, 100, 99, 0, 0, 0, 0],
      [99, 99, 0, null, 1, 1, 1],
      [99, 99, 1, 2, 0, 0, 0],
      [100001, 100001, 100000, 0, 1, 1, 1],
      [100001, 100001, null, null, 100001, 100001, 100001],
    ] as const) {
      const comparison = {
        ...facts,
        expectedCount: expected,
        actualCount: actual,
        actualOnlyCount: actualOnly,
        expectedOnlyCount: expectedOnly,
        actualOnlyUnderscoreCount: underscore,
        setEqual: actualOnly === 0 && expectedOnly === 0,
        firstMismatchIndex: mismatch,
        actualMismatchExpectedIndex: actualExpected,
      };
      const result = project(withComparison(comparison));
      expect(result.diagnosticStatus).toBe("qualified");
      expect((result.baselineFailure as Record_).tableComparison).toEqual(comparison);
    }
    // Earlier original receipts keep their existing phase even without these facts.
    const earlier = baselinePacket();
    outcome(earlier).baselineFailure = { phase: "table-contract", category: "protocol" };
    const old = project(parsed(earlier));
    expect(old.baselineFailure).toEqual({ phase: "table-contract", category: "protocol" });
    expect(old.diagnosticStatus).toBe("qualified");
  });

  test("table contract keeps the folded identifier duplicate refusal", () => {
    // [api_tokens, GROUPS, groups] folds/sorts to [api_tokens, groups, groups].
    const duplicate = {
      ...facts,
      expectedCount: 2,
      actualCount: 3,
      setEqual: true,
      actualOnlyCount: 0,
      expectedOnlyCount: 0,
      firstMismatchIndex: 2,
      actualMismatchExpectedIndex: 1,
    };
    const result = project(withComparison(duplicate));
    expect(result.diagnosticStatus).toBe("qualified");
    expect((result.baselineFailure as Record_).tableComparison).toEqual(duplicate);
    expect(
      Object.keys((result.baselineFailure as Record_).tableComparison as Record_),
    ).toHaveLength(9);
    expect(JSON.stringify(result)).not.toContain("groups");
  });

  test("table contract refuses private names and forged counts or indices", () => {
    const faults: [string, unknown][] = [
      ["actualNames", ["PRIVATE_TABLE_CANARY"]],
      ["expectedCount", true],
      ["actualCount", "PRIVATE_NAME_OR_ENDPOINT"],
      ["actualCount", 100002],
      ["setEqual", "PRIVATE_TOKEN"],
      ["orderEqual", true],
      ["firstMismatchIndex", -1],
      ["firstMismatchIndex", 100],
      ["actualMismatchExpectedIndex", true],
      ["actualMismatchExpectedIndex", 99],
      ["actualMismatchExpectedIndex", "PRIVATE_TABLE_CANARY"],
      ["actualOnlyCount", 100],
      ["expectedOnlyCount", 100],
      ["actualOnlyUnderscoreCount", 2],
      ["setEqual", true],
    ];
    for (const field of ["actualOnlyCount", "expectedOnlyCount", "actualOnlyUnderscoreCount"])
      for (const value of [true, -1, 100002, null, "PRIVATE_TABLE_CANARY"])
        faults.push([field, value]);
    for (const [field, value] of faults) {
      const result = project(withComparison({ ...facts, [field]: value }));
      expect(result.diagnosticStatus, field + "=" + String(value)).toBe("refused");
      expect(result.baselineFailure).toBeNull();
      expect(JSON.stringify(result)).not.toContain("PRIVATE_");
    }
    for (const field of Object.keys(facts)) {
      const comparison: Record_ = { ...facts };
      Reflect.deleteProperty(comparison, field);
      expect(project(withComparison(comparison)).diagnosticStatus).toBe("refused");
    }
    expect(
      project(withComparison({ ...facts, actualOnlyCount: 0, expectedOnlyCount: 0 }))
        .diagnosticStatus,
    ).toBe("refused");
    for (const count of [99, 0]) {
      const result = project(
        withComparison({
          ...facts,
          expectedCount: count,
          actualCount: count,
          firstMismatchIndex: count,
          actualMismatchExpectedIndex: null,
        }),
      );
      expect(result.diagnosticStatus).toBe("refused");
      expect(result.baselineFailure).toBeNull();
    }
    for (const mismatch of [null, 98])
      expect(
        project(
          withComparison({
            ...facts,
            actualCount: 98,
            firstMismatchIndex: mismatch,
            actualMismatchExpectedIndex: 0,
          }),
        ).diagnosticStatus,
      ).toBe("refused");
    expect(
      project(
        withComparison({
          ...facts,
          actualCount: 98,
          firstMismatchIndex: 98,
          actualOnlyCount: 0,
          actualMismatchExpectedIndex: null,
        }),
      ).diagnosticStatus,
    ).toBe("qualified");
    for (const extra of [{ phase: "row-hash" }, { category: "driver" }])
      expect(project(withComparison(facts, extra)).diagnosticStatus).toBe("refused");
    // A float token is not an integer count, as in the original type check.
    const packet = baselinePacket();
    outcome(packet).baselineFailure = {
      phase: "table-contract",
      category: "protocol",
      tableComparison: facts,
    };
    const text = JSON.stringify(packet).replace('"expectedCount":99', '"expectedCount":99.0');
    expect(project(parseJson(text) as Record_).diagnosticStatus).toBe("refused");
  });
});
