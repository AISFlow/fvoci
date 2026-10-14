// A measurement consumer for the existing selected normal-server lifecycle.
// The driver owns Docker observation; the browser receives only fixed marker
// replies, never Docker authority, owner credentials or a process-memory probe.
import { deepEquals } from "bun";
import { strict as assert } from "node:assert";
import { existsSync, linkSync, mkdirSync, readdirSync, unlinkSync, watch } from "node:fs";
import { get } from "node:http";
import { join } from "node:path";
import { digest, sha } from "../io.ts";
import type { Browser, Bundle, DriverReceipt, Inputs, Web } from "../types.ts";
import { command, privateWrite, readText, type Command, type Completed } from "./common.ts";

import {
  equalTitle,
  phases,
  workload,
  workloadHash,
  type ConnectionFacts,
} from "./equal-work-policy.ts";
export { equalSpec, equalTitle, phases, workload, workloadHash } from "./equal-work-policy.ts";
const integer = (text: string) => {
  assert.ok(/^[0-9]+$/.test(text));
  const value = Number(text);
  assert.ok(Number.isSafeInteger(value) && value >= 0);
  return value;
};
export interface Counters {
  cpuUsec: number;
  memoryCurrentBytes: number;
  lifetimePeakBytes: number;
  anonRssProxyBytes: number;
}
export function cgroupCounters(raw: string): Counters {
  const lines = raw.trim().split("\n");
  const scalar = lines.filter((line) => /^[0-9]+$/.test(line));
  const value = (key: string) => {
    const found = lines.filter((line) => line.startsWith(key + " "));
    assert.ok(found.length === 1, "missing or duplicate cgroup counter");
    return integer((found[0] as string).slice(key.length + 1));
  };
  assert.ok(scalar.length === 2, "memory.current and memory.peak required");
  const counters = {
    cpuUsec: value("usage_usec"),
    memoryCurrentBytes: integer(scalar[0] as string),
    lifetimePeakBytes: integer(scalar[1] as string),
    anonRssProxyBytes: value("anon"),
  };
  assert.ok(
    counters.memoryCurrentBytes > 0 && counters.lifetimePeakBytes >= counters.memoryCurrentBytes,
  );
  assert.ok(counters.anonRssProxyBytes > 0, "unobservable RSS is not zero");
  return counters;
}
const metricNames = [
  "fvoci_process_resident_memory_bytes",
  "fvoci_collab_helper_resident_memory_bytes",
  'fvoci_db_pool_connections{state="idle"}',
  'fvoci_db_pool_connections{state="active"}',
  "fvoci_db_pool_max_connections",
  "fvoci_task_stream_subscribers",
] as const;
export function metrics(raw: string): Record<string, number> {
  return Object.fromEntries(
    metricNames.map((key) => {
      const lines = raw.split("\n").filter((line) => line.startsWith(key + " "));
      assert.ok(lines.length === 1, "required live metric missing");
      const value = Number((lines[0] as string).slice(key.length + 1));
      assert.ok(Number.isFinite(value) && value >= 0, "unobservable metric is not zero");
      if (key === metricNames[0]) assert.ok(value > 0);
      return [key, value];
    }),
  );
}
function scrape(base: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const request = get(base + "/metrics", { timeout: 10_000 }, (response) => {
      const chunks: Buffer[] = [];
      response.on("data", (chunk: Buffer) => chunks.push(chunk));
      response.on("error", () => {
        reject(new Error("equal-work metrics unavailable"));
      });
      response.on("end", () => {
        if (response.statusCode !== 200) reject(new Error("equal-work metrics refused"));
        else resolve(Buffer.concat(chunks).toString("utf8"));
      });
    });
    request.on("error", () => {
      reject(new Error("equal-work metrics unavailable"));
    });
    request.on("timeout", () => request.destroy(new Error("equal-work metrics unavailable")));
  });
}
export interface Sample extends Counters {
  phase: string;
  workloadHash: string;
  monotonicMs: number;
  metrics: Record<string, number>;
}
export function validateSamples(samples: Sample[]): void {
  assert.ok(
    deepEquals(
      samples.map((s) => s.phase),
      phases,
    ),
    "missing, duplicate or unordered samples",
  );
  for (const [i, sample] of samples.entries()) {
    assert.ok(sample.workloadHash === workloadHash);
    assert.ok(Number.isFinite(sample.monotonicMs));
    assert.ok(sample.memoryCurrentBytes > 0 && sample.anonRssProxyBytes > 0);
    assert.ok(sample.lifetimePeakBytes >= sample.memoryCurrentBytes);
    assert.ok(Number.isSafeInteger(sample.cpuUsec) && sample.cpuUsec >= 0);
    assert.ok(deepEquals(Object.keys(sample.metrics).sort(), [...metricNames].sort()));
    for (const value of Object.values(sample.metrics))
      assert.ok(Number.isFinite(value) && value >= 0);
    assert.ok((sample.metrics[metricNames[0]] as number) > 0);
    if (i > 0) {
      assert.ok(sample.monotonicMs > (samples[i - 1] as Sample).monotonicMs);
      assert.ok(sample.cpuUsec >= (samples[i - 1] as Sample).cpuUsec);
    }
  }
}
// An ordered file handshake keeps setup/warm work out of the measured window.
// The existing command owns browser cancellation, timeout and descendant reap.
export async function measureBrowser(
  directory: string,
  container: string,
  base: string,
  browser: () => Promise<Completed>,
  run: Command = command,
): Promise<{ result: Completed; samples: Sample[] }> {
  const namespace = await run([
    "docker",
    "inspect",
    "--format",
    "{{.HostConfig.CgroupnsMode}}",
    container,
  ]);
  assert.ok(
    namespace.stdout.trim() === "private",
    "observer must read this owned container cgroup",
  );
  mkdirSync(directory, { mode: 0o700 });
  const samples: Sample[] = [],
    hashes: Record<string, string> = {};
  let failure: unknown;
  let pending = Promise.resolve();
  const publish = (path: string, value: unknown) => {
    privateWrite(path + ".pending", value);
    linkSync(path + ".pending", path);
    unlinkSync(path + ".pending");
  };
  const consume = async () => {
    while (samples.length < phases.length) {
      const index = samples.length;
      const name = `${String(index)}.request.json`,
        path = join(directory, name);
      if (!existsSync(path)) {
        assert.ok(
          !readdirSync(directory).some((n) => /\.request\.json$/.test(n) && !(n in hashes)),
          "out of order marker",
        );
        return;
      }
      const marker: unknown = JSON.parse(readText(path));
      assert.ok(deepEquals(marker, { phase: phases[index], workloadHash }));
      hashes[name] = sha(path);
      const raw = await run([
        "docker",
        "exec",
        container,
        "cat",
        "/sys/fs/cgroup/cpu.stat",
        "/sys/fs/cgroup/memory.current",
        "/sys/fs/cgroup/memory.peak",
        "/sys/fs/cgroup/memory.stat",
      ]);
      const live = await scrape(base);
      const sample = {
        phase: phases[index] as string,
        workloadHash,
        monotonicMs: performance.now(),
        ...cgroupCounters(raw.stdout),
        metrics: metrics(live),
      };
      samples.push(sample);
      privateWrite(join(directory, `${String(index)}.sample.json`), sample);
      publish(join(directory, `${String(index)}.reply.json`), {
        phase: sample.phase,
        workloadHash,
      });
    }
  };
  const changed = () => {
    pending = pending.then(async () => {
      if (failure !== undefined) return;
      try {
        await consume();
      } catch (error) {
        failure = error;
        publish(join(directory, "failed.json"), { failed: true });
      }
    });
  };
  const watcher = watch(directory, changed);
  try {
    const result = await browser();
    changed();
    await pending;
    if (failure !== undefined)
      throw failure instanceof Error ? failure : new Error("equal-work collector failed");
    assert.ok(result.returncode === 0, "browser failure cannot qualify costs");
    validateSamples(samples);
    assert.ok(
      deepEquals(
        readdirSync(directory)
          .filter((n) => n.endsWith(".request.json"))
          .sort(),
        Object.keys(hashes).sort(),
      ),
    );
    for (const [name, hash] of Object.entries(hashes))
      assert.ok(sha(join(directory, name)) === hash);
    return { result, samples };
  } finally {
    watcher.close();
    await pending;
  }
}
export function measurementInputs(
  before: Inputs,
  bundle: Bundle,
  web: Web,
  browser: Browser,
  image: string,
): object {
  return {
    source: before.head,
    tree: before.tree,
    workloadHash,
    image,
    sourceInputsHash: digest(JSON.stringify(before)),
    native: Object.fromEntries(Object.entries(bundle.binaries).map(([p, b]) => [p, b.sha256])),
    assets: web.dist_files,
    browser,
  };
}
export function validateEqualReport(
  report: unknown,
  backend: string,
  flow: string,
): Record<string, unknown> {
  const r = report as {
    config: { workers: number; metadata: unknown };
    errors: unknown[];
    stats: Record<string, unknown>;
    suites: {
      specs: {
        title: string;
        file: string;
        ok: unknown;
        tests: {
          expectedStatus: string;
          results: {
            status: string;
            retry: number;
            errors: unknown[];
            attachments: { name: string; contentType: string; body: string }[];
          }[];
        }[];
      }[];
    }[];
  };
  assert.ok(r.config.workers === 1);
  assert.ok(
    deepEquals(r.config.metadata, {
      selectedBackend: backend,
      selectedFlow: flow,
      purpose: "equal-work",
    }),
  );
  assert.ok(deepEquals(r.errors, []) && r.stats.expected === 1);
  assert.ok(["unexpected", "flaky", "skipped"].every((key) => r.stats[key] === 0));
  assert.ok(r.suites.length === 1);
  const specs = r.suites[0]?.specs;
  assert.ok(specs?.length === 1);
  const spec = specs[0];
  assert.ok(
    spec &&
      spec.title === equalTitle &&
      spec.file.endsWith("v060-equal-work.spec.ts") &&
      spec.ok === true,
  );
  assert.ok(spec.tests.length === 1);
  const test = spec.tests[0];
  assert.ok(test?.expectedStatus === "passed" && test.results.length === 1);
  const actual = test.results[0];
  assert.ok(actual?.status === "passed" && actual.retry === 0 && deepEquals(actual.errors, []));
  const entries = actual.attachments.filter((a) => a.name === "equal-work.json");
  assert.ok(entries.length === 1 && entries[0]?.contentType === "application/json");
  const facts = JSON.parse(Buffer.from(entries[0].body, "base64").toString("utf8")) as Record<
    string,
    unknown
  >;
  assert.ok(
    facts.backend === backend && facts.flow === flow && facts.workloadHash === workloadHash,
  );
  assert.ok(
    facts.warmSaves === 1 &&
      facts.warmReadbacks === 1 &&
      facts.edits === 3 &&
      facts.acks === 3 &&
      facts.freshReadbacks === 3 &&
      facts.contextsClosed === true,
  );
  const latencies = facts.ackLatencyMs as number[];
  assert.ok(latencies.length === 3 && latencies.every((v) => Number.isFinite(v) && v > 0));
  assert.ok(new Set(facts.ackIds as string[]).size === 3);
  assert.ok(deepEquals(facts.texts, workload.edits));
  return facts;
}

export function comparePair(on: DriverReceipt, off: DriverReceipt): object {
  const connections = (value: ConnectionFacts, flow: string, closed: boolean) => {
    assert.ok(
      [value.opened, value.collabOpened, value.closeEvents, value.active, value.collabActive].every(
        (v) => Number.isSafeInteger(v) && v >= 0,
      ),
    );
    assert.ok(
      value.closeEvents <= value.opened &&
        value.active <= value.opened &&
        value.collabOpened <= value.opened &&
        value.collabActive <= value.active,
    );
    assert.ok(value.contextClosed === closed);
    assert.ok(value.collabActive === (closed || flow === "off" ? 0 : 1));
    assert.ok(flow === "on" ? value.collabOpened >= 1 : value.collabOpened === 0);
    if (closed) assert.ok(value.active === 0);
  };
  const qualify = (receipt: DriverReceipt, flow: string) => {
    assert.ok(
      receipt.final_exit_code === 0 &&
        receipt.purpose === "equal-work" &&
        receipt.selected_flow === flow,
    );
    assert.ok(
      receipt.actual_browser_tests === 1 &&
        receipt.retries === 0 &&
        receipt.original_driver_failure === undefined,
    );
    assert.ok(
      receipt.owned_container_absent === true &&
        receipt.owned_loopback_port_closed === true &&
        receipt.recorded_process_identities_retired === true &&
        deepEquals(receipt.cleanup_errors, []),
    );
    const measurement = receipt.equal_work as {
      inputs: {
        source: string;
        tree: string;
        workloadHash: string;
        sourceInputsHash: string;
        image: string;
        native: Record<string, string>;
        assets: Record<string, string>;
        browser: Browser;
      };
      browser: Record<string, unknown>;
      samples: Sample[];
    };
    assert.ok(
      measurement.browser.flow === flow && measurement.browser.workloadHash === workloadHash,
    );
    const b = measurement.browser;
    const inputs = measurement.inputs,
      hash = /^[0-9a-f]{64}$/;
    assert.ok(
      inputs.source === receipt.source &&
        inputs.tree === receipt.tree &&
        inputs.workloadHash === workloadHash,
    );
    assert.ok(hash.test(inputs.sourceInputsHash) && /@sha256:[0-9a-f]{64}$/.test(inputs.image));
    for (const map of [inputs.native, inputs.assets, inputs.browser.chromium_directory_files])
      assert.ok(Object.keys(map).length > 0 && Object.values(map).every((v) => hash.test(v)));
    assert.ok(hash.test(inputs.browser.bun.sha256) && hash.test(inputs.browser.chromium.sha256));
    assert.ok(
      b.warmSaves === 1 &&
        b.warmReadbacks === 1 &&
        b.edits === 3 &&
        b.acks === 3 &&
        b.freshReadbacks === 3 &&
        b.contextsClosed === true,
    );
    assert.ok(deepEquals(b.texts, workload.edits));
    const ackIds = b.ackIds as string[],
      latencies = b.ackLatencyMs as number[];
    assert.ok(
      ackIds.length === 3 &&
        new Set(ackIds).size === 3 &&
        ackIds.every((id) =>
          /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id),
        ),
    );
    assert.ok(latencies.length === 3 && latencies.every((v) => Number.isFinite(v) && v > 0));
    validateSamples(measurement.samples);
    const resources = b.resources as {
      phase: string;
      ownerConnections: ConnectionFacts;
      observedRoomKeys: number;
    }[];
    connections(b.ownerAtStart as ConnectionFacts, flow, false);
    connections(b.ownerAfterClose as ConnectionFacts, flow, true);
    assert.ok(
      resources.length === 3 &&
        resources.every(
          (r, i) =>
            r.phase === `ack-${String(i)}` && r.observedRoomKeys === (flow === "on" ? 1 : 0),
        ),
    );
    for (const resource of resources) connections(resource.ownerConnections, flow, false);
    const fresh = b.freshConnections as {
      text: string;
      beforeClose: ConnectionFacts;
      afterClose: ConnectionFacts;
    }[];
    assert.ok(
      deepEquals(
        fresh.map((f) => f.text),
        [workload.warm, ...workload.edits],
      ),
    );
    for (const client of fresh) {
      connections(client.beforeClose, flow, false);
      connections(client.afterClose, flow, true);
      assert.ok(
        client.afterClose.opened >= client.beforeClose.opened &&
          client.afterClose.collabOpened >= client.beforeClose.collabOpened &&
          client.afterClose.closeEvents >= client.beforeClose.closeEvents,
      );
    }
    return measurement;
  };
  const a = qualify(on, "on"),
    b = qualify(off, "off");
  assert.ok(on.source === off.source && on.tree === off.tree && on.root_owner === off.root_owner);
  assert.ok(a.browser.backend === b.browser.backend);
  assert.ok(
    deepEquals(a.inputs, b.inputs),
    "different build/browser/dataset inputs cannot form a pair",
  );
  const numbers = (m: typeof a) => {
    const measured = m.samples.slice(1, -1),
      start = measured[0] as Sample,
      end = measured.at(-1) as Sample;
    return {
      cpuDeltaUsec: end.cpuUsec - start.cpuUsec,
      elapsedMs: end.monotonicMs - start.monotonicMs,
      maximumSampledAnonRssProxyBytes: Math.max(...measured.map((s) => s.anonRssProxyBytes)),
      maximumSampledServerRssBytes: Math.max(
        ...measured.map((s) => s.metrics[metricNames[0]] as number),
      ),
      lifetimeCgroupPeakBytes: end.lifetimePeakBytes,
      ackLatencyMs: m.browser.ackLatencyMs,
    };
  };
  return {
    backend: a.browser.backend,
    workloadHash,
    on: numbers(a),
    off: numbers(b),
    definitions: {
      cpu: "owned app-container cumulative cpu.stat usage_usec delta, start through final fresh readback; includes native helper and equal collector exec overhead; excludes browser, PG and search containers",
      memory:
        "memory.current is whole cgroup charge; memory.stat anon is an RSS proxy, distinct from /metrics server VmRSS; sampled maxima are not continuous peaks",
      peak: "memory.peak is container lifetime charge peak including startup/warm, not a reset measurement-window RSS peak",
      room: "total observed/open sockets are separate from /collab active sockets (ON1/OFF0); owner and four fresh contexts retain positive close events; routing keys are observed owner rooms, not an internal server registry gauge",
      latency:
        "before UI Save click through matching durable ACK and persisted UI; runner monotonic ms; no speed threshold",
    },
  };
}
