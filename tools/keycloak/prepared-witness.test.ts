import { afterEach, expect, test } from "bun:test";
import {
  chmodSync,
  closeSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  truncateSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import * as runner from "./run.ts";
import {
  preparedReadValue,
  type PreparedReadRequest,
  type PreparedSnapshot,
} from "../../apps/web/e2e-keycloak/observer.ts";
import type { PreparedActors } from "../../apps/web/e2e-keycloak/config.ts";
import type { PreparedOperationContext, PreparedPrimary, PreparedReadContext } from "./run.ts";

const audit = runner.auditPreparedOperation;

function readFixture() {
  const binding = {
    source: "a".repeat(40),
    tree: "b".repeat(40),
    target: "owned-target",
    group: "failures" as const,
    issuer: "owned-issuer",
  };
  const actors: PreparedActors = {
    owner: { id: "1".repeat(36), email: "owner@example.com", password: "unused" },
    member: { email: "member@example.com", password: "unused" },
    workspace: { id: "2".repeat(36), slug: "owned", name: "owned" },
    mismatchEmail: "mismatch@example.com",
    crossOriginEmail: "cross@example.com",
    users: Object.fromEntries(
      ["alice", "bob", "carol", "mallory", "erin", "tina"].map((key) => [
        key,
        { username: `owned-${key}`, email: `${key}@example.com` },
      ]),
    ) as PreparedActors["users"],
  };
  const child = { pid: 201 } as PreparedReadContext["child"];
  const peer = { pid: 203, startTicks: "567", parentPid: 202, state: "S" };
  const context: PreparedReadContext = {
    binding,
    actors,
    ordinal: 4,
    checkpointTaken: false,
    child,
    peer,
    scope: {
      allocations: [{ process: child, label: "keycloak-command", closed: false, forced: false }],
      entries: new Map([
        [
          "peer",
          { identity: peer, label: "descendant", allocation: 0, pidfd: null, reaped: false },
        ],
      ]),
      retired: () => false,
    },
  };
  const request: PreparedReadRequest = {
    binding: structuredClone(binding),
    ordinal: 5,
    actor: actors.owner.email,
    input: { operation: "links" },
    peer: { pid: peer.pid, startTicks: peer.startTicks },
  };
  return { context, request };
}

test("prepared fixed reads admit only the next owned peer request and isolate caller mutations", () => {
  const { context, request } = readFixture();
  const accepted = runner.acceptPreparedRead(request, context);
  expect(accepted).toEqual(request);
  accepted.binding.target = "changed";
  expect(context.binding.target).toBe("owned-target");
  for (const operation of ["user", "sessions", "invitation"] as const) {
    request.actor = context.actors.member.email;
    request.input = { operation, email: request.actor };
    expect(runner.acceptPreparedRead(request, context).input).toEqual(request.input);
  }
});

for (const [label, mutate] of [
  [
    "source",
    (f) => {
      f.request.binding.source = "d".repeat(40);
    },
  ],
  [
    "tree",
    (f) => {
      f.request.binding.tree = "e".repeat(40);
    },
  ],
  [
    "target",
    (f) => {
      f.request.binding.target = "foreign";
    },
  ],
  [
    "group",
    (f) => {
      f.request.binding.group = "flows";
    },
  ],
  [
    "issuer",
    (f) => {
      f.request.binding.issuer = "foreign";
    },
  ],
  [
    "duplicate",
    (f) => {
      f.request.ordinal = 4;
    },
  ],
  [
    "out-of-order",
    (f) => {
      f.request.ordinal = 6;
    },
  ],
  [
    "fractional ordinal",
    (f) => {
      f.request.ordinal = 4.5;
    },
  ],
  [
    "foreign actor",
    (f) => {
      f.request.actor = "foreign@example.com";
    },
  ],
  [
    "foreign email",
    (f) => {
      f.request.actor = "foreign@example.com";
      f.request.input = { operation: "user", email: f.request.actor };
    },
  ],
  [
    "different actor",
    (f) => {
      f.request.input = { operation: "sessions", email: f.context.actors.member.email };
    },
  ],
  [
    "PID",
    (f) => {
      f.request.peer.pid++;
    },
  ],
  [
    "startTicks",
    (f) => {
      f.request.peer.startTicks = "999";
    },
  ],
  [
    "actual identity",
    (f) => {
      f.context.peer = { ...f.context.peer, startTicks: "888" };
    },
  ],
  [
    "zombie peer",
    (f) => {
      f.context.peer = { ...f.context.peer, state: "Z" };
    },
  ],
  [
    "retired peer",
    (f) => {
      f.context.scope.retired = () => true;
    },
  ],
  [
    "closed allocation",
    (f) => {
      const a = f.context.scope.allocations[0];
      if (a) a.closed = true;
    },
  ],
  [
    "forced allocation",
    (f) => {
      const a = f.context.scope.allocations[0];
      if (a) a.forced = true;
    },
  ],
  [
    "missing allocation",
    (f) => {
      f.context.scope.allocations.length = 0;
    },
  ],
  [
    "other allocation",
    (f) => {
      const e = f.context.scope.entries.get("peer");
      if (e) e.allocation = 1;
    },
  ],
  [
    "missing captured peer",
    (f) => {
      f.context.scope.entries.clear();
    },
  ],
] satisfies Array<[string, (f: ReturnType<typeof readFixture>) => void]>) {
  test(`prepared fixed read refuses ${label}`, () => {
    const f = readFixture();
    mutate(f);
    expect(() => runner.acceptPreparedRead(f.request, f.context)).toThrow("Keycloak prepared");
  });
}

test("prepared reads reject malformed, arbitrary and extra-field operations", () => {
  const { context, request } = readFixture();
  for (const value of [
    null,
    [],
    {},
    { ...request, path: "untrusted" },
    { ...request, input: { operation: "sql", sql: "untrusted" } },
    { ...request, input: { operation: "links", email: context.actors.owner.email } },
    {
      ...request,
      input: { operation: "membership", email: context.actors.owner.email, slug: "owned" },
    },
    { ...request, peer: { ...request.peer, approved: true } },
  ])
    expect(() => runner.acceptPreparedRead(value, context)).toThrow("Keycloak prepared");
});

test("the one before-unlink checkpoint binds failure group/member/bob/exact subject", () => {
  const { context, request } = readFixture();
  request.actor = context.actors.member.email;
  request.input = {
    operation: "before-unlink",
    email: request.actor,
    identityEmail: context.actors.users.bob.email,
    subject: "3".repeat(36),
  };
  expect(runner.acceptPreparedRead(request, context)).toEqual(request);
  const input = request.input;
  for (const bad of [
    { ...input, email: context.actors.owner.email },
    { ...input, identityEmail: context.actors.users.alice.email },
    { ...input, subject: "foreign" },
  ])
    expect(() => runner.acceptPreparedRead({ ...request, input: bad }, context)).toThrow();
  context.checkpointTaken = true;
  expect(() => runner.acceptPreparedRead(request, context)).toThrow();
  context.checkpointTaken = false;
  context.binding = { ...context.binding, group: "flows" };
  request.binding = structuredClone(context.binding);
  expect(() => runner.acceptPreparedRead(request, context)).toThrow();
});

test("fixed frames reject truncation, extra frames, oversized bytes and malformed UTF-8 without quoting bytes", () => {
  const { request } = readFixture();
  const valid = Buffer.from(JSON.stringify(request) + "\n");
  expect(runner.preparedReadFrame(valid)).toEqual(request);
  for (const bytes of [
    valid.subarray(0, -1),
    Buffer.concat([valid, valid]),
    Buffer.alloc(4097),
    Buffer.from("private-canary\n"),
    Buffer.from([0xff, 10]),
    Buffer.from("{}\n"),
  ]) {
    expect(() => runner.preparedReadFrame(bytes)).toThrow("Keycloak prepared read frame differs");
  }
});

test("the parent read is awaited, propagates its original error, and cannot return raw native data", async () => {
  const { request } = readFixture();
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let entered = false,
    settled = false;
  const pending = runner
    .readPreparedOperation(
      {
        observeOperation: async (r) => {
          expect(r).toEqual(request);
          r.binding.target = "mutation";
          entered = true;
          await gate;
          return { value: [], rollback: "confirmed", lifecycleDrain: "confirmed", leases: 0 };
        },
      },
      request,
    )
    .then((value) => {
      settled = true;
      return value;
    });
  expect(entered).toBe(true);
  expect(settled).toBe(false);
  release();
  expect(await pending).toEqual([]);
  expect(request.binding.target).toBe("owned-target");
  const original = new Error("parent-checkpoint-rejected");
  const failure = await runner
    .readPreparedOperation(
      {
        observeOperation: () => {
          throw original;
        },
      },
      request,
    )
    .catch((error: unknown) => error);
  expect(failure).toBe(original);
  const datasetFailure = await runner
    .readPreparedOperation({ observeOperation: () => Promise.resolve({ dataset: {} }) }, request)
    .catch((error: unknown) => error);
  expect(datasetFailure).toBeInstanceOf(Error);
  expect(String(datasetFailure)).toContain("Keycloak prepared native closure differs");
  const missingFailure = await runner
    .readPreparedOperation({} as Pick<PreparedPrimary, "observeOperation">, request)
    .catch((error: unknown) => error);
  expect(missingFailure).toBeInstanceOf(Error);
  expect(String(missingFailure)).toContain("callback missing");
});

test("only bounded OIDC projections and ACK can leave the parent", () => {
  const envelope = (value: unknown) => ({
    value,
    rollback: "confirmed",
    lifecycleDrain: "confirmed",
    leases: 0,
  });
  const row = {
    user: "owner@example.com",
    provider: "generic",
    subject: "owned-subject",
    issuer: null,
    email: null,
  };
  expect(preparedReadValue({ operation: "links" }, envelope([row]))).toEqual([row]);
  for (const value of [
    [{ ...row, token: "private-canary" }],
    Array(10001).fill(row),
    [{ ...row, subject: "s".repeat(513) }],
  ])
    expect(() => preparedReadValue({ operation: "links" }, envelope(value))).toThrow();
  for (const result of [
    { ...envelope([]), dataset: {} },
    { ...envelope([]), leases: 1 },
    { ...envelope([]), rollback: "missing" },
    { ...envelope([]), lifecycleDrain: "missing" },
  ])
    expect(() => preparedReadValue({ operation: "links" }, result)).toThrow();
  for (const value of [-1, 0.5, NaN, "1"])
    expect(() => preparedReadValue({ operation: "users" }, envelope(value))).toThrow();
  expect(preparedReadValue({ operation: "users" }, envelope(8))).toBe(8);
  const checkpoint = {
    operation: "before-unlink" as const,
    email: "member@example.com",
    identityEmail: "bob@example.com",
    subject: "3".repeat(36),
  };
  expect(preparedReadValue(checkpoint, { ack: true })).toBeNull();
  for (const result of [{ ack: false }, { ack: true, dataset: {} }, envelope(null)])
    expect(() => preparedReadValue(checkpoint, result)).toThrow();
});
const cleanups: Array<() => void> = [];
afterEach(() => {
  for (const close of cleanups.splice(0)) close();
});

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "kc-operation-"));
  const path = join(directory, "server.log");
  const fd = openSync(path, "wx", 0o600);
  cleanups.push(() => {
    closeSync(fd);
    rmSync(directory, { recursive: true, force: true });
  });
  const binding = {
    source: "a".repeat(40),
    tree: "b".repeat(40),
    target: "c".repeat(64),
    group: "flows" as const,
    issuer: "owned-issuer",
  };
  const before: PreparedSnapshot = {
    binding,
    users: [],
    links: [],
    memberships: [],
    states: 0,
    wholePrimary: {},
  };
  const server = {
    identity: { pid: 123, startTicks: "456" },
    executable: { path: "/sealed/fvoci-server", sha256: "d".repeat(64) },
  };
  const child = { pid: 123 } as PreparedOperationContext["child"];
  const root = { pid: 123, startTicks: "456", parentPid: 99, state: "Z" };
  const descendant = { pid: 124, startTicks: "457", parentPid: 123, state: "Z" };
  const keycloak = { pid: 789, startTicks: "888", parentPid: 99, state: "S" };
  const entry = (identity: typeof root, allocation: number) => ({
    identity,
    allocation,
    label: "normal-server",
    pidfd: null,
    reaped: true,
  });
  const context: PreparedOperationContext = {
    binding,
    server,
    child,
    logPath: path,
    logFd: fd,
    exitCode: 0,
    portClosed: true,
    manifest: {
      sourceInputs: { source: binding.source, tree: binding.tree, files: {} },
      binaries: { "fvoci-server": { ...server.executable, features: [], profile: {} } },
    },
    scope: {
      allocations: [
        { process: child, label: "normal-server", closed: true, forced: false, key: "server" },
        {
          process: { pid: 789 } as PreparedOperationContext["child"],
          label: "keycloak-command",
          closed: false,
          forced: false,
          key: "keycloak",
        },
      ],
      entries: new Map([
        ["server", entry(root, 0)],
        ["descendant", entry(descendant, 0)],
        ["keycloak", entry(keycloak, 1)],
      ]),
      errors: [],
      retired: (row) => row.pid !== 789,
    },
  };
  for (const key of [8, 9, 1])
    for (const outcome of ["prepared", "acquired", "released"]) {
      writeSync(
        fd,
        "FVOCI_E2E_MAINTENANCE_RECEIPT " +
          JSON.stringify({
            schema: 1,
            pid: 123,
            key,
            outcome,
            generation: outcome === "prepared" ? null : "1",
            ownerSha256: String(key).repeat(64),
          }) +
          "\n",
      );
    }
  return { context, before, after: structuredClone(before), delta: { emails: [], links: [] } };
}

test("the awaited parent receives actual allocation-only closure and the maintained private log receipts", async () => {
  const f = fixture();
  const expectedServer = f.context.server;
  if (expectedServer === undefined) throw new Error("fixture server missing");
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let entered: () => void = () => {};
  const entry = new Promise<void>((resolve) => {
    entered = resolve;
  });
  let settled = false;
  const primary: Pick<PreparedPrimary, "assertWholePrimary"> = {
    assertWholePrimary: async (before, after, delta, witness) => {
      expect(before).toBe(f.before);
      expect(after).toBe(f.after);
      expect(delta).toBe(f.delta);
      expect(witness.binding).toEqual(f.before.binding);
      expect(witness.server).toEqual(expectedServer);
      expect(witness.retirement.identities.map((row) => row.pid)).toEqual([123, 124]);
      expect(witness.maintenance.receipts).toHaveLength(9);
      expect(witness.maintenance.identity).toEqual(expectedServer.identity);
      entered();
      await gate;
    },
  };
  const pending = audit(primary, f.before, f.after, f.delta, f.context).then(() => {
    settled = true;
  });
  await entry;
  expect(settled).toBe(false);
  release();
  await pending;
  expect(settled).toBe(true);
});

const rejectionCases: Array<[string, (context: PreparedOperationContext) => void]> = [
  [
    "foreign source",
    (c) => {
      c.binding = { ...c.binding, source: "e".repeat(40) };
    },
  ],
  [
    "foreign tree",
    (c) => {
      c.binding = { ...c.binding, tree: "e".repeat(40) };
    },
  ],
  [
    "foreign target",
    (c) => {
      c.binding = { ...c.binding, target: "e".repeat(64) };
    },
  ],
  [
    "foreign group",
    (c) => {
      c.binding = { ...c.binding, group: "failures" };
    },
  ],
  [
    "foreign issuer",
    (c) => {
      c.binding = { ...c.binding, issuer: "foreign-issuer" };
    },
  ],
  [
    "missing protected witness",
    (c) => {
      c.server = undefined;
    },
  ],
  [
    "foreign PID",
    (c) => {
      if (c.server) c.server.identity.pid = 999;
    },
  ],
  [
    "foreign startTicks",
    (c) => {
      if (c.server) c.server.identity.startTicks = "999";
    },
  ],
  [
    "foreign ELF",
    (c) => {
      if (c.server) c.server.executable = { ...c.server.executable, sha256: "e".repeat(64) };
    },
  ],
  [
    "foreign executable path",
    (c) => {
      if (c.server) c.server.executable = { ...c.server.executable, path: "/foreign/fvoci-server" };
    },
  ],
  [
    "missing allocation",
    (c) => {
      c.scope.allocations = [];
    },
  ],
  [
    "nonzero exit",
    (c) => {
      c.exitCode = 1;
    },
  ],
  [
    "forced retirement",
    (c) => {
      const a = c.scope.allocations[0];
      if (a) a.forced = true;
    },
  ],
  [
    "unclosed allocation",
    (c) => {
      const a = c.scope.allocations[0];
      if (a) a.closed = false;
    },
  ],
  [
    "live descendant",
    (c) => {
      c.scope.retired = (row) => row.pid === 123;
    },
  ],
  [
    "open port",
    (c) => {
      c.portClosed = false;
    },
  ],
  [
    "scope error",
    (c) => {
      c.scope.errors.push("observed-error");
    },
  ],
  [
    "foreign log file",
    (c) => {
      const foreign = join(dirname(c.logPath), "foreign.log");
      writeFileSync(foreign, readFileSync(c.logPath), { mode: 0o600 });
      c.logPath = foreign;
    },
  ],
  [
    "nonprivate log",
    (c) => {
      chmodSync(c.logPath, 0o644);
    },
  ],
];
for (const [name, mutate] of rejectionCases)
  test(`prepared audit refuses ${name} before calling the parent`, async () => {
    const f = fixture();
    mutate(f.context);
    let calls = 0,
      failure: unknown;
    try {
      await audit(
        {
          assertWholePrimary: () => {
            calls++;
            return Promise.resolve();
          },
        },
        f.before,
        f.after,
        f.delta,
        f.context,
      );
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(Error);
    expect(calls).toBe(0);
  });

test("missing context or callback cannot self-authorize an audit", async () => {
  const f = fixture();
  for (const [primary, context] of [
    [{ assertWholePrimary: async () => {} }, undefined],
    [{}, f.context],
  ] as const) {
    let failure: unknown;
    try {
      await audit(
        primary as Pick<PreparedPrimary, "assertWholePrimary">,
        f.before,
        f.after,
        f.delta,
        context,
      );
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(Error);
  }
});

test("the existing maintenance reader rejects oversized and foreign-PID logs", async () => {
  for (const malformed of ["oversized", "foreign-pid"]) {
    const f = fixture();
    let calls = 0,
      failure: unknown;
    if (malformed === "oversized") truncateSync(f.context.logPath, 32 * 1024 * 1024 + 1);
    else
      writeFileSync(
        f.context.logPath,
        "FVOCI_E2E_MAINTENANCE_RECEIPT " +
          JSON.stringify({
            schema: 1,
            pid: 999,
            key: 8,
            outcome: "released",
            generation: "1",
            ownerSha256: "a".repeat(64),
          }),
      );
    try {
      await audit(
        {
          assertWholePrimary: () => {
            calls++;
            return Promise.resolve();
          },
        },
        f.before,
        f.after,
        f.delta,
        f.context,
      );
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(Error);
    expect(calls).toBe(0);
  }
});

test("the parent's original failure propagates and a changed log cannot finish successfully", async () => {
  const f = fixture(),
    original = new Error("parent-native-audit-failed");
  let failure: unknown;
  try {
    await audit(
      {
        assertWholePrimary: () => Promise.reject(original),
      },
      f.before,
      f.after,
      f.delta,
      f.context,
    );
  } catch (error) {
    failure = error;
  }
  expect(failure).toBe(original);
  failure = undefined;
  try {
    await audit(
      {
        assertWholePrimary: () => {
          const bytes = readFileSync(f.context.logPath);
          const first = bytes[0];
          if (first === undefined) throw new Error("fixture log empty");
          bytes[0] = first ^ 1;
          writeFileSync(f.context.logPath, bytes);
          return Promise.resolve();
        },
      },
      f.before,
      f.after,
      f.delta,
      f.context,
    );
  } catch (error) {
    failure = error;
  }
  expect(failure).toBeInstanceOf(Error);
});
