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
  selectedReadbackAppendEffect,
  nativeReceiptsRetained,
  reportCases,
  startupBlockers,
  type Audit,
} from "./ui-audit.ts";
import { UiError, get, record, type Record_ } from "./ui-common.ts";
import { parseJson, digest } from "../selected-backend-ci/io.ts";
import { clone, must, parsed } from "./ui-fakes.ts";

const number = (n: number | string) => ["integer", String(n)];

test("restart retains every complete native operation receipt independently of barrier IDs", () => {
  const before = [1, 2, 3].map((seq) => ({
    seq,
    op: `operation-${String(seq)}`,
    len: 10,
    sha: "a".repeat(64),
    actor: "creator",
  }));
  expect(nativeReceiptsRetained(before, [...clone(before), { seq: 4, op: "new-operation" }])).toBe(
    true,
  );
  for (const field of ["seq", "op", "len", "sha", "actor"]) {
    const changed = clone(before) as Record_[];
    must(changed[1])[field] = "changed";
    expect(nativeReceiptsRetained(before, changed), field).toBe(false);
  }
  expect(nativeReceiptsRetained(before, [before[0], before[2]])).toBe(false);
  expect(nativeReceiptsRetained([], [])).toBe(false);
  expect(nativeReceiptsRetained([before[0]], [before[0]])).toBe(false);
});
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

function vapidRecords(): [Record_, Record_, Audit] {
  const [before, after, audit] = counterRecords();
  const stableSha256 = "7".repeat(64);
  before.instanceConfig = [
    {
      rowSha256: "5".repeat(64),
      stableSha256,
      singleton: true,
      vapidBothNull: true,
      vapidPublicPresent: false,
      vapidPrivateSealed: false,
    },
  ];
  after.instanceConfig = [
    {
      rowSha256: "6".repeat(64),
      stableSha256,
      singleton: true,
      vapidBothNull: false,
      vapidPublicPresent: true,
      vapidPrivateSealed: true,
    },
  ];
  (before.fingerprints as Record_).instance_config = { ["5".repeat(64)]: 1 };
  (after.fingerprints as Record_).instance_config = { ["6".repeat(64)]: 1 };
  return [before, after, audit];
}
const vapidWitness = (value: Record_) => (value.instanceConfig as Record_[])[0] as Record_;

describe("preservation audit", () => {
  test("only the singleton null-to-sealed VAPID initialization is preserved", () => {
    const [before, after, audit] = vapidRecords();
    assertPreserved(before, after, audit);
    expect(() => {
      assertPreserved(before, after);
    }).toThrow("UI_PREEXISTING_ROW_CHANGED");
    const initialized = clone(after);
    (before.fingerprints as Record_).instance_config = clone(
      (after.fingerprints as Record_).instance_config,
    );
    before.instanceConfig = clone(initialized.instanceConfig);
    assertPreserved(before, initialized, audit);
  });

  test.each([
    [
      "existing public key",
      (b: Record_) => {
        vapidWitness(b).vapidBothNull = false;
        vapidWitness(b).vapidPublicPresent = true;
      },
    ],
    [
      "existing private key",
      (b: Record_) => {
        vapidWitness(b).vapidBothNull = false;
        vapidWitness(b).vapidPrivateSealed = true;
      },
    ],
    [
      "missing public key",
      (_b: Record_, a: Record_) => {
        vapidWitness(a).vapidPublicPresent = false;
      },
    ],
    [
      "unsealed private key",
      (_b: Record_, a: Record_) => {
        vapidWitness(a).vapidPrivateSealed = false;
      },
    ],
    [
      "other column changed",
      (_b: Record_, a: Record_) => {
        vapidWitness(a).stableSha256 = "8".repeat(64);
      },
    ],
    [
      "wrong old singleton id",
      (b: Record_) => {
        vapidWitness(b).singleton = false;
      },
    ],
    [
      "wrong new singleton id",
      (_b: Record_, a: Record_) => {
        vapidWitness(a).singleton = false;
      },
    ],
    [
      "extra old row",
      (b: Record_) => {
        (b.instanceConfig as unknown[]).push(clone(vapidWitness(b)));
      },
    ],
    [
      "extra new row",
      (_b: Record_, a: Record_) => {
        (a.instanceConfig as unknown[]).push(clone(vapidWitness(a)));
      },
    ],
    [
      "missing proof",
      (b: Record_) => {
        delete b.instanceConfig;
      },
    ],
    [
      "unbound old row",
      (b: Record_) => {
        vapidWitness(b).rowSha256 = "8".repeat(64);
      },
    ],
    [
      "unbound new row",
      (_b: Record_, a: Record_) => {
        vapidWitness(a).rowSha256 = "8".repeat(64);
      },
    ],
    [
      "duplicate fingerprint",
      (_b: Record_, a: Record_) => {
        (a.fingerprints as Record_).instance_config = { ["6".repeat(64)]: 2 };
      },
    ],
    [
      "other singleton",
      (b: Record_, a: Record_) => {
        (b.fingerprints as Record_).instance_settings = { ["8".repeat(64)]: 1 };
        (a.fingerprints as Record_).instance_settings = { ["9".repeat(64)]: 1 };
      },
    ],
  ] as const)("VAPID exception refuses %s", (_label, mutate) => {
    const [before, after, audit] = vapidRecords();
    mutate(before, after);
    expect(() => {
      assertPreserved(before, after, audit);
    }).toThrow("UI_PREEXISTING_ROW_CHANGED");
  });

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

  test("only native witnessed login and document projection events may omit a scope", () => {
    for (const projection of [false, true]) {
      const [original, after, audit] = counterRecords();
      const target = projection ? "7".repeat(32) : must(actor[1]);
      audit.documents = { [target]: must(workspace[1]) };
      const allocations = (after.allocations as Record<string, Record<string, Record_>>).events;
      const refs = must(must(allocations)["3".repeat(64)]);
      refs.workspaces = projection ? [workspace[1]] : [];
      refs.actors = projection ? [] : [actor[1]];
      refs.eventWitness = {
        class: projection ? "document-projection" : "actor-login",
        sequence: number(1),
        target,
      };
      must(ops(after).events)[0] = [
        number(1),
        projection ? workspace : ["null"],
        projection ? ["null"] : actor,
      ];
      assertPreserved(original, after, audit);
      for (const field of ["class", "sequence", "target"]) {
        const wrong = clone(after);
        const witness = (
          (
            (wrong.allocations as Record<string, Record<string, Record_>>).events as Record<
              string,
              Record_
            >
          )["3".repeat(64)] as Record_
        ).eventWitness as Record_;
        witness[field] = field === "sequence" ? number(2) : "foreign";
        expect(() => {
          assertPreserved(original, wrong, audit);
        }, field).toThrow(UiError);
      }
      expect(() => {
        assertPreserved(original, after, { ...audit, documents: {}, actors: {} });
      }).toThrow(UiError);
      const noWitness = clone(after);
      delete (
        must((noWitness.allocations as Record<string, Record<string, Record_>>).events)[
          "3".repeat(64)
        ] as Record_
      ).eventWitness;
      expect(() => {
        assertPreserved(original, noWitness, audit);
      }).toThrow(UiError);
    }
  });

  test.each(["documents", "tasks"] as const)(
    "OFF %s projection needs an exact new native resource allocation, including personal space",
    (table) => {
      for (const personal of [false, true]) {
        const [original, after, audit] = counterRecords();
        const target = "7".repeat(32);
        const scope = personal ? "8".repeat(32) : must(workspace[1]);
        const resourceHash = "6".repeat(64);
        const prints = after.fingerprints as Record<string, Record_>;
        const allocations = after.allocations as Record<string, Record<string, Record_>>;
        if (personal) {
          must(must(ops(after).users)[0])[2] = ["blob", scope];
          must(ops(after).workspaces).push([
            ["blob", scope],
            ["text", "private-owned"],
            ["text", "personal"],
          ]);
          must(prints.workspaces)["8".repeat(64)] = 1;
          must(allocations.workspaces)["8".repeat(64)] = {
            workspaces: [],
            actors: [],
            events: [],
            self: scope,
            consumer: null,
          };
        }
        must(prints[table])[resourceHash] = 1;
        must(allocations[table])[resourceHash] = {
          workspaces: [scope],
          actors: [actor[1]],
          events: [],
          self: target,
          consumer: null,
        };
        const refs = must(must(allocations.events)["3".repeat(64)]);
        refs.workspaces = [scope];
        refs.actors = [];
        refs.eventWitness = {
          class: table === "documents" ? "document-projection" : "task-projection",
          sequence: number(1),
          target,
        };
        must(ops(after).events)[0] = [number(1), ["blob", scope], ["null"]];
        assertPreserved(original, after, audit);
        for (const field of ["self", "workspaces", "actors"]) {
          const wrong = clone(after);
          const resource = must(
            must((wrong.allocations as Record<string, Record<string, Record_>>)[table])[
              resourceHash
            ],
          );
          resource[field] = field === "self" ? "9".repeat(32) : ["9".repeat(32)];
          expect(() => {
            assertPreserved(original, wrong, audit);
          }, field).toThrow(UiError);
        }
        const noResource = clone(after);
        Reflect.deleteProperty(
          must((noResource.allocations as Record<string, Record<string, Record_>>)[table]),
          resourceHash,
        );
        expect(() => {
          assertPreserved(original, noResource, audit);
        }).toThrow();
        const preexisting = clone(original);
        must((preexisting.fingerprints as Record<string, Record_>)[table])[resourceHash] = 1;
        expect(() => {
          assertPreserved(preexisting, after, audit);
        }).toThrow(UiError);
      }
    },
  );

  test("auth revocation events are witnessed only for a dedicated auth allocation actor", () => {
    for (const kind of ["actor-logout", "actor-password-change", "actor-password-reset"]) {
      const logout = kind === "actor-logout";
      const [original, after, audit] = counterRecords();
      const subject = must(actor[1]);
      const authAudit: Audit = { ...audit, authActors: [subject] };
      const allocations = (after.allocations as Record<string, Record<string, Record_>>).events;
      const refs = must(must(allocations)["3".repeat(64)]);
      refs.workspaces = [];
      refs.actors = [subject];
      refs.eventWitness = {
        class: kind,
        sequence: number(1),
        ...(logout ? { actor: subject } : { target: subject }),
      };
      must(ops(after).events)[0] = [number(1), ["null"], actor];
      assertPreserved(original, after, authAudit);
      // The same producer of a non-auth allocation actor stays foreign.
      expect(() => {
        assertPreserved(original, after, audit);
      }, kind).toThrow(UiError);
      const witnessOf = (value: Record_) =>
        must(
          must((value.allocations as Record<string, Record<string, Record_>>).events)[
            "3".repeat(64)
          ],
        );
      for (const fault of ["subject", "extra", "people", "spaces", "class"]) {
        const wrong = clone(after);
        const wrongRefs = witnessOf(wrong);
        const witness = wrongRefs.eventWitness as Record_;
        if (fault === "subject") witness[logout ? "actor" : "target"] = "9".repeat(32);
        else if (fault === "extra") witness[logout ? "target" : "actor"] = subject;
        else if (fault === "people") wrongRefs.actors = ["9".repeat(32)];
        else if (fault === "spaces") wrongRefs.workspaces = [workspace[1]];
        else witness.class = "actor-password-unknown";
        expect(
          () => {
            assertPreserved(original, wrong, authAudit);
          },
          kind + " " + fault,
        ).toThrow(UiError);
      }
    }
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

test("SQLite seeded readback state preserves every byte and only admits one native claim", async () => {
  const { selectedReadbackState } = await import("./ui-audit.ts");
  const { digest } = await import("../selected-backend-ci/io.ts");
  const hash = (value: unknown) => digest(JSON.stringify(value));
  const original = [
    ["blob", "a".repeat(32)],
    ["blob", "b".repeat(32)],
    ["blob", "00ff80"],
    number(1),
    ["null"],
    number(10),
    number(10),
    number(7),
    number(2),
    number(2),
  ];
  const current = structuredClone(original);
  current[6] = number(30);
  current[7] = number(8);
  const dataset = (row: unknown[]) => ({
    backend: "sqlite",
    fingerprints: { document_states: { [hash(row)]: 1 } },
  });
  const before = dataset(original),
    after = dataset(current);
  const prepared = { dataset: dataset(current), checks: { clockMicros: "40" } };
  const audit = {
    sqlitePreparation: {
      workspace: "a".repeat(32),
      document: "b".repeat(32),
      original: { dataset: before, checks: { clockMicros: "20" } },
      prepared,
      state: { original, current, after: structuredClone(current) },
    },
    serial: { expectedBackend: "sqlite", before: { checks: { clockMicros: "20" } } },
  } as unknown as Audit;
  expect(selectedReadbackState(before, after, audit)).toBe(hash(original));
  const changes: ((value: Audit) => void)[] = [
    (a) => {
      if (a.sqlitePreparation) a.sqlitePreparation.state.current[2] = ["blob", "00ff81"];
    },
    (a) => {
      if (a.sqlitePreparation) a.sqlitePreparation.state.current[7] = number(9);
    },
    (a) => {
      if (a.sqlitePreparation) a.sqlitePreparation.state.current[6] = number(19);
    },
    (a) => {
      if (a.sqlitePreparation) a.sqlitePreparation.state.current[8] = number(3);
    },
    (a) => {
      if (a.sqlitePreparation) a.sqlitePreparation.document = "c".repeat(32);
    },
    (a) => {
      if (a.sqlitePreparation) a.sqlitePreparation.state.after[9] = number(3);
    },
    (a) => {
      if (a.serial) a.serial.expectedBackend = undefined;
    },
  ];
  for (const change of changes) {
    const wrong = structuredClone(audit);
    change(wrong);
    expect(() => selectedReadbackState(before, after, wrong)).toThrow();
  }
  const missing = structuredClone(after);
  missing.fingerprints.document_states = {};
  expect(() => selectedReadbackState(before, missing, audit)).toThrow();
  const remote = { ...after, backend: "libsql-remote" };
  expect(() => selectedReadbackState(before, remote, audit)).toThrow();
  // A native append witness changes only the exact tail, alongside the
  // existing one-claim metadata; cutoff and state bytes stay pinned above.
  const appended = structuredClone(audit);
  const selected = must(appended.sqlitePreparation);
  selected.readback = { count: 2, rows: {} };
  selected.state.current[9] = number(4);
  selected.state.after = structuredClone(selected.state.current);
  const appendedDataset = dataset(selected.state.current);
  selected.prepared.dataset = appendedDataset;
  expect(selectedReadbackState(before, appendedDataset, appended)).toBe(hash(original));
  for (const tail of [2, 3, 5]) {
    const wrong = structuredClone(appended);
    must(wrong.sqlitePreparation).state.current[9] = number(tail);
    expect(() => selectedReadbackState(before, appendedDataset, wrong)).toThrow();
  }
});

test("SQLite readback full-row seal admits only its two exact hashes and exact old reader references", () => {
  const tables = ["document_collab_op_receipts", "document_collab_updates", "events", "audit_log"];
  const rows = Object.fromEntries(
    tables.map((table) => [
      table,
      {
        [digest(table + "first")]: 1,
        [digest(table + "second")]: 1,
      },
    ]),
  );
  const before = { fingerprints: Object.fromEntries(tables.map((table) => [table, {}])) };
  const eventHashes = Object.keys(rows.events ?? {});
  const eventOperations = Object.fromEntries(
    eventHashes.map((hash, index) => [
      hash,
      [number(index + 1), ["blob", "a".repeat(32)], ["blob", "b".repeat(32)]],
    ]),
  );
  const eventIds = Object.fromEntries(
    eventHashes.map((hash, index) => [hash, String(index + 1).repeat(32)]),
  );
  const after = { fingerprints: rows, operations: { events: Object.values(eventOperations) } };
  const audit = {
    sqlitePreparation: {
      workspace: "a".repeat(32),
      readbackActor: { id: "b".repeat(32) },
      prepared: { dataset: structuredClone(after) },
      readback: { count: 2, rows: structuredClone(rows), eventOperations, eventIds },
    },
  } as unknown as Audit;
  const table = tables[0] as string,
    sha = Object.keys(rows[table] ?? {})[0] as string;
  const refs = { workspaces: ["a".repeat(32)], actors: ["b".repeat(32)] };
  expect(selectedReadbackAppendEffect(before, after, table, sha, 1, refs, audit)).toBe(true);
  expect(selectedReadbackAppendEffect(before, after, table, "f".repeat(64), 1, refs, audit)).toBe(
    false,
  );
  for (const mutate of [
    (_a: Audit, b: typeof after) => {
      b.fingerprints[table] = {};
    },
    (a: Audit) => {
      must(a.sqlitePreparation).readback = { count: 2, rows: { [table]: rows[table] ?? {} } };
    },
    (a: Audit) => {
      must(must(a.sqlitePreparation).readback).rows[table] = { [sha]: 1 };
    },
    (a: Audit) => {
      must(a.sqlitePreparation).prepared.dataset = before;
    },
  ]) {
    const a = structuredClone(audit),
      b = structuredClone(after);
    mutate(a, b);
    expect(() => selectedReadbackAppendEffect(before, b, table, sha, 1, refs, a)).toThrow();
  }
  for (const wrong of [
    { ...refs, actors: [] },
    { ...refs, workspaces: ["c".repeat(32)] },
  ])
    expect(() =>
      selectedReadbackAppendEffect(before, after, table, sha, 1, wrong, audit),
    ).toThrow();
  expect(() => selectedReadbackAppendEffect(before, after, table, sha, 2, refs, audit)).toThrow();
});

function sqlitePreparedRecords() {
  const id = (digit: string) => digit.repeat(32),
    h = (value: unknown) => digest(JSON.stringify(value));
  const prep = id("1"),
    reader = id("2"),
    newActors = [id("3"), id("4"), id("5")],
    workspace = id("6"),
    oldWorkspace = id("7"),
    document = id("8"),
    sessions = [id("c"), id("d")];
  const refs = (
    workspaces: string[] = [],
    actors: string[] = [],
    self: string | null = null,
  ): Record_ => ({ workspaces, actors, events: [], self, consumer: null });
  const originalState = [
    ["blob", oldWorkspace],
    ["blob", document],
    ["blob", "00ff"],
    number(1),
    ["null"],
    number(10),
    number(10),
    number(7),
    number(2),
    number(2),
  ];
  const state = structuredClone(originalState);
  state[6] = number(30);
  state[7] = number(8);
  const oldFence = [
    ["blob", oldWorkspace],
    ["blob", document],
    ["blob", id("a")],
    number(1),
    number(10),
  ];
  const fence = [
    ["blob", oldWorkspace],
    ["blob", document],
    ["blob", id("b")],
    number(2),
    number(50),
  ];
  const userHashes = [h("original-preparation-user"), h("original-readback-user")];
  const fingerprints: Record<string, Record<string, number>> = {
    users: { [userHashes[0] as string]: 1, [userHashes[1] as string]: 1 },
    documents: { [h("original-unrelated-document")]: 1 },
    tasks: {},
    attachments: {},
    workspaces: { [h("original-workspace")]: 1 },
    sessions: {},
    events: {},
    invitations: {},
    notifications: {},
    memberships: {},
    document_states: { [h(originalState)]: 1 },
    collab_room_fences: { [h(oldFence)]: 1 },
    maintenance_job_claims: {},
  };
  const allocations: Record<string, Record<string, Record_>> = Object.fromEntries(
    Object.keys(fingerprints).map((table) => [table, {}]),
  );
  allocations.users = {
    [userHashes[0] as string]: refs([], [], prep),
    [userHashes[1] as string]: refs([], [], reader),
  };
  const operations: Record<string, unknown[]> = {
    users: [
      ["blob", prep],
      ["blob", reader],
    ].map((value, index) => [
      value,
      ["text", index === 0 ? "original-prep" : "original-reader"],
      ["null"],
    ]),
    workspaces: [
      [
        ["blob", oldWorkspace],
        ["text", "original"],
        ["text", "team"],
      ],
    ],
    events: [],
    event_sequence: [[number(1), number(0)]],
    collab_fence_counter: [[number(1), number(2)]],
    collab_room_fences: [oldFence],
    task_collab_room_fences: [],
    maintenance_job_claims: [1, 2, 3, 4, 5, 6, 7, 8, 9].map((key) => [
      number(key),
      ["null"],
      number(0),
      ["null"],
    ]),
  };
  for (const row of operations.maintenance_job_claims as unknown[])
    (fingerprints.maintenance_job_claims as Record<string, number>)[h(row)] = 1;
  const before: Record_ = {
    backend: "sqlite",
    schemaCurrent: true,
    schemaSha256: "f".repeat(64),
    lineage: "fvoci-sqlite-060",
    ledger: [],
    fingerprints,
    allocations,
    operations,
  };
  const prepared = structuredClone(before),
    prints = record(prepared, "fingerprints"),
    allocated = record(prepared, "allocations"),
    ops = record(prepared, "operations");
  for (const [index, actor] of newActors.entries()) {
    const hash = h(["new-user", actor]);
    record(prints, "users")[hash] = 1;
    record(allocated, "users")[hash] = refs([], [], actor);
    (ops.users as unknown[]).push([["blob", actor], ["text", "new-" + String(index)], ["null"]]);
  }
  record(prints, "workspaces")[h("new-workspace")] = 1;
  record(allocated, "workspaces")[h("new-workspace")] = refs([], [], workspace);
  (ops.workspaces as unknown[]).push([
    ["blob", workspace],
    ["text", namespace],
    ["text", "team"],
  ]);
  for (const [index, actor] of [prep, reader].entries()) {
    const session = sessions[index] as string,
      hash = h(["session", actor]);
    record(prints, "sessions")[hash] = 1;
    record(allocated, "sessions")[hash] = refs([], [actor], session);
    const event = h(["login", actor]),
      seq = index + 1;
    record(prints, "events")[event] = 1;
    record(allocated, "events")[event] = {
      ...refs([], [actor], event.slice(0, 32)),
      eventWitness: { class: "actor-login", sequence: number(seq), target: actor },
    };
    (ops.events as unknown[]).push([number(seq), ["null"], ["blob", actor]]);
  }
  const event = h("workspace-created");
  record(prints, "events")[event] = 1;
  record(allocated, "events")[event] = {
    ...refs([workspace], [prep], event.slice(0, 32)),
    eventWitness: null,
  };
  (ops.events as unknown[]).push([number(3), ["blob", workspace], ["blob", prep]]);
  ops.event_sequence = [[number(1), number(3)]];
  ops.collab_fence_counter = [[number(1), number(3)]];
  prints.document_states = { [h(state)]: 1 };
  record(allocated, "document_states")[h(state)] = refs([oldWorkspace]);
  prints.collab_room_fences = { [h(fence)]: 1 };
  record(allocated, "collab_room_fences")[h(fence)] = refs([oldWorkspace]);
  ops.collab_room_fences = [fence];
  const maintenanceRows = ops.maintenance_job_claims as unknown[][];
  for (const row of maintenanceRows)
    if ([1, 8, 9].includes(Number(get(row, 0, 1)))) row[2] = number(1);
  prints.maintenance_job_claims = Object.fromEntries(maintenanceRows.map((row) => [h(row), 1]));
  const maintenance = {
    leaseMicros: "300000000",
    server: {
      identity: { pid: 99, startTicks: "123" },
      targetSha256: "e".repeat(64),
      receipts: [8, 9, 1].flatMap((key) =>
        ["prepared", "acquired", "released"].map((outcome) => ({
          schema: 1,
          pid: 99,
          key,
          ownerSha256: digest(Buffer.from(String(key).repeat(32), "hex")),
          generation: outcome === "prepared" ? null : "1",
          outcome,
        })),
      ),
    },
  };
  const original = { dataset: before, checks: { clockMicros: "20", outbox: [] } },
    current = {
      dataset: prepared,
      checks: {
        clockMicros: "40",
        eventHead: "3",
        outbox: [],
        owners: { maintenance_job_claims: maintenanceRows },
      },
    };
  const audit: Audit = {
    namespaces: [namespace],
    actors: Object.fromEntries(newActors.map((actor, index) => [actor, "new-" + String(index)])),
    workspaces: { [workspace]: namespace },
    documents: {},
    servers: [],
    targetSha256: "e".repeat(64),
    serverStarts: 1,
    observedFences: [fence],
    sqlitePreparation: {
      preparationActor: {
        id: prep,
        email: "original-prep",
        rowFingerprint: userHashes[0] as string,
        sessionId: sessions[0] as string,
      },
      readbackActor: {
        id: reader,
        email: "original-reader",
        rowFingerprint: userHashes[1] as string,
        sessionId: sessions[1] as string,
      },
      workspace: oldWorkspace,
      document,
      original,
      prepared: current,
      maintenance,
      state: { original: originalState, current: state, after: structuredClone(state) },
    },
    serial: {
      before: original,
      after: current,
      policy: { revisionKeep: 200, revisionSnapshotIntervalHours: 24 },
      expectedBackend: "sqlite",
    },
  };
  return { before, prepared, audit, reader, workspace, h, refs };
}

test("prepared-only original document or actor mutation cannot be hidden by final restoration", async () => {
  const { assertSqlitePreparedPreserved } = await import("./ui-audit.ts");
  const valid = sqlitePreparedRecords();
  expect(() => {
    assertSqlitePreparedPreserved(valid.before, valid.prepared, valid.audit);
  }).not.toThrow();
  for (const table of ["documents", "users"]) {
    const changed = sqlitePreparedRecords(),
      prints = record(changed.prepared, "fingerprints", table);
    const original = Object.keys(record(changed.before, "fingerprints", table))[0] as string;
    Reflect.deleteProperty(prints, original);
    prints["9".repeat(64)] = 1;
    expect(() => {
      assertSqlitePreparedPreserved(changed.before, changed.prepared, changed.audit);
    }).toThrow("UI_PREEXISTING_ROW_CHANGED");
    // A restored final observation cannot erase that observed prepared failure.
    expect(() => {
      assertSqlitePreparedPreserved(valid.before, valid.prepared, valid.audit);
    }).not.toThrow();
  }
});

test("prepared maintenance preserves every cell of unscheduled keys even if final rows are restored", async () => {
  const { assertSqlitePreparedPreserved } = await import("./ui-audit.ts");
  const valid = sqlitePreparedRecords();
  expect(() => {
    assertSqlitePreparedPreserved(valid.before, valid.prepared, valid.audit);
  }).not.toThrow();
  for (const key of [2, 3, 4, 5, 6, 7]) {
    for (const [column, cell] of [
      [1, ["blob", "a".repeat(32)]],
      [2, number(1)],
      [3, number(300000030)],
    ] as const) {
      const f = sqlitePreparedRecords(),
        rows = get(f.prepared, "operations", "maintenance_job_claims") as unknown[][];
      const row = must(rows[key - 1]);
      row[column] = [...cell];
      record(f.prepared, "fingerprints").maintenance_job_claims = Object.fromEntries(
        rows.map((r) => [f.h(r), 1]),
      );
      expect(() => {
        assertSqlitePreparedPreserved(f.before, f.prepared, f.audit);
      }).toThrow("UI_SQLITE_UNSCHEDULED_MAINTENANCE_CHANGED");
      // A later restored row cannot authorize the observed prepared mutation.
      expect(get(valid.prepared, "operations", "maintenance_job_claims", key - 1)).toEqual(
        get(valid.before, "operations", "maintenance_job_claims", key - 1),
      );
    }
  }
});

test("prepared scheduled maintenance accepts exact producer prefixes and refuses foreign owner/generation/expiry", async () => {
  const { assertSqlitePreparedPreserved } = await import("./ui-audit.ts");
  const live = () => {
    const f = sqlitePreparedRecords(),
      row = must((get(f.prepared, "operations", "maintenance_job_claims") as unknown[][])[7]);
    row[1] = ["blob", "8".repeat(32)];
    row[3] = number(300000030);
    const receipts = must(f.audit.sqlitePreparation?.maintenance).server.receipts as Record_[];
    must(f.audit.sqlitePreparation?.maintenance).server.receipts = receipts.filter(
      (r) => r.key !== 8 || r.outcome !== "released",
    );
    return { ...f, row };
  };
  const check = (f: ReturnType<typeof live>) => {
    const rows = get(f.prepared, "operations", "maintenance_job_claims") as unknown[][];
    record(f.prepared, "fingerprints").maintenance_job_claims = Object.fromEntries(
      rows.map((r) => [f.h(r), 1]),
    );
    assertSqlitePreparedPreserved(f.before, f.prepared, f.audit);
  };
  expect(() => {
    check(live());
  }).not.toThrow();
  const pending = live();
  pending.row[1] = ["null"];
  pending.row[2] = number(0);
  pending.row[3] = ["null"];
  must(pending.audit.sqlitePreparation?.maintenance).server.receipts = (
    must(pending.audit.sqlitePreparation?.maintenance).server.receipts as Record_[]
  ).filter((r) => r.key !== 8 || r.outcome === "prepared");
  expect(() => {
    check(pending);
  }).not.toThrow();
  const mutations: ((f: ReturnType<typeof live>) => void)[] = [
    (f) => {
      f.row[1] = ["blob", "a".repeat(32)];
    },
    (f) => {
      f.row[2] = number(2);
    },
    (f) => {
      f.row[3] = number(300000019);
    },
    (f) => {
      f.row[3] = number(300000041);
    },
    (f) => {
      f.row[3] = number(40);
    },
    (f) => {
      f.row[1] = ["null"];
      f.row[3] = ["null"];
    },
    (f) => {
      must((must(f.audit.sqlitePreparation?.maintenance).server.receipts as Record_[])[0]).pid =
        100;
    },
    (f) => {
      must(f.audit.sqlitePreparation?.maintenance).server.targetSha256 = "f".repeat(64);
    },
    (f) => {
      must(f.audit.sqlitePreparation?.maintenance).leaseMicros = "300000001";
    },
    (f) => {
      must(
        (must(f.audit.sqlitePreparation?.maintenance).server.receipts as Record_[])[1],
      ).generation = "2";
    },
  ];
  for (const mutate of mutations) {
    const f = live();
    mutate(f);
    expect(() => {
      check(f);
    }).toThrow();
  }
});

test("readback actor non-login namespace events refuse in prepared and final even with valid login/session", async () => {
  const { assertSqlitePreparedPreserved, allocatedRows } = await import("./ui-audit.ts");
  const f = sqlitePreparedRecords(),
    hash = f.h("foreign-readback-event");
  record(f.prepared, "fingerprints", "events")[hash] = 1;
  record(f.prepared, "allocations", "events")[hash] = {
    ...f.refs([f.workspace], [f.reader], hash.slice(0, 32)),
    eventWitness: null,
  };
  (get(f.prepared, "operations", "events") as unknown[]).push([
    number(4),
    ["blob", f.workspace],
    ["blob", f.reader],
  ]);
  record(f.prepared, "operations").event_sequence = [[number(1), number(4)]];
  expect(() => {
    assertSqlitePreparedPreserved(f.before, f.prepared, f.audit);
  }).toThrow("UI_SQLITE_PREPARATION_EFFECT_REFUSED");
  expect(() => allocatedRows(f.before, f.prepared, f.audit)).toThrow(
    "UI_SQLITE_PREPARATION_EFFECT_REFUSED",
  );
});

test("SQLite invitation login is bound to one exact prepared new-actor row in both phases", async () => {
  const { assertSqlitePreparedPreserved, allocatedRows } = await import("./ui-audit.ts");
  const fixture = () => {
    const f = sqlitePreparedRecords(),
      actor = Object.keys(f.audit.actors)[0] as string,
      sha = f.h("invitation-login"),
      refs = {
        ...f.refs([], [actor], "e".repeat(32)),
        actors: [actor],
        workspaces: [] as string[],
        eventWitness: { class: "actor-invitation-login", sequence: number(4), target: actor },
      };
    record(f.prepared, "fingerprints", "events")[sha] = 1;
    record(f.prepared, "allocations", "events")[sha] = refs;
    (get(f.prepared, "operations", "events") as unknown[]).push([
      number(4),
      ["null"],
      ["blob", actor],
    ]);
    record(f.prepared, "operations").event_sequence = [[number(1), number(4)]];
    return { ...f, sha, refs };
  };
  const check = (f: ReturnType<typeof fixture>, phase: "prepared" | "after") => {
    if (phase === "prepared") assertSqlitePreparedPreserved(f.before, f.prepared, f.audit);
    else allocatedRows(f.before, f.prepared, f.audit, phase);
  };
  for (const phase of ["prepared", "after"] as const) {
    expect(() => {
      check(fixture(), phase);
    }).not.toThrow();
    for (const mutate of [
      (f: ReturnType<typeof fixture>) => {
        f.refs.eventWitness.target = f.reader;
      },
      (f: ReturnType<typeof fixture>) => {
        f.refs.actors = [f.reader];
      },
      (f: ReturnType<typeof fixture>) => {
        f.refs.workspaces = [f.workspace];
      },
      (f: ReturnType<typeof fixture>) => {
        record(f.prepared, "fingerprints", "events")[f.sha] = 2;
      },
      (f: ReturnType<typeof fixture>) => {
        f.audit.sqlitePreparation = undefined;
      },
      (f: ReturnType<typeof fixture>) => {
        record(must(f.audit.sqlitePreparation).prepared.dataset).backend = "libsql-remote";
      },
    ]) {
      const f = fixture();
      mutate(f);
      expect(() => {
        check(f, phase);
      }).toThrow();
    }
    const f = fixture(),
      after = structuredClone(f.prepared);
    (record(after, "allocations", "events")[f.sha] as Record_).eventWitness = {
      ...f.refs.eventWitness,
      sequence: number(5),
    };
    expect(() => allocatedRows(f.before, after, f.audit, phase)).toThrow("UI_FOREIGN_EVENT");
  }
});

test("SQLite extraction events bind one new closed document attachment and its exact producer row", async () => {
  const { allocatedRows } = await import("./ui-audit.ts");
  const fixture = () => {
    const f = sqlitePreparedRecords(),
      after = structuredClone(f.prepared),
      uploader = Object.keys(f.audit.actors)[0] as string,
      document = "a".repeat(32),
      target = "b".repeat(32),
      eventId = "e".repeat(32),
      attachmentHash = f.h("complete-private-attachment-row"),
      cells: unknown[][] = [
        ["blob", eventId],
        number(4),
        ["blob", f.workspace],
        ["null"],
        ["text", "attachment.extracted"],
        ["text", "attachment"],
        ["blob", target],
        [
          "text",
          JSON.stringify({ attachmentId: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", status: "ok" }),
        ],
        ["text", "web"],
        number(50),
      ],
      sha = f.h(cells),
      fields: unknown[][] = [
        ["blob", target],
        ["blob", f.workspace],
        ["blob", document],
        ["null"],
        ["blob", uploader],
        ["text", "stored"],
        ["text", "ok"],
        number(1),
        ["null"],
        ["null"],
        number(45),
      ],
      extraction = {
        columns: [
          "id",
          "workspace_id",
          "document_id",
          "task_id",
          "uploader_id",
          "status",
          "extract_status",
          "extract_attempts",
          "extract_lease_token",
          "extract_lease_expires_at",
          "completed_at",
        ],
        cells: fields,
        sha256: attachmentHash,
      },
      attachment = { ...f.refs([f.workspace], [uploader], target), extraction },
      event = {
        ...f.refs([f.workspace], [], eventId),
        eventWitness: {
          class: "sqlite-attachment-extracted",
          sequence: number(4),
          target,
          columns: [
            "id",
            "seq",
            "workspace_id",
            "actor_user_id",
            "verb",
            "target_type",
            "target_id",
            "payload",
            "channel",
            "created_at",
          ],
          cells,
        },
      };
    record(after, "fingerprints").attachments = { [attachmentHash]: 1 };
    record(after, "allocations").attachments = { [attachmentHash]: attachment };
    record(after, "fingerprints", "events")[sha] = 1;
    record(after, "allocations", "events")[sha] = event;
    (get(after, "operations", "events") as unknown[]).push([
      number(4),
      ["blob", f.workspace],
      ["null"],
    ]);
    record(after, "operations").event_sequence = [[number(1), number(4)]];
    f.audit.documents = { [document]: f.workspace };
    must(f.audit.serial).after = {
      ...must(f.audit.sqlitePreparation).prepared,
      dataset: after,
      checks: { ...must(f.audit.sqlitePreparation).prepared.checks, clockMicros: "60" },
    };
    return {
      ...f,
      after,
      fields,
      cells,
      event,
      extraction,
      attachmentHash,
      sha,
      attachment,
      target,
    };
  };
  expect(() => {
    const f = fixture();
    // The complete allocation includes the ordinary workspace-created event
    // with a NULL witness, independently accounted for by its actual actor.
    expect(
      Object.values(record(f.after, "allocations", "events")).some(
        (row) => get(row, "eventWitness") === null,
      ),
    ).toBe(true);
    allocatedRows(f.before, f.after, f.audit);
  }).not.toThrow();
  const mutations: ((f: ReturnType<typeof fixture>) => void)[] = [
    (f) => {
      f.cells[7] = ["text", "{}"];
    },
    (f) => {
      f.cells[8] = ["text", "system"];
    },
    (f) => {
      f.event.eventWitness.target = "f".repeat(32);
    },
    (f) => {
      f.fields[1] = ["blob", "f".repeat(32)];
    },
    (f) => {
      f.fields[2] = ["blob", "f".repeat(32)];
    },
    (f) => {
      f.fields[3] = ["blob", "a".repeat(32)];
    },
    (f) => {
      f.fields[4] = ["blob", f.reader];
      record(f.attachment).actors = [f.reader];
    },
    (f) => {
      f.fields[5] = ["text", "uploading"];
    },
    (f) => {
      f.fields[6] = ["text", "pending"];
    },
    (f) => {
      f.fields[7] = number(3);
    },
    (f) => {
      f.fields[8] = ["blob", "f".repeat(32)];
    },
    (f) => {
      f.fields[9] = number(100);
    },
    (f) => {
      f.fields[10] = number(51);
    },
    (f) => {
      f.extraction.sha256 = "f".repeat(64);
    },
    (f) => {
      record(f.after, "fingerprints", "attachments")[f.attachmentHash] = 2;
    },
    (f) => {
      record(f.after, "fingerprints", "events")[f.sha] = 2;
    },
    (f) => {
      record(f.before, "allocations").attachments = { old: f.attachment };
    },
    (f) => {
      record(f.after, "allocations", "attachments").duplicate = f.attachment;
    },
    (f) => {
      record(f.after, "allocations", "events").duplicate = f.event;
    },
    (f) => {
      (get(f.after, "operations", "events") as unknown[]).push([
        number(4),
        ["blob", f.workspace],
        ["null"],
      ]);
    },
    (f) => {
      f.after.backend = "libsql-remote";
    },
    (f) => {
      f.audit.sqlitePreparation = undefined;
    },
  ];
  for (const mutate of mutations) {
    const f = fixture();
    mutate(f);
    // Changed producer rows get their honest new fingerprint too; refusal
    // must bind the tuple/payload rather than rely only on a stale hash.
    const changedHash = f.h(f.cells);
    if (changedHash !== f.sha) {
      record(f.after, "fingerprints", "events")[changedHash] = record(
        f.after,
        "fingerprints",
        "events",
      )[f.sha];
      record(f.after, "allocations", "events")[changedHash] = f.event;
      Reflect.deleteProperty(record(f.after, "fingerprints", "events"), f.sha);
      Reflect.deleteProperty(record(f.after, "allocations", "events"), f.sha);
    }
    expect(() => allocatedRows(f.before, f.after, f.audit)).toThrow();
  }
});

test("same sealed readback publication rows explain allocation and event counters without admitting other old-reader effects", async () => {
  const { assertSqlitePreparedPreserved, allocatedRows } = await import("./ui-audit.ts");
  const fixture = (product = false) => {
    const f = sqlitePreparedRecords(),
      selected = must(f.audit.sqlitePreparation),
      rows: Record<string, Record<string, number>> = {},
      eventOperations: Record<string, unknown[]> = {},
      eventIds: Record<string, string> = {},
      actors: Record<string, string> = {};
    const workspace = product ? f.workspace : selected.workspace,
      actor = product ? (Object.keys(f.audit.actors)[0] as string) : f.reader;
    for (const table of product
      ? ["events", "audit_log"]
      : ["document_collab_updates", "document_collab_op_receipts", "events", "audit_log"]) {
      rows[table] = {};
      if (!Object.hasOwn(record(f.before, "fingerprints"), table))
        record(f.before, "fingerprints")[table] = {};
      if (!Object.hasOwn(record(f.before, "allocations"), table))
        record(f.before, "allocations")[table] = {};
      if (!Object.hasOwn(record(f.prepared, "fingerprints"), table))
        record(f.prepared, "fingerprints")[table] = {};
      if (!Object.hasOwn(record(f.prepared, "allocations"), table))
        record(f.prepared, "allocations")[table] = {};
      for (let i = 0; i < 2; i++) {
        const hash = f.h([table, i]),
          id = String(i + 8).repeat(32),
          operation = [number(i + 4), ["blob", workspace], ["blob", actor]];
        actors[hash] = actor;
        rows[table][hash] = 1;
        record(f.prepared, "fingerprints", table)[hash] = 1;
        record(f.prepared, "allocations", table)[hash] = {
          ...f.refs(
            [workspace],
            table === "document_collab_updates" ? [] : [actor],
            table === "events" ? id : null,
          ),
          ...(table === "events" ? { eventWitness: null } : {}),
        };
        if (table === "events") {
          eventOperations[hash] = operation;
          eventIds[hash] = id;
          (get(f.prepared, "operations", "events") as unknown[]).push(operation);
        }
      }
    }
    if (product) {
      const document = "e".repeat(32),
        hash = f.h("product document");
      f.audit.documents = { [document]: workspace };
      record(f.prepared, "fingerprints", "documents")[hash] = 1;
      record(f.prepared, "allocations", "documents")[hash] = f.refs([workspace], [actor], document);
      selected.products = [
        { count: 2, rows, eventOperations, eventIds, actors, workspace, document },
      ];
    } else {
      selected.readback = { count: 2, rows, eventOperations, eventIds };
      selected.state.current[9] = number(4);
      selected.state.after = structuredClone(selected.state.current);
      record(f.prepared, "fingerprints").document_states = { [f.h(selected.state.current)]: 1 };
      record(f.prepared, "allocations", "document_states")[f.h(selected.state.current)] = f.refs([
        selected.workspace,
      ]);
    }
    record(f.prepared, "operations").event_sequence = [[number(1), number(5)]];
    return f;
  };
  for (const product of [false, true])
    for (const phase of ["prepared", "after"] as const) {
      const valid = fixture(product);
      expect(() => {
        if (phase === "prepared")
          assertSqlitePreparedPreserved(valid.before, valid.prepared, valid.audit);
        else allocatedRows(valid.before, valid.prepared, valid.audit, phase);
      }).not.toThrow();
      for (const mutate of [
        (f: ReturnType<typeof fixture>) => {
          const s = must(f.audit.sqlitePreparation);
          must(s.readback ?? s.products?.[0]).eventOperations = {};
        },
        (f: ReturnType<typeof fixture>) => {
          const s = must(f.audit.sqlitePreparation);
          must(s.readback ?? s.products?.[0]).rows.audit_log = {};
        },
        (f: ReturnType<typeof fixture>) => {
          const s = must(f.audit.sqlitePreparation);
          const hash = Object.keys(
            must(s.readback ?? s.products?.[0]).rows.events ?? {},
          )[0] as string;
          (record(f.prepared, "allocations", "events")[hash] as Record_).self = "e".repeat(32);
        },
        (f: ReturnType<typeof fixture>) => {
          const s = must(f.audit.sqlitePreparation);
          const hash = Object.keys(
            must(s.readback ?? s.products?.[0]).rows.audit_log ?? {},
          )[0] as string;
          (record(f.prepared, "allocations", "audit_log")[hash] as Record_).actors = [
            s.preparationActor.id,
          ];
        },
        (f: ReturnType<typeof fixture>) => {
          const s = must(f.audit.sqlitePreparation);
          const hash = f.h("unmatched old-reader event");
          record(f.prepared, "fingerprints", "events")[hash] = 1;
          record(f.prepared, "allocations", "events")[hash] = {
            ...f.refs([s.workspace], [f.reader], "e".repeat(32)),
            eventWitness: null,
          };
        },
      ]) {
        const f = fixture(product);
        mutate(f);
        expect(() => allocatedRows(f.before, f.prepared, f.audit, phase)).toThrow();
      }
    }
});

function preparationNotificationRecords(index: number, late = false) {
  const f = sqlitePreparedRecords(),
    selected = must(f.audit.sqlitePreparation),
    ids = Object.keys(f.audit.actors),
    performer = ids[index] as string,
    recipient = index === 0 ? selected.preparationActor.id : (ids[0] as string),
    target = "a".repeat(31) + String(index),
    eventId = "b".repeat(31) + String(index),
    eventHash = f.h(["accepted event", index]),
    invitationHash = f.h(["accepted invitation", index]),
    columns = [
      "id",
      "workspace_id",
      "user_id",
      "event_id",
      "verb",
      "actor_user_id",
      "target_type",
      "target_id",
      "payload",
      "read_at",
      "archived_at",
      "created_at",
    ];
  const uuid = (id: string) =>
    [id.slice(0, 8), id.slice(8, 12), id.slice(12, 16), id.slice(16, 20), id.slice(20)].join("-");
  selected.prepared.checks.memberships = [selected.preparationActor.id, ...ids].map((id) => {
    const role = id === selected.preparationActor.id || id === ids[0] ? "owner" : "member",
      hash = f.h(["membership", id]);
    record(f.prepared, "fingerprints", "memberships")[hash] = 1;
    record(f.prepared, "allocations", "memberships")[hash] = f.refs([f.workspace], [id]);
    return { workspaceId: uuid(f.workspace), userId: uuid(id), role, rowFingerprint: hash };
  });
  record(f.prepared, "fingerprints", "invitations")[invitationHash] = 1;
  record(f.prepared, "allocations", "invitations")[invitationHash] = {
    ...f.refs([f.workspace], [recipient]),
    invitation: {
      id: target,
      workspace: f.workspace,
      inviter: recipient,
      email: f.audit.actors[performer],
      role: index === 0 ? "owner" : "member",
      acceptedAt: number(30),
      expiresAt: number(1000),
    },
  };
  record(f.prepared, "fingerprints", "events")[eventHash] = 1;
  record(f.prepared, "allocations", "events")[eventHash] = {
    ...f.refs([f.workspace], [performer], eventId),
    eventWitness: {
      class: "workspace-invitation-accepted",
      sequence: number(4),
      workspace: f.workspace,
      actor: performer,
      target,
      email: f.audit.actors[performer],
      createdAt: number(30),
    },
  };
  (get(f.prepared, "operations", "events") as unknown[]).push([
    number(4),
    ["blob", f.workspace],
    ["blob", performer],
  ]);
  record(f.prepared, "operations").event_sequence = [[number(1), number(4)]];
  const cells: unknown[][] = [
    ["blob", "9".repeat(31) + String(index)],
    ["blob", f.workspace],
    ["blob", recipient],
    ["blob", eventId],
    ["text", "invitation.accepted"],
    ["blob", performer],
    ["text", "invitation"],
    ["blob", target],
    ["text", "{}"],
    ["null"],
    ["null"],
    number(35),
  ];
  const sha = f.h(cells),
    notification = {
      ...f.refs([f.workspace], [recipient, performer]),
      events: [eventId],
      notification: { columns, cells, sha256: sha },
    };
  const insert = (dataset: Record_) => {
    record(dataset, "fingerprints", "notifications")[sha] = 1;
    record(dataset, "allocations", "notifications")[sha] = structuredClone(notification);
  };
  if (!late) insert(f.prepared);
  const after = structuredClone(f.prepared);
  if (late) insert(after);
  must(f.audit.serial).after = {
    ...selected.prepared,
    dataset: after,
    checks: { ...selected.prepared.checks, clockMicros: "45" },
  };
  return { ...f, after, sha, eventHash, invitationHash, recipient, performer, index };
}

test("preparation notifications bind all three actual invitation role pairs in prepared/final and late-final delivery", async () => {
  const { assertSqlitePreparedPreserved, allocatedRows } = await import("./ui-audit.ts");
  for (const index of [0, 1, 2]) {
    const f = preparationNotificationRecords(index);
    expect(() => {
      assertSqlitePreparedPreserved(f.before, f.prepared, f.audit);
    }).not.toThrow();
    expect(() => allocatedRows(f.before, f.after, f.audit)).not.toThrow();
    const late = preparationNotificationRecords(index, true);
    expect(() => {
      assertSqlitePreparedPreserved(late.before, late.prepared, late.audit);
    }).not.toThrow();
    expect(() => allocatedRows(late.before, late.after, late.audit)).not.toThrow();
    const missing = structuredClone(f.after);
    record(missing, "fingerprints").notifications = {};
    record(missing, "allocations").notifications = {};
    expect(() => allocatedRows(f.before, missing, f.audit)).toThrow(
      "UI_SQLITE_PREPARATION_NOTIFICATION_CHANGED",
    );
  }
});

test("preparation notification role/source/typed-row changes refuse even when forged row hashes agree", async () => {
  const { assertSqlitePreparedPreserved, allocatedRows } = await import("./ui-audit.ts");
  for (const phase of ["prepared", "after"] as const) {
    for (const change of [
      (cells: unknown[][], f: ReturnType<typeof preparationNotificationRecords>) => {
        cells[2] = ["blob", f.reader];
      },
      (cells: unknown[][], f: ReturnType<typeof preparationNotificationRecords>) => {
        cells[5] = ["blob", must(f.audit.sqlitePreparation).preparationActor.id];
      },
      (cells: unknown[][], f: ReturnType<typeof preparationNotificationRecords>) => {
        cells[1] = ["blob", must(f.audit.sqlitePreparation).workspace];
      },
      (cells: unknown[][]) => {
        cells[3] = ["blob", "e".repeat(32)];
      },
      (cells: unknown[][]) => {
        cells[7] = ["blob", "e".repeat(32)];
      },
      (cells: unknown[][]) => {
        cells[4] = ["text", "comment.created"];
      },
      (cells: unknown[][]) => {
        cells[6] = ["text", "user"];
      },
      (cells: unknown[][]) => {
        cells[8] = ["text", '{"unexpected":true}'];
      },
      (cells: unknown[][]) => {
        cells[9] = number(35);
      },
      (cells: unknown[][]) => {
        cells[10] = number(35);
      },
      (cells: unknown[][]) => {
        cells[11] = number(29);
      },
      (cells: unknown[][]) => {
        cells[11] = number(1045);
      },
      (cells: unknown[][]) => {
        cells[11] = ["text", "35"];
      },
      (cells: unknown[][]) => {
        cells[0] = ["blob", "0".repeat(32)];
      },
    ]) {
      const f = preparationNotificationRecords(0, phase === "after"),
        dataset = phase === "prepared" ? f.prepared : f.after,
        refs = record(dataset, "allocations", "notifications", f.sha),
        row = record(refs, "notification"),
        cells = get(row, "cells") as unknown[][];
      change(cells, f);
      const hash = f.h(cells);
      row.sha256 = hash;
      Reflect.deleteProperty(record(dataset, "fingerprints", "notifications"), f.sha);
      Reflect.deleteProperty(record(dataset, "allocations", "notifications"), f.sha);
      record(dataset, "fingerprints", "notifications")[hash] = 1;
      record(dataset, "allocations", "notifications")[hash] = refs;
      expect(() => {
        if (phase === "prepared") assertSqlitePreparedPreserved(f.before, dataset, f.audit);
        else allocatedRows(f.before, dataset, f.audit);
      }).toThrow();
    }
    for (const mutate of [
      (f: ReturnType<typeof preparationNotificationRecords>, d: Record_) => {
        record(d, "fingerprints", "notifications")[f.sha] = 2;
      },
      (f: ReturnType<typeof preparationNotificationRecords>, d: Record_) => {
        record(d, "allocations", "notifications", f.sha).actors = [f.performer, f.recipient];
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(f.prepared, "allocations", "events", f.eventHash).eventWitness = {};
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(f.prepared, "fingerprints").events = {};
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(f.prepared, "fingerprints").invitations = {};
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(f.prepared, "allocations", "invitations", f.invitationHash, "invitation").inviter =
          f.reader;
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(f.prepared, "allocations", "invitations", f.invitationHash, "invitation").role =
          "admin";
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(f.prepared, "allocations", "invitations", f.invitationHash, "invitation").email =
          "foreign";
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(
          f.prepared,
          "allocations",
          "invitations",
          f.invitationHash,
          "invitation",
        ).acceptedAt = number(19);
      },
      (f: ReturnType<typeof preparationNotificationRecords>) => {
        record(f.prepared, "allocations", "invitations", f.invitationHash, "invitation").expiresAt =
          number(30);
      },
      (f: ReturnType<typeof preparationNotificationRecords>, d: Record_) => {
        const refs = structuredClone(record(d, "allocations", "notifications", f.sha)),
          row = record(refs, "notification");
        (get(row, "cells") as unknown[][])[0] = ["blob", "8".repeat(32)];
        const hash = f.h(row.cells);
        row.sha256 = hash;
        record(d, "allocations", "notifications")[hash] = refs;
        record(d, "fingerprints", "notifications")[hash] = 1;
      },
    ]) {
      const f = preparationNotificationRecords(0, phase === "after"),
        dataset = phase === "prepared" ? f.prepared : f.after;
      mutate(f, dataset);
      expect(() => {
        if (phase === "prepared") assertSqlitePreparedPreserved(f.before, dataset, f.audit);
        else allocatedRows(f.before, dataset, f.audit);
      }).toThrow();
    }
  }
});
