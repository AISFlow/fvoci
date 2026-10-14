import { expect, test } from "bun:test";
import type { DriverReceipt } from "../types.ts";
import type { ConnectionFacts } from "./equal-work-policy.ts";
import {
  cgroupCounters,
  comparePair,
  metrics,
  phases,
  validateEqualReport,
  validateSamples,
  workload,
  workloadHash,
  equalTitle,
  type Sample,
} from "./equal-work.ts";

const rawMetrics = [
  "fvoci_process_resident_memory_bytes 1048576",
  "fvoci_collab_helper_resident_memory_bytes 0",
  'fvoci_db_pool_connections{state="idle"} 2',
  'fvoci_db_pool_connections{state="active"} 0',
  "fvoci_db_pool_max_connections 5",
  "fvoci_task_stream_subscribers 0",
].join("\n");
const rawCgroup = "usage_usec 10\nuser_usec 7\nsystem_usec 3\n1024\n2048\nanon 512\nfile 256\n";
const samples = (): Sample[] =>
  phases.map((phase, index) => ({
    phase,
    workloadHash,
    monotonicMs: index + 1,
    cpuUsec: index * 100,
    memoryCurrentBytes: 1024,
    lifetimePeakBytes: 2048,
    anonRssProxyBytes: 512,
    metrics: metrics(rawMetrics),
  }));
function receipt(flow: "on" | "off"): DriverReceipt {
  const connections = (closed: boolean): ConnectionFacts => ({
    opened: flow === "on" ? 2 : 1,
    collabOpened: flow === "on" ? 1 : 0,
    closeEvents: closed ? (flow === "on" ? 2 : 1) : 0,
    active: closed ? 0 : flow === "on" ? 2 : 1,
    collabActive: closed || flow === "off" ? 0 : 1,
    contextClosed: closed,
  });
  return {
    source: "a".repeat(40),
    tree: "b".repeat(40),
    root_owner: "fixture",
    purpose: "equal-work",
    selected_flow: flow,
    final_exit_code: 0,
    actual_browser_tests: 1,
    retries: 0,
    cleanup_errors: [],
    owned_container_absent: true,
    owned_loopback_port_closed: true,
    recorded_process_identities_retired: true,
    equal_work: {
      inputs: {
        source: "a".repeat(40),
        tree: "b".repeat(40),
        workloadHash,
        sourceInputsHash: "e".repeat(64),
        image: "fixture@sha256:" + "f".repeat(64),
        browser: {
          bun: { path: "/fixture/bun", sha256: "1".repeat(64) },
          chromium: { path: "/fixture/chromium", sha256: "2".repeat(64) },
          chromium_directory_files: { chromium: "2".repeat(64) },
        },
        native: { server: "c".repeat(64) },
        assets: { app: "d".repeat(64) },
      },
      samples: samples(),
      browser: {
        backend: "sqlite",
        flow,
        workloadHash,
        warmSaves: 1,
        warmReadbacks: 1,
        edits: 3,
        acks: 3,
        freshReadbacks: 3,
        contextsClosed: true,
        resources: [0, 1, 2].map((i) => ({
          phase: `ack-${String(i)}`,
          ownerConnections: connections(false),
          observedRoomKeys: flow === "on" ? 1 : 0,
        })),
        ownerAtStart: connections(false),
        ownerAfterClose: connections(true),
        freshConnections: [workload.warm, ...workload.edits].map((text) => ({
          text,
          beforeClose: connections(false),
          afterClose: connections(true),
        })),
        texts: workload.edits,
        ackLatencyMs: [3, 2, 4],
        ackIds: ["a", "b", "c"].map((v) => v.repeat(8) + "-0000-4000-8000-000000000000"),
      },
    },
  };
}
test("unreadable/absent/NaN/duplicate counters cannot become zero-cost success", () => {
  expect(cgroupCounters(rawCgroup).cpuUsec).toBe(10);
  expect(metrics(rawMetrics).fvoci_collab_helper_resident_memory_bytes).toBe(0);
  for (const raw of [
    "",
    rawCgroup.replace("anon 512", "anon 0"),
    rawCgroup.replace("usage_usec 10\n", ""),
    rawCgroup + "usage_usec 11\n",
    rawCgroup.replace("1024", "NaN"),
  ])
    expect(() => cgroupCounters(raw)).toThrow();
  for (const raw of [
    "",
    rawMetrics.replace("1048576", "NaN"),
    rawMetrics.replace("1048576", "0"),
    rawMetrics + "\nfvoci_process_resident_memory_bytes 12",
  ])
    expect(() => metrics(raw)).toThrow();
});
test("missing, duplicate, unordered, different-workload or reversed CPU samples refuse", () => {
  validateSamples(samples());
  const faults: ((s: Sample[]) => void)[] = [
    (s) => {
      s.pop();
    },
    (s) => {
      s.push({ ...(s[0] as Sample) });
    },
    (s) => {
      s.reverse();
    },
    (s) => {
      (s[2] as Sample).workloadHash = "e".repeat(64);
    },
    (s) => {
      (s[3] as Sample).cpuUsec = 0;
    },
    (s) => {
      (s[2] as Sample).metrics = {};
    },
    (s) => {
      (s[2] as Sample).anonRssProxyBytes = 0;
    },
  ];
  for (const fault of faults) {
    const s = samples();
    fault(s);
    expect(() => {
      validateSamples(s);
    }).toThrow();
  }
});
test("equal costs are observations, independent of which mode is faster", () => {
  expect(comparePair(receipt("on"), receipt("off"))).toMatchObject({
    backend: "sqlite",
    on: { cpuDeltaUsec: 600 },
    off: { cpuDeltaUsec: 600 },
  });
});
test("total sockets are measured separately; collab ON1/OFF0 and each context retirement remain mandatory", () => {
  expect(() => {
    comparePair(receipt("on"), receipt("off"));
  }).not.toThrow();
  type BrowserFacts = {
    resources: { ownerConnections: ConnectionFacts }[];
    ownerAfterClose: ConnectionFacts;
    freshConnections: { beforeClose: ConnectionFacts; afterClose: ConnectionFacts }[];
  };
  const faults: ((facts: BrowserFacts) => void)[] = [
    (f) => {
      (f.resources[0] as BrowserFacts["resources"][number]).ownerConnections.collabActive = 1;
    },
    (f) => {
      f.ownerAfterClose.contextClosed = false;
    },
    (f) => {
      f.freshConnections.pop();
    },
    (f) => {
      (f.freshConnections[0] as BrowserFacts["freshConnections"][number]).afterClose.active = 1;
    },
  ];
  for (const fault of faults) {
    const off = receipt("off");
    fault((off.equal_work as { browser: BrowserFacts }).browser);
    expect(() => {
      comparePair(receipt("on"), off);
    }).toThrow();
  }
});
test("different inputs/workload, zero tests, missing ACK/readback, failed/cancelled body and failed cleanup block the pair", () => {
  type Measurement = {
    inputs: Record<string, unknown>;
    samples: Sample[];
    browser: Record<string, unknown>;
  };
  const faults: ((r: DriverReceipt) => void)[] = [
    (r) => {
      r.final_exit_code = 1;
    },
    (r) => {
      r.final_exit_code = 130;
    },
    (r) => {
      r.actual_browser_tests = 0;
    },
    (r) => {
      r.cleanup_errors = ["cleanup failed"];
    },
    (r) => {
      r.owned_container_absent = false;
    },
    (r) => {
      r.owned_loopback_port_closed = false;
    },
    (r) => {
      r.original_driver_failure = { type: "failure" };
    },
    (r) => {
      (r.equal_work as Measurement).inputs.native = { server: "changed" };
    },
    (r) => {
      (r.equal_work as Measurement).browser.acks = 0;
    },
    (r) => {
      (r.equal_work as Measurement).browser.freshReadbacks = 0;
    },
    (r) => {
      (r.equal_work as Measurement).browser.ackLatencyMs = [1, 2];
    },
    (r) => {
      (r.equal_work as Measurement).browser.workloadHash = "different";
    },
    (r) => {
      (r.equal_work as Measurement).samples.pop();
    },
  ];
  for (const fault of faults) {
    const off = receipt("off");
    fault(off);
    expect(() => comparePair(receipt("on"), off)).toThrow();
  }
});
test("zero-case, skipped or absent reporter evidence is never a measurement PASS", () => {
  for (const report of [
    null,
    {},
    {
      config: {
        workers: 1,
        metadata: { selectedBackend: "sqlite", selectedFlow: "on", purpose: "equal-work" },
      },
      errors: [],
      stats: { expected: 0, unexpected: 0, skipped: 0, flaky: 0 },
      suites: [],
    },
  ])
    expect(() => validateEqualReport(report, "sqlite", "on")).toThrow();
});

test("the actual Playwright producer metadata requires exactly one actual worker", () => {
  const facts = (receipt("on").equal_work as { browser: Record<string, unknown> }).browser;
  const report = {
    config: {
      workers: 1,
      metadata: {
        selectedBackend: "sqlite",
        selectedFlow: "on",
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
            file: "e2e-pending/v060-equal-work.spec.ts",
            title: equalTitle,
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
                        body: Buffer.from(JSON.stringify(facts)).toString("base64"),
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
  expect(validateEqualReport(report, "sqlite", "on")).toEqual(facts);
  for (const actualWorkers of [undefined, 0, 2]) {
    const changed = structuredClone(report);
    Reflect.set(changed.config.metadata, "actualWorkers", actualWorkers);
    expect(() => {
      validateEqualReport(changed, "sqlite", "on");
    }).toThrow();
  }
  const extra = structuredClone(report);
  Reflect.set(extra.config.metadata, "unbound", true);
  expect(() => {
    validateEqualReport(extra, "sqlite", "on");
  }).toThrow();
});
