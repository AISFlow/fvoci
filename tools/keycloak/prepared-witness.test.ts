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
import type { PreparedSnapshot } from "../../apps/web/e2e-keycloak/observer.ts";
import type { PreparedOperationContext, PreparedPrimary } from "./run.ts";

const audit = runner.auditPreparedOperation;
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
