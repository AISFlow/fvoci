// The admitted parent owns startup, credentials, seed preparation, browser and
// closure. This adapter only collects the existing workload and qualifies its
// whole native observations. It grants no runtime or target authority.
import { deepEquals } from "bun";
import { strict as assert } from "node:assert";
import {
  comparePair,
  measureBrowser,
  validateEqualReport,
  workload,
  workloadHash,
  type EqualInputs,
  type EqualMeasurement,
  type EqualPairReceipt,
} from "../selected-backend-ci/drivers/equal-work.ts";
import type { Command, Completed } from "../selected-backend-ci/drivers/common.ts";
import { uuidHex, type Audit } from "./ui-audit.ts";
import { get, valueDigest, type Record_ } from "./ui-common.ts";
import type { ServerHandle } from "./ui-container.ts";

export interface LogicalSeed {
  users: typeof workload.users;
  documents: typeof workload.documents;
  roles: ["owner", "member"];
  initialBody: "";
  preparation: typeof workload.preparation;
}
export interface HostedSeed {
  namespace: string;
  owner: string;
  member: string;
  workspace: string;
  document: string;
  logical: LogicalSeed;
}
export interface HostedInputs extends EqualInputs {
  targetSha256: string;
  configSha256: string;
  logical: LogicalSeed;
}
export interface HostedMeasurement extends EqualMeasurement {
  inputs: HostedInputs;
  flow: "on" | "off";
  containerId: string;
  serverPid: number;
  maintenanceIdentity: { pid: number; startTicks: string };
  seed: HostedSeed;
  report: unknown;
  browserAllocation: HostedSeed;
}
export interface HostedClosure {
  containerId: string;
  serverPid: number;
  serverExit: number;
  forced: boolean;
  containerAbsent: boolean;
  portClosed: boolean;
  recordedIdentitiesRetired: boolean;
  cleanupErrors: string[];
}
export interface HostedEvidence {
  // Complete unfiltered native baseline results. The digest is derived here,
  // never substituted for the observations or used as preparation authority.
  before: Record_;
  after: Record_;
  serialChecksBefore: Record_;
  serialChecksAfter: Record_;
  operationWitnesses: Audit;
}
export interface HostedCase {
  measurement: HostedMeasurement;
  evidence: HostedEvidence;
  closure: HostedClosure;
}

export function measurementContainer(server: Pick<ServerHandle, "containerId">): string {
  assert.ok(
    typeof server.containerId === "string" && /^[a-f0-9]{64}$/.test(server.containerId),
    "equal-work requires the actual owned app container",
  );
  return server.containerId;
}

export async function measureHosted(
  binding: {
    server: ServerHandle;
    base: string;
    flow: "on" | "off";
    inputs: HostedInputs;
    seed: HostedSeed;
  },
  directory: string,
  browser: () => Promise<{ result: Completed; report: unknown; allocation: HostedSeed }>,
  run?: Command,
): Promise<HostedMeasurement> {
  const containerId = measurementContainer(binding.server);
  assert.ok(
    /^http:\/\/127\.0\.0\.1:[1-9][0-9]*$/.test(binding.base),
    "normal loopback base required",
  );
  const serverPid = binding.server.child.pid;
  assert.ok(Number.isSafeInteger(serverPid) && serverPid > 0);
  const maintenanceIdentity = binding.server.maintenanceIdentity;
  assert.ok(maintenanceIdentity, "owned container maintenance identity required");
  const { inputs, seed, flow } = structuredClone({
    inputs: binding.inputs,
    seed: binding.seed,
    flow: binding.flow,
  });
  let observed: Awaited<ReturnType<typeof browser>> | undefined;
  const measured = await measureBrowser(
    directory,
    containerId,
    binding.base,
    async () => {
      observed = await browser();
      return observed.result;
    },
    run,
  );
  assert.ok(
    observed && deepEquals(observed.allocation, seed, true),
    "browser seed binding required",
  );
  const facts = validateEqualReport(observed.report, "libsql-remote", flow);
  return {
    inputs,
    seed,
    flow,
    containerId,
    serverPid,
    maintenanceIdentity: structuredClone(maintenanceIdentity),
    report: observed.report,
    browserAllocation: observed.allocation,
    browser: facts,
    samples: measured.samples,
  };
}

const same = (a: unknown, b: unknown) => deepEquals(a, b, true);
function qualify(value: HostedCase): EqualPairReceipt {
  const { measurement: m, evidence: e, closure: c } = value;
  const seed = m.seed,
    inputs = m.inputs;
  assert.ok(/^[a-f0-9]{40}$/.test(inputs.source) && /^[a-f0-9]{40}$/.test(inputs.tree));
  assert.ok([inputs.targetSha256, inputs.configSha256].every((s) => /^[a-f0-9]{64}$/.test(s)));
  assert.ok(
    same(seed.logical, {
      users: workload.users,
      documents: workload.documents,
      roles: ["owner", "member"],
      initialBody: "",
      preparation: workload.preparation,
    }),
  );
  assert.ok(same(inputs.logical, seed.logical) && inputs.workloadHash === workloadHash);
  assert.ok(/^tui-[a-f0-9]{20}$/.test(seed.namespace));
  const owner = uuidHex(seed.owner),
    member = uuidHex(seed.member),
    workspace = uuidHex(seed.workspace),
    document = uuidHex(seed.document);
  assert.ok(owner !== member && same(e.operationWitnesses.namespaces, [seed.namespace]));
  assert.ok(
    same(e.operationWitnesses.actors, {
      [owner]: seed.namespace + "-owner@example.invalid",
      [member]: seed.namespace + "-member@example.invalid",
    }),
  );
  assert.ok(
    same(e.operationWitnesses.workspaces, { [workspace]: seed.namespace }) &&
      same(e.operationWitnesses.documents, { [document]: workspace }),
  );
  assert.ok(
    e.operationWitnesses.targetSha256 === inputs.targetSha256 &&
      e.operationWitnesses.serverStarts === 1 &&
      e.operationWitnesses.servers.length === 1,
  );
  assert.ok(same(get(e.operationWitnesses.servers[0], "identity"), m.maintenanceIdentity));
  assert.ok(Number.isSafeInteger(m.serverPid) && m.serverPid > 0);
  assert.ok(Number.isSafeInteger(m.maintenanceIdentity.pid) && m.maintenanceIdentity.pid > 0);
  assert.ok(/^[0-9]+$/.test(m.maintenanceIdentity.startTicks));
  assert.ok(
    same(m.browserAllocation, seed) &&
      same(validateEqualReport(m.report, "libsql-remote", m.flow), m.browser),
  );
  assert.ok(
    c.containerId === m.containerId &&
      /^[a-f0-9]{64}$/.test(c.containerId) &&
      c.serverPid === m.serverPid,
  );
  assert.equal(c.serverExit, 0);
  assert.equal(c.forced, false);
  return {
    source: inputs.source,
    tree: inputs.tree,
    root_owner: inputs.targetSha256,
    purpose: "equal-work",
    selected_flow: m.flow,
    final_exit_code: c.serverExit,
    actual_browser_tests: 1,
    retries: 0,
    owned_container_absent: c.containerAbsent,
    owned_loopback_port_closed: c.portClosed,
    recorded_process_identities_retired: c.recordedIdentitiesRetired,
    cleanup_errors: c.cleanupErrors,
    equal_work: m,
  };
}

/** The maintained parent auditrail must validate every native row/counter and
 * serial quiescence/horizon fact against these actual observations, or throw.
 * Callback presence or a test no-op is not native/runtime product acceptance. */
export interface HostedNativeContext extends HostedEvidence {
  inputs: HostedInputs;
  seed: HostedSeed;
  flow: "on" | "off";
  samples: EqualMeasurement["samples"];
  closure: HostedClosure;
}
export type HostedNativeValidator = (context: HostedNativeContext) => void | Promise<void>;

export async function compareHostedPair(
  onCase: HostedCase,
  offCase: HostedCase,
  validateNativeEvidence: HostedNativeValidator,
): Promise<object> {
  assert.ok(
    typeof validateNativeEvidence === "function",
    "actual parent native validator required",
  );
  const [on, off] = structuredClone<[HostedCase, HostedCase]>([onCase, offCase]);
  assert.ok(on.measurement.flow === "on" && off.measurement.flow === "off");
  assert.ok(
    on.measurement.containerId !== off.measurement.containerId,
    "independent normal server allocations required",
  );
  assert.ok(on.measurement.seed.namespace !== off.measurement.seed.namespace);
  for (const key of ["owner", "member", "workspace", "document"] as const)
    assert.ok(on.measurement.seed[key] !== off.measurement.seed[key]);
  assert.ok(
    same(on.evidence.after, off.evidence.before),
    "cumulative native snapshot continuity required",
  );
  const a = qualify(on),
    b = qualify(off);
  const comparison = comparePair(a, b);
  for (const value of [on, off]) {
    const { measurement: m, evidence, closure } = value;
    // Inspect the runtime return at this external callback boundary too.
    const validator: (context: HostedNativeContext) => unknown = validateNativeEvidence;
    const result = await Promise.resolve(
      validator({
        ...evidence,
        inputs: m.inputs,
        seed: m.seed,
        flow: m.flow,
        samples: m.samples,
        closure,
      }),
    );
    assert.ok(
      result === undefined,
      "native validator must reject by throwing, not return a verdict",
    );
  }
  return {
    comparison,
    physicalDatasets: {
      on: { before: valueDigest(on.evidence.before), after: valueDigest(on.evidence.after) },
      off: { before: valueDigest(off.evidence.before), after: valueDigest(off.evidence.after) },
    },
    backendDatabaseCpu: "NOTRUN",
  };
}
