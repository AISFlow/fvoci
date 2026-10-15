import { expect, test } from "bun:test";
import { strict as assert } from "node:assert";
import {
  equalTitle,
  metrics,
  phases,
  workload,
  workloadHash,
} from "../selected-backend-ci/drivers/equal-work.ts";
import type { ConnectionFacts } from "../selected-backend-ci/drivers/equal-work-policy.ts";
import {
  compareHostedPair,
  measurementContainer,
  type HostedCase,
  type HostedNativeContext,
  type HostedNativeValidator,
  type HostedSeed,
} from "./ui-equal-work.ts";
import { record, valueDigest, type Record_ } from "./ui-common.ts";

const id = (value: number) => value.toString(16).padStart(8, "0") + "-0000-4000-8000-000000000000";
const bare = (value: string) => value.replaceAll("-", "");
function facts(flow: "on" | "off"): Record_ {
  const connections = (closed: boolean): ConnectionFacts => ({
    opened: flow === "on" ? 2 : 1,
    collabOpened: flow === "on" ? 1 : 0,
    closeEvents: closed ? (flow === "on" ? 2 : 1) : 0,
    active: closed ? 0 : flow === "on" ? 2 : 1,
    collabActive: closed || flow === "off" ? 0 : 1,
    contextClosed: closed,
  });
  return {
    backend: "libsql-remote",
    flow,
    workloadHash,
    warmSaves: 1,
    warmReadbacks: 1,
    edits: 3,
    acks: 3,
    freshReadbacks: 3,
    contextsClosed: true,
    texts: workload.edits,
    ackLatencyMs: [1, 2, 3],
    ackIds: [id(100), id(101), id(102)],
    ownerAtStart: connections(false),
    ownerAfterClose: connections(true),
    resources: [0, 1, 2].map((i) => ({
      phase: `ack-${String(i)}`,
      observedRoomKeys: flow === "on" ? 1 : 0,
      ownerConnections: connections(false),
    })),
    freshConnections: [workload.warm, ...workload.edits].map((text) => ({
      text,
      beforeClose: connections(false),
      afterClose: connections(true),
    })),
  };
}
function reporter(browser: Record_): unknown {
  return {
    config: {
      workers: 1,
      metadata: {
        selectedBackend: "libsql-remote",
        selectedFlow: browser.flow,
        purpose: "equal-work",
        actualWorkers: 1,
      },
    },
    errors: [],
    stats: { expected: 1, unexpected: 0, flaky: 0, skipped: 0 },
    suites: [
      {
        specs: [
          {
            title: equalTitle,
            file: "e2e-pending/v060-equal-work.spec.ts",
            ok: true,
            tests: [
              {
                expectedStatus: "passed",
                results: [
                  {
                    status: "passed",
                    retry: 0,
                    errors: [],
                    attachments: [
                      {
                        name: "equal-work.json",
                        contentType: "application/json",
                        body: Buffer.from(JSON.stringify(browser)).toString("base64"),
                      },
                    ],
                  },
                ],
              },
            ],
          },
        ],
      },
    ],
  };
}
function one(flow: "on" | "off", previous?: HostedCase): HostedCase {
  const index = flow === "on" ? 1 : 2;
  const seed: HostedSeed = {
    namespace: "tui-" + String(index).repeat(20),
    owner: id(index * 10),
    member: id(index * 10 + 1),
    workspace: id(index * 10 + 2),
    document: id(index * 10 + 3),
    logical: {
      users: workload.users,
      documents: workload.documents,
      roles: ["owner", "member"],
      initialBody: "",
      preparation: workload.preparation,
    },
  };
  // Opaque native observations are forwarded to the parent's maintained
  // auditor. This fixture is not native DB or quiescence acceptance evidence.
  const before = structuredClone(
    previous?.evidence.after ?? {
      schema: 1,
      backend: "libsql-remote",
      ledger: ["native ledger"],
      fingerprints: { api_tokens: { ["a".repeat(64)]: 1 } },
      operations: {
        event_sequence: [
          [
            ["integer", "1"],
            ["integer", "0"],
          ],
        ],
      },
    },
  );
  const after = structuredClone(before);
  record(after, "fingerprints")[seed.namespace] = { [String(index).repeat(64)]: 1 };
  record(after, "operations").event_sequence = [
    [
      ["integer", "1"],
      ["integer", String(index)],
    ],
  ];
  const serialChecksBefore = structuredClone(
    previous?.evidence.serialChecksAfter ?? {
      schema: 1,
      outbox: [],
      background: [],
      revisionCandidates: [],
    },
  );
  const serialChecksAfter = {
    ...structuredClone(serialChecksBefore),
    eventHead: String(index),
    outbox: [
      {
        consumer: "mail",
        immutableSha256: "a".repeat(64),
        rowSha256: "b".repeat(64),
        lastSeq: String(index),
        leaseOwner: null,
        leaseUntilMicros: null,
        updatedAtMicros: String(index * 100),
      },
    ],
  };
  const browser = facts(flow);
  const live = metrics(
    'fvoci_process_resident_memory_bytes 1024\nfvoci_collab_helper_resident_memory_bytes 0\nfvoci_db_pool_connections{state="idle"} 2\nfvoci_db_pool_connections{state="active"} 0\nfvoci_db_pool_max_connections 5\nfvoci_task_stream_subscribers 0',
  );
  const containerId = String(index).repeat(64),
    serverPid = 300 + index,
    maintenanceIdentity = { pid: 100 + index, startTicks: String(200 + index) };
  return {
    measurement: {
      inputs: {
        source: "a".repeat(40),
        tree: "b".repeat(40),
        sourceInputsHash: "c".repeat(64),
        image: "fixture@sha256:" + "d".repeat(64),
        native: { server: "e".repeat(64) },
        assets: { app: "f".repeat(64) },
        browser: {
          bun: { path: "/fixture/bun", sha256: "1".repeat(64) },
          chromium: { path: "/fixture/chromium", sha256: "2".repeat(64) },
          chromium_directory_files: { chromium: "2".repeat(64) },
        },
        workloadHash,
        targetSha256: "3".repeat(64),
        configSha256: "4".repeat(64),
        logical: seed.logical,
      },
      seed,
      flow,
      containerId,
      serverPid,
      maintenanceIdentity,
      browserAllocation: structuredClone(seed),
      report: reporter(browser),
      browser,
      samples: phases.map((phase, i) => ({
        phase,
        workloadHash,
        monotonicMs: i + 1,
        cpuUsec: i * 100,
        memoryCurrentBytes: 1024,
        lifetimePeakBytes: 2048,
        anonRssProxyBytes: 512,
        metrics: live,
      })),
    },
    evidence: {
      before,
      after,
      serialChecksBefore,
      serialChecksAfter,
      operationWitnesses: {
        namespaces: [seed.namespace],
        actors: {
          [bare(seed.owner)]: seed.namespace + "-owner@example.invalid",
          [bare(seed.member)]: seed.namespace + "-member@example.invalid",
        },
        workspaces: { [bare(seed.workspace)]: seed.namespace },
        documents: { [bare(seed.document)]: bare(seed.workspace) },
        targetSha256: "3".repeat(64),
        servers: [
          {
            identity: structuredClone(maintenanceIdentity),
            targetSha256: "3".repeat(64),
            receipts: [],
          },
        ],
        serverStarts: 1,
        observedFences: [],
      },
    },
    closure: {
      containerId,
      serverPid,
      serverExit: 0,
      forced: false,
      containerAbsent: true,
      portClosed: true,
      recordedIdentitiesRetired: true,
      cleanupErrors: [],
    },
  };
}
const pair = (): [HostedCase, HostedCase] => {
  const on = one("on");
  return [on, one("off", on)];
};
// Test-only observer. Its successful return proves adapter behavior only;
// production must supply the real whole-native auditrail and run it on target.
const observeOnly: HostedNativeValidator = () => {};

test("host process handles cannot select a sampler or claim an owned container", () => {
  for (const containerId of [undefined, "", "short", "x".repeat(64)])
    expect(() => measurementContainer({ containerId })).toThrow();
  expect(measurementContainer({ containerId: "a".repeat(64) })).toBe("a".repeat(64));
});
test("truthful cumulative physical snapshots and the unchanged equal-work workload reach the parent validator", async () => {
  const [on, off] = pair(),
    seen: HostedNativeContext[] = [];
  const result = await compareHostedPair(on, off, (context) => {
    seen.push(context);
  });
  expect(result).toMatchObject({
    comparison: { backend: "libsql-remote", on: { cpuDeltaUsec: 600 }, off: { cpuDeltaUsec: 600 } },
    physicalDatasets: {
      on: { before: valueDigest(on.evidence.before), after: valueDigest(on.evidence.after) },
      off: { before: valueDigest(on.evidence.after), after: valueDigest(off.evidence.after) },
    },
    backendDatabaseCpu: "NOTRUN",
  });
  expect(valueDigest(on.evidence.before)).not.toBe(valueDigest(off.evidence.before));
  expect(seen).toEqual(
    [on, off].map((v) => ({
      ...v.evidence,
      inputs: v.measurement.inputs,
      seed: v.measurement.seed,
      flow: v.measurement.flow,
      samples: v.measurement.samples,
      closure: v.closure,
    })),
  );
});
test("required validator, thrown native rejection and returned verdict cannot qualify a pair", async () => {
  const [on, off] = pair();
  await assert.rejects(
    Reflect.apply(compareHostedPair, undefined, [on, off]) as Promise<object>,
    /actual parent native validator required/,
  );
  for (const flow of ["on", "off"]) {
    await assert.rejects(
      compareHostedPair(on, off, async (context) => {
        await Promise.resolve();
        if (context.flow === flow) throw new Error("native row/counter/quiescence rejection");
      }),
      /native row\/counter\/quiescence rejection/,
    );
  }
  for (const verdict of [true, false, "a".repeat(64)])
    await assert.rejects(
      Reflect.apply(compareHostedPair, undefined, [on, off, () => verdict]) as Promise<object>,
      /native validator must reject by throwing/,
    );
});
test("comparison waits for both native validations and keeps the supplied case immutable", async () => {
  const [on, off] = pair(),
    original = structuredClone([on, off]);
  const completed: string[] = [];
  await compareHostedPair(on, off, async (context) => {
    await Promise.resolve();
    completed.push(context.flow);
  });
  expect(completed).toEqual(["on", "off"]);
  expect([on, off]).toEqual(original);
});
const faults: [string, (value: HostedCase) => void][] = [
  [
    "target",
    (v) => {
      v.measurement.inputs.targetSha256 = "5".repeat(64);
    },
  ],
  [
    "config",
    (v) => {
      v.measurement.inputs.configSha256 = "5".repeat(64);
    },
  ],
  [
    "source",
    (v) => {
      v.measurement.inputs.source = "5".repeat(40);
    },
  ],
  [
    "tree",
    (v) => {
      v.measurement.inputs.tree = "5".repeat(40);
    },
  ],
  [
    "native",
    (v) => {
      v.measurement.inputs.native.server = "5".repeat(64);
    },
  ],
  [
    "assets",
    (v) => {
      v.measurement.inputs.assets.app = "5".repeat(64);
    },
  ],
  [
    "image",
    (v) => {
      v.measurement.inputs.image = "fixture@sha256:" + "5".repeat(64);
    },
  ],
  [
    "browser Bun",
    (v) => {
      v.measurement.inputs.browser.bun.sha256 = "5".repeat(64);
    },
  ],
  [
    "browser Chromium",
    (v) => {
      v.measurement.inputs.browser.chromium.sha256 = "5".repeat(64);
    },
  ],
  [
    "logical seed",
    (v) => {
      Reflect.set(v.measurement.seed.logical, "users", 1);
    },
  ],
  [
    "browser allocation",
    (v) => {
      v.measurement.browserAllocation.document = id(999);
    },
  ],
  [
    "normal exit",
    (v) => {
      v.closure.serverExit = 1;
    },
  ],
  [
    "forced exit",
    (v) => {
      v.closure.forced = true;
    },
  ],
  [
    "container binding",
    (v) => {
      v.closure.containerId = "f".repeat(64);
    },
  ],
  [
    "absent container",
    (v) => {
      v.closure.containerAbsent = false;
    },
  ],
  [
    "port closure",
    (v) => {
      v.closure.portClosed = false;
    },
  ],
  [
    "identity retirement",
    (v) => {
      v.closure.recordedIdentitiesRetired = false;
    },
  ],
  [
    "maintenance identity",
    (v) => {
      v.measurement.maintenanceIdentity.startTicks = "999";
    },
  ],
  [
    "cleanup",
    (v) => {
      v.closure.cleanupErrors.push("cleanup failed");
    },
  ],
  [
    "samples",
    (v) => {
      v.measurement.samples.pop();
    },
  ],
  [
    "ACK count",
    (v) => {
      v.measurement.browser.acks = 0;
      v.measurement.report = reporter(v.measurement.browser);
    },
  ],
  [
    "fresh reads",
    (v) => {
      v.measurement.browser.freshReadbacks = 2;
      v.measurement.report = reporter(v.measurement.browser);
    },
  ],
  [
    "reporter count",
    (v) => {
      record(v.measurement.report, "stats").expected = 0;
    },
  ],
  [
    "before snapshot continuity",
    (v) => {
      v.evidence.before.extra = "changed";
    },
  ],
];
for (const [name, fault] of faults)
  test(`${name} refuses hosted pair`, async () => {
    const [on, off] = pair();
    fault(off);
    await assert.rejects(compareHostedPair(on, off, observeOnly));
  });
test("reused namespace/container/actor and reversed flows refuse", async () => {
  for (const fault of [
    (on: HostedCase, off: HostedCase) => {
      off.measurement.seed.namespace = on.measurement.seed.namespace;
    },
    (on: HostedCase, off: HostedCase) => {
      off.measurement.containerId = on.measurement.containerId;
    },
    (on: HostedCase, off: HostedCase) => {
      off.measurement.seed.owner = on.measurement.seed.owner;
    },
    (_on: HostedCase, off: HostedCase) => {
      off.measurement.flow = "on";
    },
  ]) {
    const [on, off] = pair();
    fault(on, off);
    await assert.rejects(compareHostedPair(on, off, observeOnly));
  }
});
