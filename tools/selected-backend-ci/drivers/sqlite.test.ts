import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  truncateSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import process from "node:process";
import {
  checkpointPrefix,
  preparationSteps,
  publicCheckpoint,
  root,
  sha,
  parseJson,
} from "../io.ts";
import { laneRetirement } from "../runtime.ts";
import { discoverGroups } from "../../web-e2e/groups.ts";
import type { Current } from "./binding.ts";
import { restoreSpec, restoreBrowserTitle, validateRestoreReport } from "./binding.ts";
import { runtimeError, command as actualCommand } from "./common.ts";
import type { Child, CommandOptions, Completed, Row } from "./common.ts";
import {
  expectedMigrations,
  finalize,
  main,
  positiveDockerAbsence,
  prepareContainer,
  recordFailure,
  retiredIdentities,
  shellExports,
  summary,
  readSelectedSqlitePacket,
  sqliteReadbackAppendWitness,
  sqliteAppendPublicationWitness,
} from "./sqlite.ts";
import type { Seam, State } from "./sqlite.ts";

const SOURCE = "a".repeat(40),
  TREE = "b".repeat(40),
  OWNER = "pure-owned-control",
  NETWORK = "c".repeat(64);
const PRIVATE = "SYNTHETIC_PRIVATE_SECRET";
const identity = [process.getuid?.() ?? -1, process.getgid?.() ?? -1] as const;
const scratch: string[] = [];
afterEach(() => {
  for (const path of scratch.splice(0)) rmSync(path, { recursive: true, force: true });
});
function directory(): string {
  const path = mkdtempSync(join(tmpdir(), "fvoci-sqlite-driver-"));
  scratch.push(path);
  return path;
}
const ok = (stdout = ""): Completed => ({ returncode: 0, stdout, stderr: "" });

function readbackAppendFixture() {
  const uuid = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const workspaceId = uuid(1),
    documentId = uuid(2),
    actorId = uuid(3);
  const blob = (v: string) => ["blob", v.replaceAll("-", "")];
  const integer = (v: number) => ["integer", String(v)];
  const hash = (v: unknown) => createHash("sha256").update(JSON.stringify(v)).digest("hex");
  const columns = {
    document_collab_updates: [
      "workspace_id",
      "document_id",
      "seq",
      "op_id",
      "payload",
      "created_at",
    ],
    document_collab_op_receipts: [
      "workspace_id",
      "document_id",
      "op_id",
      "seq",
      "payload_len",
      "payload_sha256",
      "actor_user_id",
      "created_at",
    ],
    events: [
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
    audit_log: [
      "id",
      "actor_user_id",
      "workspace_id",
      "verb",
      "target_type",
      "target_id",
      "payload",
      "ip",
      "created_at",
    ],
  };
  const updates: unknown[] = [],
    receipts: unknown[] = [],
    writes: unknown[] = [];
  const appendRows: Record<string, unknown[]> = {
    document_collab_updates: [],
    document_collab_op_receipts: [],
    events: [],
    audit_log: [],
  };
  for (let seq = 1; seq <= 3; seq++) {
    const op = uuid(seq + 3),
      hex = "0102",
      sha = createHash("sha256").update(Buffer.from(hex, "hex")).digest("hex");
    updates.push({ seq, op, hex });
    receipts.push({ seq, op, len: 2, sha, actor: actorId });
    if (seq > 1)
      writes.push({
        workspaceId,
        documentId,
        actorId,
        sha256: sha,
        length: 2,
        empty: false,
        applied: true,
      });
    const payload = ["text", JSON.stringify({ documentId, opId: op, seq, writerGeneration: 2 })];
    const cells = {
      document_collab_updates: [
        blob(workspaceId),
        blob(documentId),
        integer(seq),
        blob(op),
        ["blob", hex],
        integer(seq === 1 ? 10 : 30),
      ],
      document_collab_op_receipts: [
        blob(workspaceId),
        blob(documentId),
        blob(op),
        integer(seq),
        integer(2),
        ["blob", sha],
        blob(actorId),
        integer(seq === 1 ? 10 : 30),
      ],
      events: [
        blob(uuid(10 + seq)),
        integer(100 + seq),
        blob(workspaceId),
        blob(actorId),
        ["text", "document.collab_update_appended"],
        ["text", "document"],
        blob(documentId),
        payload,
        ["text", "web"],
        integer(seq === 1 ? 10 : 30),
      ],
      audit_log: [
        blob(uuid(20 + seq)),
        blob(actorId),
        blob(workspaceId),
        ["text", "document.collab_update_appended"],
        ["text", "document"],
        blob(documentId),
        payload,
        ["null"],
        integer(seq === 1 ? 10 : 30),
      ],
    };
    for (const table of Object.keys(columns) as (keyof typeof columns)[])
      appendRows[table]?.push({
        columns: columns[table],
        cells: cells[table],
        sha256: hash(cells[table]),
      });
  }
  const current = {
    writer_generation: 2,
    state: "ff80",
    cutoff: 0,
    tail: 3,
    content: { exact: true },
    text: "original",
    version: 1,
    revisions: [],
    updates,
    receipts,
    appendRows,
  };
  const original = structuredClone(current);
  original.tail = 1;
  original.updates = original.updates.slice(0, 1);
  original.receipts = original.receipts.slice(0, 1);
  for (const table of Object.keys(appendRows))
    original.appendRows[table] = original.appendRows[table]?.slice(0, 1) ?? [];
  const dataset = (value: typeof current) => ({
    operations: {
      events: (value.appendRows.events ?? []).map((row) =>
        (row as { cells: unknown[] }).cells.slice(1, 4),
      ),
    },
    fingerprints: Object.fromEntries(
      Object.entries(value.appendRows).map(([table, rows]) => [
        table,
        Object.fromEntries(rows.map((row) => [(row as { sha256: string }).sha256, 1])),
      ]),
    ),
  });
  return {
    original,
    current,
    before: dataset(original),
    prepared: dataset(current),
    workspaceId,
    documentId,
    actorId,
    writes,
    clocks: ["20", "40"] as [string, string],
  };
}

test("SQLite readback admits only two wire/ACK-matched full typed native appends and retains every original", async () => {
  const input = readbackAppendFixture();
  const seal = await sqliteReadbackAppendWitness(input);
  expect(seal.count).toBe(2);
  expect(Object.values(seal.rows).map((rows) => Object.keys(rows).length)).toEqual([2, 2, 2, 2]);
  expect(Object.keys(seal.eventOperations).sort()).toEqual(
    Object.keys(seal.rows.events ?? {}).sort(),
  );
  const changes: ((f: ReturnType<typeof readbackAppendFixture>) => void)[] = [
    (f) => {
      f.current.state = "ff81";
    },
    (f) => {
      f.current.cutoff = 1;
    },
    (f) => {
      f.current.content = { exact: false };
    },
    (f) => {
      f.current.tail = 4;
    },
    (f) => {
      f.current.updates.shift();
    },
    (f) => {
      f.current.receipts[0] = {};
    },
    (f) => {
      f.writes.pop();
    },
    (f) => {
      f.writes.push(structuredClone(f.writes[0]));
    },
    (f) => {
      (f.writes[0] as Record<string, unknown>).applied = false;
    },
    (f) => {
      (f.writes[0] as Record<string, unknown>).actorId = f.documentId;
    },
    (f) => {
      (f.writes[0] as Record<string, unknown>).documentId = f.workspaceId;
    },
    (f) => {
      (f.current.receipts[2] as Record<string, unknown>).op = (
        f.current.receipts[1] as Record<string, unknown>
      ).op;
    },
    (f) => {
      (f.current.updates[1] as Record<string, unknown>).hex = "0103";
    },
    (f) => {
      (f.current.receipts[1] as Record<string, unknown>).len = 3;
    },
    (f) => {
      f.current.appendRows.document_collab_updates?.shift();
    },
    (f) => {
      (f.current.appendRows.document_collab_updates?.[1] as { sha256: string }).sha256 = "f".repeat(
        64,
      );
    },
    (f) => {
      (f.current.appendRows.document_collab_op_receipts?.[1] as { cells: string[][] }).cells[6] = [
        "blob",
        f.documentId.replaceAll("-", ""),
      ];
    },
    (f) => {
      f.clocks[1] = "29";
    },
    (f) => {
      f.prepared.fingerprints.document_collab_updates = {};
    },
    (f) => {
      f.before.fingerprints.document_collab_op_receipts = {};
    },
  ];
  for (const change of changes) {
    const wrong = readbackAppendFixture();
    change(wrong);
    expect(await rejection(sqliteReadbackAppendWitness(wrong))).toBeInstanceOf(Error);
  }
});

test("SQLite readback refuses altered complete publication rows even when their forged native fingerprints agree", async () => {
  for (const table of ["events", "audit_log"] as const) {
    const event = table === "events",
      offset = event ? 1 : 0;
    const mutations: ((cells: unknown[][], f: ReturnType<typeof readbackAppendFixture>) => void)[] =
      [
        (cells) => {
          cells[0] = ["blob", "0".repeat(32)];
        },
        (cells, f) => {
          cells[2] = ["blob", f.documentId.replaceAll("-", "")];
        },
        (cells, f) => {
          cells[event ? 3 : 1] = ["blob", f.workspaceId.replaceAll("-", "")];
        },
        (cells) => {
          cells[3 + offset] = ["text", "document.updated"];
        },
        (cells) => {
          cells[4 + offset] = ["text", "task"];
        },
        (cells, f) => {
          cells[5 + offset] = ["blob", f.actorId.replaceAll("-", "")];
        },
        (cells) => {
          const p = JSON.parse(String(cells[6 + offset]?.[1])) as Record<string, unknown>;
          p.writerGeneration = 3;
          cells[6 + offset] = ["text", JSON.stringify(p)];
        },
        (cells) => {
          const p = JSON.parse(String(cells[6 + offset]?.[1])) as Record<string, unknown>;
          p.opId = "00000000-0000-4000-8000-000000000099";
          cells[6 + offset] = ["text", JSON.stringify(p)];
        },
        (cells) => {
          const p = JSON.parse(String(cells[6 + offset]?.[1])) as Record<string, unknown>;
          p.seq = 99;
          cells[6 + offset] = ["text", JSON.stringify(p)];
        },
        (cells) => {
          cells[cells.length - 1] = ["integer", "41"];
        },
        (cells) => {
          cells[cells.length - 1] = ["text", "30"];
        },
        (cells) => {
          cells[event ? 8 : 7] = event ? ["text", "system"] : ["integer", "7"];
        },
      ];
    if (event)
      mutations.push((cells) => {
        cells[1] = ["integer", "0"];
      });
    for (const mutate of mutations) {
      const f = readbackAppendFixture(),
        row = f.current.appendRows[table]?.[1] as { cells: unknown[][]; sha256: string };
      const old = row.sha256;
      mutate(row.cells, f);
      row.sha256 = createHash("sha256").update(JSON.stringify(row.cells)).digest("hex");
      Reflect.deleteProperty(f.prepared.fingerprints[table] ?? {}, old);
      (f.prepared.fingerprints[table] as Record<string, number>)[row.sha256] = 1;
      f.prepared.operations.events = (f.current.appendRows.events ?? []).map((value) =>
        (value as { cells: unknown[] }).cells.slice(1, 4),
      );
      expect(await rejection(sqliteReadbackAppendWitness(f))).toBeInstanceOf(Error);
    }
    for (const extra of [false, true]) {
      const f = readbackAppendFixture();
      if (extra) f.current.appendRows[table]?.push(structuredClone(f.current.appendRows[table][1]));
      else f.current.appendRows[table]?.shift();
      expect(await rejection(sqliteReadbackAppendWitness(f))).toBeInstanceOf(Error);
    }
  }
});

test("PRODUCT4 uses the same publication contract for every native receipt, including compacted operations", () => {
  const fixture = () => {
    const f = readbackAppendFixture();
    // A product target is new. Compaction may remove its updates, but every
    // acknowledged receipt still requires its original event and audit row.
    f.current.appendRows.document_collab_updates = [];
    return {
      ...f,
      original: { appendRows: { events: [], audit_log: [] } },
      before: { fingerprints: { events: {}, audit_log: {} } },
      receipts: f.current.receipts as {
        seq: number;
        op: string;
        len: number;
        sha: string;
        actor: string;
      }[],
      clocks: ["0", "40"] as [string, string],
    };
  };
  const valid = fixture(),
    seal = sqliteAppendPublicationWitness(valid);
  expect(seal.count).toBe(3);
  expect(Object.values(seal.rows).map((rows) => Object.keys(rows).length)).toEqual([3, 3]);
  expect(Object.keys(seal.eventOperations)).toHaveLength(3);
  for (const table of ["events", "audit_log"] as const)
    for (const extra of [false, true]) {
      const f = fixture();
      if (extra) f.current.appendRows[table]?.push(structuredClone(f.current.appendRows[table][0]));
      else f.current.appendRows[table]?.pop();
      expect(() => sqliteAppendPublicationWitness(f)).toThrow();
    }
  for (const mutate of [
    (f: ReturnType<typeof fixture>) => {
      f.receipts.pop();
    },
    (f: ReturnType<typeof fixture>) => {
      f.receipts[1] = structuredClone(f.receipts[0] as (typeof f.receipts)[number]);
    },
    (f: ReturnType<typeof fixture>) => {
      f.current.writer_generation = 3;
    },
    (f: ReturnType<typeof fixture>) => {
      f.prepared.fingerprints.audit_log = {};
    },
  ]) {
    const f = fixture();
    mutate(f);
    expect(() => sqliteAppendPublicationWitness(f)).toThrow();
  }
});

test("actual waited logged command returns empty streams; only its completed private packet is parsed", async () => {
  const base = directory(),
    packet = join(base, "observation.json"),
    log = join(base, "native.log");
  writeFileSync(log, "", { mode: 0o600 });
  const value = {
    connectionClose: "confirmed",
    lifecycleDrain: "confirmed",
    leases: 0,
    payload: "exact native packet",
  };
  const script = `const { publishSelectedSqlitePacket } = await import(${JSON.stringify(join(root, "tools/turso/ui-container.ts"))});
    console.error("PRIVATE_DIAGNOSTIC is not JSON");
    publishSelectedSqlitePacket(${JSON.stringify(packet)}, ${JSON.stringify(value)});`;
  const result = await actualCommand([process.execPath, "--no-env-file", "-e", script], {
    cwd: root,
    log,
    waitOnInterrupt: true,
  });
  expect(result).toEqual({ returncode: 0, stdout: "", stderr: "" });
  expect(readFileSync(log, "utf8")).toContain("PRIVATE_DIAGNOSTIC is not JSON");
  expect(readSelectedSqlitePacket(packet)).toEqual(value);
  expect(statSync(packet).mode & 0o777).toBe(0o600);
  // Strict packet parsing never falls back to the mixed diagnostic transcript.
  for (const bytes of [
    Buffer.from("{}\nPRIVATE_DIAGNOSTIC"),
    Buffer.from([0xff]),
    Buffer.from("[]"),
  ]) {
    writeFileSync(packet, bytes);
    expect(() => readSelectedSqlitePacket(packet)).toThrow();
  }
  chmodSync(packet, 0o644);
  expect(() => readSelectedSqlitePacket(packet)).toThrow();
  chmodSync(packet, 0o600);
  truncateSync(packet, 64 * 1024 * 1024);
  expect(() => readSelectedSqlitePacket(packet)).toThrow();
  const link = join(base, "link.json");
  symlinkSync(packet, link);
  expect(() => readSelectedSqlitePacket(link)).toThrow();
});
function restoreReport() {
  return {
    config: { workers: 1 },
    errors: [],
    stats: { expected: 1, unexpected: 0, flaky: 0, skipped: 0 },
    suites: [
      {
        specs: [
          {
            file: restoreSpec,
            title: restoreBrowserTitle,
            ok: true,
            tests: [
              {
                expectedStatus: "passed",
                results: [{ status: "passed", retry: 0, errors: [] }],
              },
            ],
          },
        ],
      },
    ],
  };
}
const rejection = (outcome: Promise<unknown>) =>
  outcome.then(
    (value: unknown) => ({ resolved: value }),
    (error: unknown) => error,
  );
function logged(options: CommandOptions | undefined, output: string): void {
  if (options?.log !== undefined) writeFileSync(options.log, output);
}
// An owned long-lived child that has already exited with `code`.
const exitedChild = (code = 0): Child => ({
  exited: Promise.resolve(code),
  exitCode: code,
  signalCode: null,
  kill: () => undefined,
});
// Real migration definitions and compiled registry, as the server applies them.
function realMigrations(): [number, string, string][] {
  const folder = join(root, "migrations/sqlite/060");
  const definitions = readdirSync(folder)
    .filter((file) => /^[0-9][0-9]_.*\.sql$/.test(file))
    .sort()
    .map((file) => ({ stem: file.slice(0, -4), sha256: sha(join(folder, file)) }));
  return expectedMigrations(definitions, readFileSync(join(root, "src/db/migrate.rs"), "utf8"));
}

interface World {
  base: string;
  run: string;
  driver: string;
  current: Current;
  source: Record<string, string>;
  executables: Record<string, string>;
  calls: string[][];
  emitted: string[];
  seam: Seam;
}
// A fake host boundary: docker, the server process, HTTP and the browser are
// scripted; files, hashes, receipts and the real migration registry are real.
function world(
  flow: "on" | "off",
  options: {
    browserExit?: number;
    command?: (args: string[]) => Completed | undefined;
  } = {},
): World {
  const base = directory();
  const runtime = join(base, "runtime");
  mkdirSync(runtime);
  const run = join(runtime, "root-current-sqlite-0123456789ab");
  const target = join(base, "target");
  mkdirSync(target);
  const executables: Record<string, string> = {};
  const binaries: Record<string, unknown> = {};
  for (const name of ["fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"]) {
    const path = join(target, name);
    writeFileSync(path, "synthetic " + name);
    executables[name] = path;
    binaries[path] = { sha256: sha(path), target: { name } };
  }
  const bun = join(base, "bun");
  writeFileSync(bun, "synthetic bun");
  mkdirSync(join(base, "chromium"));
  const chromium = join(base, "chromium", "chrome");
  writeFileSync(chromium, "synthetic chromium");
  const manifestPath = join(base, "binding.json");
  writeFileSync(manifestPath, "{}");
  const driver = join(base, "driver.ts");
  writeFileSync(driver, "synthetic driver");
  const before = {
    head: SOURCE,
    tree: TREE,
    status: "",
    tracked: { "a.txt": "1".repeat(64) },
    external: { "/external": "2".repeat(64) },
    untracked: {},
  };
  const current = {
    manifest: { schema: 1, ready: true, flow, source: SOURCE, tree: TREE, compiledSource: SOURCE },
    manifestPath,
    grant: { runId: "1", runAttempt: "1" },
    run,
    before,
    sourceBefore: structuredClone(before),
    build: { binaries },
    compileReceipt: {},
    assets: { source: SOURCE, dist_files: { "index.html": "3".repeat(64) } },
    abi: { host_runtime_files: {} },
    flow,
  } as unknown as Current;
  const source = {
    FVOCI_CI_SELECTED_RUNS: runtime,
    FVOCI_CI_OWNER: OWNER,
    FVOCI_ROOT_RUN_OWNER: OWNER,
    FVOCI_CI_BUN: bun,
    TMPDIR: base,
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: join(base, "cache"),
    PATH: "/usr/bin:/bin",
  };
  const calls: string[][] = [],
    emitted: string[] = [];
  const migrations = realMigrations();
  const seam: Seam = {
    trap: () => undefined,
    loadCurrent: () => Promise.resolve(current),
    command(args, commandOptions) {
      calls.push(args);
      const scripted = options.command?.(args);
      if (scripted !== undefined) {
        logged(commandOptions, scripted.stdout);
        return Promise.resolve(scripted);
      }
      const [tool, verb] = args;
      if (tool === bun && verb === "--eval") return Promise.resolve(ok(chromium + "\n"));
      if (tool === bun) {
        if (args.includes(restoreSpec)) {
          logged(commandOptions, "  1 passed\n");
          writeFileSync(
            commandOptions?.env?.PLAYWRIGHT_JSON_OUTPUT_FILE as string,
            JSON.stringify(restoreReport()),
          );
          return Promise.resolve(ok());
        }
        const exit = options.browserExit ?? 0;
        logged(commandOptions, exit ? "  1 failed\n" : "  1 passed (3.0s)\n");
        writeFileSync(
          join(run, "database", "actor-fresh.json"),
          JSON.stringify({
            backend: "sqlite",
            commit: "confirmed",
            poolClosed: true,
            connectionClose: "confirmed",
            operationSucceeded: true,
          }),
        );
        if (flow === "off") writeFileSync(join(run, "playwright-result.private.json"), "{}");
        return Promise.resolve({ returncode: exit, stdout: "", stderr: "" });
      }
      const text = args.join(" ");
      let result = ok();
      if (text.includes("fvoci-runtime-abi")) result = ok("qualified dependency\n");
      else if (args.includes("sha256sum"))
        result = ok(
          ["fvoci-server", "fvoci-migrate", "collab-engine"]
            .map((name) => sha(executables[name] as string) + "  /fvoci/bin/" + name)
            .join("\n") + "\n",
        );
      else if (text.includes("{{.HostConfig.NetworkMode}}")) result = ok("host\n");
      else if (text.includes("{{json .NetworkSettings.Networks}}"))
        result = ok(JSON.stringify({ host: { NetworkID: NETWORK } }));
      else if (verb === "network") result = ok(NETWORK + " host\n");
      else if (verb === "inspect")
        result = { returncode: 1, stdout: "", stderr: "Error: No such object: owned\n" };
      logged(commandOptions, result.stdout);
      return Promise.resolve(result);
    },
    spawnServer(_args, log) {
      const db = join(run, "database", "app.sqlite");
      writeFileSync(db, "");
      chmodSync(db, 0o600);
      writeSync(log, "fvoci-server listening on http://127.0.0.1:43210\n");
      return exitedChild();
    },
    ownedRows: () =>
      Promise.resolve([
        {
          pid: 10,
          parent: 1,
          uid: identity[0],
          gid: identity[1],
          args: "/fvoci/bin/fvoci-server",
          namespace_pid: 7,
          start_ticks: "5",
        },
      ]),
    identityGone: () => true,
    pgSql: () => Promise.reject(runtimeError("no PostgreSQL in the sqlite lane")),
    portClosed: () => Promise.resolve(true),
    probeSetup: () => Promise.resolve({ status: 200, body: { needed: true } }),
    inputCheck: () => Promise.resolve(structuredClone(before)),
    treeHashes: (path): Record<string, string> =>
      path === join(root, "apps/web/dist")
        ? { "index.html": "3".repeat(64) }
        : { chrome: "4".repeat(64) },
    migrationRows: () => migrations,
    validateOffReport: () => Array.from({ length: 8 }, (_, index) => "off " + String(index)),
    products: () => {
      calls.push(["PRODUCT4-PARENT-BINDING"]);
      return Promise.resolve({
        afterSeedStopped: () => Promise.resolve(),
        afterReadback: () => Promise.resolve(),
        afterRestartStopped: () => Promise.resolve(),
      });
    },
    restart: (context) => {
      expect(context.sqliteProducts).toBeDefined();
      return Promise.resolve({ restartBrowserExit: 0 });
    },
    emit(line) {
      emitted.push(line);
    },
    actor: () => identity,
  };
  return { base, run, driver, current, source, executables, calls, emitted, seam };
}
const receiptOf = (run: string) =>
  JSON.parse(readFileSync(join(run, "receipt.json"), "utf8")) as Record<string, unknown>;
const runMain = (fixture: World) => main(fixture.seam, fixture.driver, fixture.source, identity);
const lastLine = (fixture: World) =>
  JSON.parse(fixture.emitted.at(-1) as string) as Record<string, unknown>;

describe("selected sqlite driver body", () => {
  test("restore config uses the allocated output directory and one exact spec", async () => {
    const run = directory();
    const childEnv: Record<string, string | undefined> = {
      ...process.env,
      CI: "true",
      FVOCI_E2E_SELECTED_BACKEND: "sqlite",
      FVOCI_E2E_SELECTED_FLOW: "on",
      FVOCI_E2E_RESULT_DIR: run,
    };
    delete childEnv.JEST_WORKER_ID;
    delete childEnv.FVOCI_E2E_PENDING;
    const child = Bun.spawn(
      [
        process.execPath,
        "--no-env-file",
        "--eval",
        'import config from "./apps/web/e2e-pending/revision-restore-playwright.config.ts"; ' +
          "console.log(JSON.stringify({testDir:config.testDir,testMatch:config.testMatch," +
          "outputDir:config.outputDir,trace:config.use?.trace,workers:config.workers,retries:config.retries}));",
      ],
      { cwd: root, env: childEnv, stdout: "pipe", stderr: "pipe" },
    );
    const [stdout, stderr, exit] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ]);
    expect(stderr).toBe("");
    expect(exit).toBe(0);
    expect(JSON.parse(stdout)).toEqual({
      testDir: ".",
      testMatch: "**/v060-sqlite-revision-restore.spec.ts",
      outputDir: join(run, "playwright-output"),
      trace: "retain-on-failure",
      workers: 1,
      retries: 0,
    });
  });
  test("selected restore is absent from normal workspace browser discovery", () => {
    expect(
      discoverGroups()
        .flat()
        .some((spec) => basename(spec) === basename(restoreSpec)),
    ).toBe(false);
  });
  test("restore report requires the one exact case with no skips or retries", () => {
    expect(() => {
      validateRestoreReport(parseJson(JSON.stringify(restoreReport())));
    }).not.toThrow();
    const oneCase = (r: ReturnType<typeof restoreReport>) => {
      const item = r.suites[0]?.specs[0];
      if (!item) throw new Error("Missing fixture case");
      return item;
    };
    const oneResult = (r: ReturnType<typeof restoreReport>) => {
      const result = oneCase(r).tests[0]?.results[0];
      if (!result) throw new Error("Missing fixture result");
      return result;
    };
    const changes = [
      (r: ReturnType<typeof restoreReport>) => {
        r.stats.expected = 0;
        r.suites = [];
      },
      (r: ReturnType<typeof restoreReport>) => {
        r.stats.skipped = 1;
      },
      (r: ReturnType<typeof restoreReport>) => {
        r.stats.flaky = 1;
      },
      (r: ReturnType<typeof restoreReport>) => {
        oneCase(r).title = "unexpected test";
      },
      (r: ReturnType<typeof restoreReport>) => {
        oneResult(r).retry = 1;
      },
      (r: ReturnType<typeof restoreReport>) => {
        oneResult(r).status = "skipped";
      },
      (r: ReturnType<typeof restoreReport>) => {
        const suite = r.suites[0];
        if (!suite) throw new Error("Missing fixture suite");
        suite.specs.push(structuredClone(oneCase(r)));
      },
    ];
    for (const change of changes) {
      const report = restoreReport();
      change(report);
      expect(() => {
        validateRestoreReport(parseJson(JSON.stringify(report)));
      }).toThrow();
    }
  });
  for (const flow of ["on", "off"] as const)
    test(`an ${flow} pass is a qualified runtime retirement`, async () => {
      const fixture = world(flow);
      expect(await runMain(fixture)).toBe(0);
      const receipt = receiptOf(fixture.run);
      expect(receipt.final_exit_code).toBe(0);
      expect(receipt.cleanup_errors).toEqual([]);
      expect(Object.hasOwn(receipt, "diagnostic_errors")).toBe(false);
      expect(receipt.actual_browser_tests).toBe(flow === "off" ? 8 : 1);
      expect(fixture.calls.filter((args) => args[0] === "PRODUCT4-PARENT-BINDING").length).toBe(
        flow === "on" ? 1 : 0,
      );
      expect(statSync(join(fixture.run, "normal-server.log")).mode & 0o777).toBe(0o600);
      const restoreCalls = fixture.calls.filter((args) => args.includes(restoreSpec));
      expect(restoreCalls.length).toBe(flow === "on" ? 1 : 0);
      if (flow === "on") {
        const args = restoreCalls[0] as string[];
        expect(args[args.indexOf("--config") + 1]).toBe(
          "e2e-pending/revision-restore-playwright.config.ts",
        );
      }
      expect(receipt.actual_migration_rows).toEqual(realMigrations());
      expect(laneRetirement(fixture.run, "sqlite", flow, SOURCE, TREE, OWNER, 0).qualified).toBe(
        true,
      );
      expect(statSync(join(fixture.run, "environment.private.sh")).mode & 0o777).toBe(0o600);
      // Restart runs for the ON flow only; the OFF report is validated instead.
      expect(receipt.current_schema_server_restart).toEqual(
        flow === "on" ? { restartBrowserExit: 0 } : undefined,
      );
      expect(receipt.actual_off_titles === undefined).toBe(flow === "on");
      expect(lastLine(fixture).final_exit_code).toBe(0);
    });

  test("the browser exit is the driver exit and its log digest survives cleanup", async () => {
    const fixture = world("on", { browserExit: 7 });
    expect(await runMain(fixture)).toBe(7);
    const packet = JSON.parse(
      readFileSync(join(fixture.run, "original-failure.private.json"), "utf8"),
    ) as Record<string, unknown>;
    expect(packet.failure_code).toBe("SELECTED_BODY_NONZERO");
    expect(packet.original_driver_failure).toEqual({
      type: "ReturnedNonzero",
      phase: "browser",
      observedExit: 7,
    });
    expect(packet.original_body_log_sha256).toBe(sha(join(fixture.run, "browser.log")));
    expect(packet.observed_failed_exit).toBe(7);
    expect(packet.known_driver_checkpoint).toBeNull();
    expect(receiptOf(fixture.run).current_schema_server_restart).toBeUndefined();
    expect(laneRetirement(fixture.run, "sqlite", "on", SOURCE, TREE, OWNER, 7).qualified).toBe(
      true,
    );
    expect(fixture.calls.some((args) => args[1] === "rm")).toBe(true);
  });

  test("restore output EACCES is public-safe even before JSON reporting starts", async () => {
    const fixture = world("on", {
      command: (args) =>
        args.includes(restoreSpec)
          ? {
              returncode: 1,
              stdout: `EACCES: permission denied, mkdir '/${PRIVATE}/test-results'\n`,
              stderr: "",
            }
          : undefined,
    });
    expect(await runMain(fixture)).toBe(1);
    const receipt = receiptOf(fixture.run);
    expect(receipt).toMatchObject({
      browser_exit: 0,
      observed_failed_exit: 1,
      restore_browser_exit: 1,
      restore_failure_code: "PLAYWRIGHT_OUTPUT_EACCES",
      browser_report_state: "report-missing",
      known_browser_checkpoint: null,
      cleanup_errors: [],
      owned_container_absent: true,
    });
    expect(receipt.current_schema_server_restart).toBeUndefined();
    const log = join(fixture.run, "restore-browser.log");
    expect(receipt.original_body_log_sha256).toBe(sha(log));
    expect(statSync(log).mode & 0o777).toBe(0o600);
    expect(lastLine(fixture)).toMatchObject({
      restore_failure_code: "PLAYWRIGHT_OUTPUT_EACCES",
      restore_browser_exit: 1,
      browser_report_state: "report-missing",
    });
    expect(fixture.emitted.join("")).not.toContain(PRIVATE);
  });

  test("a restore assertion keeps its exit, private report and public source location", async () => {
    const fixture = world("on", {
      command(args) {
        if (!args.includes(restoreSpec)) return undefined;
        const report = restoreReport();
        const spec = report.suites[0]?.specs[0];
        if (!spec) throw new Error("missing restore case");
        spec.ok = false;
        const result = spec.tests[0]?.results[0];
        if (!result) throw new Error("missing restore result");
        Object.assign(result, {
          status: "failed",
          errorLocation: { file: restoreSpec, line: 83 },
          error: { message: PRIVATE, stack: PRIVATE },
        });
        writeFileSync(
          join(fixture.run, "restore-playwright-result.private.json"),
          JSON.stringify(report),
        );
        return { returncode: 7, stdout: PRIVATE, stderr: "" };
      },
    });
    expect(await runMain(fixture)).toBe(7);
    const report = join(fixture.run, "restore-playwright-result.private.json");
    expect(statSync(report).mode & 0o777).toBe(0o600);
    expect(receiptOf(fixture.run)).toMatchObject({
      browser_exit: 0,
      restore_browser_exit: 7,
      sqlite_restore_report_sha256: sha(report),
      browser_report_state: "matched",
      known_browser_test: restoreBrowserTitle,
      known_browser_status: "failed",
      known_browser_checkpoint: restoreSpec + ":83",
      observed_failed_exit: 7,
      cleanup_errors: [],
    });
    expect(lastLine(fixture).known_browser_checkpoint).toBe(restoreSpec + ":83");
    expect(fixture.emitted.join("")).not.toContain(PRIVATE);
  });

  test("a refused grant creates no run root and no container", async () => {
    const fixture = world("on");
    fixture.seam.loadCurrent = () => Promise.reject(new Error("NOT GRANTED"));
    expect(await rejection(runMain(fixture))).toMatchObject({ message: "NOT GRANTED" });
    expect(fixture.calls).toEqual([]);
    expect(() => statSync(fixture.run)).toThrow();
  });

  test("a failed docker create keeps its step and exit and removes nothing", async () => {
    const fixture = world("on", {
      command: (args) =>
        args[1] === "create"
          ? { returncode: 125, stdout: "", stderr: "SYNTHETIC_PRIVATE_DAEMON" }
          : undefined,
    });
    expect(await runMain(fixture)).toBe(1);
    const receipt = receiptOf(fixture.run);
    expect(receipt.failed_phase).toBe("container-prepare");
    expect(receipt.known_driver_checkpoint).toBe(checkpointPrefix + "container-create");
    expect(receipt.preparation_command_exit).toBe(125);
    expect(fixture.calls.map((args) => args[1])).toEqual(["create"]);
    expect(receipt.cleanup_errors).toEqual([
      "owned loopback port never observed; retirement remains unqualified",
    ]);
    expect(fixture.emitted.join("")).not.toContain("SYNTHETIC_PRIVATE");
  });

  test("an entrypoint that exits before listening is a server-startup failure", async () => {
    const fixture = world("on");
    fixture.seam.spawnServer = (_args, log) => {
      writeSync(log, "synthetic entrypoint exit\n");
      return exitedChild(1);
    };
    expect(await runMain(fixture)).toBe(1);
    const receipt = receiptOf(fixture.run);
    expect(receipt.failed_phase).toBe("server-startup");
    expect(receipt.original_driver_failure).toEqual({
      type: "AssertionError",
      message: "normal entrypoint exited before listen; see actual raw log",
    });
    expect(receipt.normal_server_exit).toBe(1);
    expect(receipt.owned_container_absent).toBe(true);
    expect(receipt.cleanup_errors).toContain("normal-server-finish-unconfirmed");
  });
});

// One preparation run with a scripted docker boundary.
function preparation(base: string, script: (args: string[]) => Completed) {
  const run = join(base, "run");
  mkdirSync(run);
  const binaries = Object.fromEntries(
    ["server", "migrate", "engine"].map((name) => [join(base, name), { sha256: "h" }]),
  );
  const state = {
    current: { build: { binaries } },
    run,
    name: "synthetic-owned",
    owner: OWNER,
    dbroot: join(base, "db"),
    storage: join(base, "storage"),
    dist: join(base, "dist"),
    server: join(base, "server"),
    migrate: join(base, "migrate"),
    engine: join(base, "engine"),
    receipt: { phase: "container-prepare" },
    step: null,
    created: false,
  } as unknown as State;
  const seam = {
    command(args: string[], options?: CommandOptions) {
      const result = script(args);
      logged(options, result.stdout);
      return Promise.resolve(result);
    },
  } as unknown as Seam;
  return { state, seam, run };
}
function dockerScript(fault?: string, mode = "host\n", attachments?: unknown, driver?: string) {
  return (args: string[]): Completed => {
    const json = args[3] === "{{json .NetworkSettings.Networks}}";
    if (
      (args[1] === "inspect" && fault === "query") ||
      (args[1] === "inspect" && json && fault === "attachment-query") ||
      (args[1] === "network" && fault === "driver-query")
    )
      return { returncode: 7, stdout: "", stderr: "SYNTHETIC_PRIVATE_QUERY" };
    if (args[1] === "inspect" && fault === "spawn")
      throw Object.assign(new Error("SYNTHETIC_PRIVATE_SPAWN"), { name: "OSError" });
    if (args.includes("fvoci-runtime-abi"))
      return ok(fault === "ldd" ? "dependency not found\n" : "qualified dependency\n");
    if (args.includes("sha256sum"))
      return ok(["h", "h", "h"].map((h) => (fault === "hash" ? "wrong" : h) + " file").join("\n"));
    if (args[1] === "network")
      return ok(driver ?? NETWORK + (fault === "network" ? " bridge\n" : " host\n"));
    if (args[1] === "inspect")
      return ok(json ? JSON.stringify(attachments ?? { host: { NetworkID: NETWORK } }) : mode);
    return ok();
  };
}

describe("selected sqlite preparation", () => {
  test("host network proof uses the attached driver, not the mode rendering", async () => {
    // Alternate mode strings are synthetic compatibility cases.
    const cases: [string, unknown, string, boolean][] = [
      ["host\n", { host: { NetworkID: NETWORK } }, NETWORK + " host\n", true],
      [NETWORK + "\n", { host: { NetworkID: NETWORK } }, NETWORK + " host\n", true],
      [
        "WARNING: synthetic stderr\nhost\n",
        { host: { NetworkID: NETWORK } },
        NETWORK + " host\n",
        true,
      ],
      ["host\n", { bridge: { NetworkID: NETWORK } }, NETWORK + " bridge\n", false],
      ["host\n", { host: { NetworkID: NETWORK } }, NETWORK + " Host\n", false],
      ["host\n", { host: { NetworkID: NETWORK } }, "d".repeat(64) + " host\n", false],
      ["host\n", {}, "", false],
      [
        "host\n",
        { host: { NetworkID: NETWORK }, bridge: { NetworkID: "d".repeat(64) } },
        "",
        false,
      ],
    ];
    for (const [mode, attachments, driver, accepted] of cases) {
      const { state, seam, run } = preparation(
        directory(),
        dockerScript(undefined, mode, attachments, driver),
      );
      const outcome = prepareContainer(state, seam);
      if (accepted) await outcome;
      else expect(await rejection(outcome)).toBeInstanceOf(Error);
      expect(readFileSync(join(run, "actual-network-mode.log"), "utf8")).toBe(mode);
      expect(state.receipt.last_preparation_command_exit).toBe(0);
    }
  });

  test("assertions and command failures record the step and exit before cleanup", async () => {
    const cases: [string, number | null, string][] = [
      ["ldd", 0, "runtime-ldd-dependencies"],
      ["hash", 0, "copied-hashes-match"],
      ["network", 0, "network-driver-host"],
      ["query", 7, "network-mode"],
      ["attachment-query", 7, "network-attachments"],
      ["driver-query", 7, "network-driver"],
      ["spawn", null, "network-mode"],
    ];
    for (const [fault, exit, step] of cases) {
      const { state, seam, run } = preparation(directory(), dockerScript(fault));
      const error = await rejection(prepareContainer(state, seam));
      expect(error).toBeInstanceOf(Error);
      recordFailure(state, null, { error });
      const path = join(run, "original-failure.private.json");
      const packet = JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
      expect(statSync(path).mode & 0o777).toBe(0o600);
      expect(packet.preparation_command_exit).toBe(exit);
      expect(packet.known_driver_checkpoint).toBe(checkpointPrefix + step);
      expect(publicCheckpoint(packet.known_driver_checkpoint)).toBe(checkpointPrefix + step);
      // A later cleanup failure never replaces the first outcome.
      state.receipt.last_preparation_command_exit = 99;
      state.step = "copy-static";
      recordFailure(state, 99, { error: runtimeError("SECOND_PRIVATE_CLEANUP") });
      expect(JSON.parse(readFileSync(path, "utf8"))).toEqual(packet);
      expect(state.receipt.preparation_command_exit).toBe(exit);
      expect(state.receipt.observed_failed_exit).toBeNull();
    }
  });

  test("a checkpoint is a fixed step name, never derived from the error", () => {
    const { state, run } = preparation(directory(), dockerScript());
    state.step = "copy-inputs";
    const forged = runtimeError(checkpointPrefix + "forged-step PRIVATE_URL_cookie");
    forged.stack = "at " + checkpointPrefix + "forged-step";
    recordFailure(state, null, { error: forged });
    expect(state.receipt.known_driver_checkpoint).toBe(checkpointPrefix + "copy-inputs");
    const packet = JSON.parse(
      readFileSync(join(run, "original-failure.private.json"), "utf8"),
    ) as Record<string, unknown>;
    expect(packet.known_driver_checkpoint).toBe(checkpointPrefix + "copy-inputs");
    // Before the first owned step nothing is named; outside preparation the
    // field is not recorded at all.
    const first = preparation(directory(), dockerScript());
    recordFailure(first.state, null, { error: runtimeError("PRIVATE_URL_cookie") });
    expect(first.state.receipt.known_driver_checkpoint).toBeNull();
    const browser = preparation(directory(), dockerScript());
    browser.state.receipt.phase = "browser";
    browser.state.step = "copy-inputs";
    recordFailure(browser.state, 1, { error: runtimeError("PRIVATE") });
    expect(browser.state.receipt.known_driver_checkpoint).toBeUndefined();
  });

  test("public checkpoint projection accepts only a registered step", () => {
    for (const step of preparationSteps)
      expect(publicCheckpoint(checkpointPrefix + step)).toBe(checkpointPrefix + step);
    for (const value of [
      null,
      244,
      "https://private.example/a",
      "/foreign/tools/selected-backend-ci/drivers/sqlite.ts#copy-inputs",
      "scripts/selected-backend-ci/current-sqlite-driver.py:244",
      "tools/selected-backend-ci/drivers/postgres.ts#copy-inputs",
      checkpointPrefix,
      checkpointPrefix + "unknown-step",
      checkpointPrefix + "copy-inputs\nPRIVATE",
      checkpointPrefix + "copy-inputs ",
    ])
      expect(publicCheckpoint(value)).toBeNull();
  });
});

describe("selected sqlite finalization", () => {
  test("container absence requires a positive docker no-such answer", () => {
    for (const [returncode, stderr, expected] of [
      [1, "No such container: owned", true],
      [1, "Error: No such object: owned", true],
      [1, "permission denied", false],
      [1, "daemon unreachable", false],
      [0, "No such container: owned", false],
    ] as const)
      expect(positiveDockerAbsence({ returncode, stderr })).toBe(expected);
  });

  test("an empty process observation cannot claim retired identities", () => {
    const row = {} as Row;
    expect(retiredIdentities([], () => true)).toBe(false);
    expect(retiredIdentities([row], () => false)).toBe(false);
    expect(retiredIdentities([row], () => true)).toBe(true);
  });

  test("the summary line discloses digests only", () => {
    const receipt = {
      source: SOURCE,
      original_driver_failure: { type: "AssertionError", message: PRIVATE },
      driver_error: PRIVATE,
      final_exit_code: 7,
      failed_phase: "server-startup",
      retained_private_evidence: PRIVATE,
    };
    const line = JSON.stringify(summary(receipt, 7, []));
    expect(line).not.toContain(PRIVATE);
    const parsed = JSON.parse(line) as Record<string, unknown>;
    expect(String(parsed.original_driver_failure_sha256)).toMatch(/^[0-9a-f]{64}$/);
    expect(parsed.final_exit_code).toBe(7);
    expect(parsed.failure_code).toBe("SELECTED_DRIVER_FAILED");
  });

  // The first original failure precedes every cleanup observation; each
  // secondary fault is recorded and never replaces the first exit.
  async function exercise(fault?: string) {
    const fixture = world("on");
    const run = fixture.run;
    mkdirSync(run);
    mkdirSync(join(run, "database"));
    const log = join(run, "normal-server.log");
    writeFileSync(log, "private synthetic body log");
    const logFd = fault === "log-close" ? 2 ** 30 : openSync(log, "a");
    if (fault === "log-hash") chmodSync(log, 0);
    if (fault === "receipt") writeFileSync(join(run, "receipt.json"), "occupied");
    const attempted: string[][] = [];
    const packet = join(run, "original-failure.private.json");
    const cleanupFault = () =>
      Object.assign(new Error("SECOND_PRIVATE_CLEANUP"), { name: "OSError" });
    const server: Child = {
      exited: fault === "wait" ? Promise.reject(cleanupFault()) : Promise.resolve(0),
      exitCode: fault === "wait" ? null : 0,
      signalCode: null,
      kill: () => {
        if (fault === "wait") throw cleanupFault();
        return true;
      },
    };
    server.exited.catch(() => undefined);
    const seam: Seam = {
      ...fixture.seam,
      command(args) {
        attempted.push(args.slice(0, 2));
        expect(statSync(packet).isFile()).toBe(true);
        if (fault === "remove" && args[1] === "rm") throw cleanupFault();
        if (fault === "inspect" && args[1] === "inspect") throw cleanupFault();
        return Promise.resolve(
          args[1] === "inspect"
            ? { returncode: 1, stdout: "", stderr: "No such container: owned" }
            : ok(),
        );
      },
      ownedRows: () =>
        fault === "rows" ? Promise.reject(cleanupFault()) : Promise.resolve([{ pid: 22 } as Row]),
      identityGone: (row) => row.pid === 22,
      portClosed: () => (fault === "port" ? Promise.reject(cleanupFault()) : Promise.resolve(true)),
      inputCheck: () =>
        fault === "inputs"
          ? Promise.reject(cleanupFault())
          : Promise.resolve(fixture.current.before),
    };
    const state = {
      current: fixture.current,
      run,
      name: "owned",
      head: SOURCE,
      tree: TREE,
      dbroot: join(run, "database"),
      storage: join(run, "storage"),
      dist: join(root, "apps/web/dist"),
      bun: fixture.source.FVOCI_CI_BUN,
      server: fixture.executables["fvoci-server"],
      migrate: fixture.executables["fvoci-migrate"],
      fixture: fixture.executables["fvoci-e2e-fixture"],
      engine: fixture.executables["collab-engine"],
      before: fixture.current.before,
      sourceBefore: fixture.current.before,
      receipt: {
        source: SOURCE,
        tree: TREE,
        root_owner: OWNER,
        selected_flow: "on",
        phase: "browser",
        browser_exit: 7,
        owned_container_absent: null,
        owned_loopback_port_closed: null,
        recorded_process_identities_retired: null,
      },
      step: null,
      created: true,
      serverProcess: server,
      serverLog: logFd,
      base: "http://127.0.0.1:12345",
      serverRow: { pid: 22, namespace_pid: 22 } as Row,
      browserInputs: null,
      code: 7,
    } as unknown as State;
    recordFailure(state, 7, { error: runtimeError("FIRST_PRIVATE_ORIGINAL") });
    expect(await finalize(state, seam)).toBe(7);
    const original = JSON.parse(readFileSync(packet, "utf8")) as Record<string, unknown>;
    expect(original.observed_failed_exit).toBe(7);
    expect(original.original_driver_failure).toEqual({
      type: "RuntimeError",
      message: "FIRST_PRIVATE_ORIGINAL",
    });
    const line = fixture.emitted.join("");
    const published = lastLine(fixture);
    expect(published.final_exit_code).toBe(7);
    expect(published.failed_phase).toBe("browser");
    expect(line).not.toContain("FIRST_PRIVATE_ORIGINAL");
    expect(line).not.toContain("SECOND_PRIVATE_CLEANUP");
    if (fault) expect((published.cleanup_failure_codes as string[]).length).toBeGreaterThan(0);
    if (fault === "rows") expect(state.receipt.recorded_process_identities_retired).toBeNull();
    if (fault === "port") expect(state.receipt.owned_loopback_port_closed).toBeNull();
    if (fault === "inspect") expect(state.receipt.owned_container_absent).toBeNull();
    expect(attempted).toContainEqual(["docker", "inspect"]);
    if (fault !== "receipt") expect(receiptOf(run).final_exit_code).toBe(7);
    return published;
  }
  test("finalization survives each secondary fault", async () => {
    expect((await exercise()).cleanup_failure_codes).toEqual([]);
    for (const [fault, code] of [
      ["rows", "process-observation-failed"],
      ["remove", "owned-container-removal-failed"],
      ["inspect", "owned-container-absence-failed"],
      ["wait", "docker-exec-wait-failed"],
      ["log-close", "server-log-close-failed"],
      ["log-hash", "server-log-hash-failed"],
      ["port", "loopback-port-observation-failed"],
      ["inputs", "post-input-check-failed"],
      ["receipt", "final-receipt-write-failed"],
    ] as const)
      expect((await exercise(fault)).cleanup_failure_codes).toContain(code);
  });

  test("an unobserved port and the original phase are recorded as such", async () => {
    const fixture = world("on");
    mkdirSync(fixture.run);
    const state = {
      current: fixture.current,
      run: fixture.run,
      head: SOURCE,
      tree: TREE,
      dbroot: join(fixture.run, "database"),
      storage: join(fixture.run, "storage"),
      dist: join(root, "apps/web/dist"),
      server: fixture.executables["fvoci-server"],
      migrate: fixture.executables["fvoci-migrate"],
      fixture: fixture.executables["fvoci-e2e-fixture"],
      engine: fixture.executables["collab-engine"],
      before: fixture.current.before,
      sourceBefore: fixture.current.before,
      receipt: {
        phase: "container-prepare",
        owned_container_absent: null,
        owned_loopback_port_closed: null,
        loopback_port_observation: "not-observed",
        recorded_process_identities_retired: null,
      },
      step: null,
      created: false,
      serverProcess: null,
      serverLog: null,
      base: null,
      serverRow: null,
      browserInputs: null,
      code: 7,
    } as unknown as State;
    const error = new Error(PRIVATE);
    error.name = "AssertionError";
    recordFailure(state, null, { error });
    expect(await finalize(state, fixture.seam)).toBe(7);
    expect(state.receipt.owned_loopback_port_closed).toBeNull();
    expect(state.receipt.failed_phase).toBe("container-prepare");
    expect(state.receipt.original_driver_failure).toEqual({
      type: "AssertionError",
      message: PRIVATE,
    });
    expect(state.receipt.cleanup_errors).toEqual([
      "owned loopback port never observed; retirement remains unqualified",
    ]);
  });
});

test("environment.sh is POSIX sh exports in shlex.quote form", () => {
  expect(shellExports({ A: "x", B: `{"k":"it's"}`, C: "" })).toBe(
    `export A=x\nexport B='{"k":"it'"'"'s"}'\nexport C=''\n`,
  );
});

test("SIGINT kills and reaps the owned body child, then finalizes with exit 1", async () => {
  const base = directory();
  const child = Bun.spawn(
    [process.execPath, join(import.meta.dir, "sqlite-interrupt.fixture.ts"), base],
    {
      cwd: root,
      env: {
        ...process.env,
        FVOCI_CI_SELECTED_RUNS: join(base, "runtime"),
        FVOCI_CI_OWNER: OWNER,
        FVOCI_ROOT_RUN_OWNER: OWNER,
        FVOCI_CI_BUN: process.execPath,
      },
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  try {
    const stdout = await new Response(child.stdout).text();
    expect(await child.exited).toBe(1);
    const blocked = Number(readFileSync(join(base, "blocker.log"), "utf8").trim());
    expect(Number.isSafeInteger(blocked) && blocked > 0).toBe(true);
    expect(() => process.kill(blocked, 0)).toThrow();
    const receipt = receiptOf(join(base, "runtime", "root-current-sqlite-0123456789ab"));
    expect(receipt.failed_phase).toBe("container-prepare");
    expect(receipt.original_driver_failure).toMatchObject({ type: "KeyboardInterrupt" });
    expect(receipt.known_driver_checkpoint).toBe(checkpointPrefix + "container-start");
    expect(receipt.preparation_command_exit).toBeNull();
    expect(receipt.final_exit_code).toBe(1);
    const calls = readFileSync(join(base, "calls.jsonl"), "utf8")
      .trim()
      .split("\n")
      .map((line) => (JSON.parse(line) as string[])[1]);
    expect(calls).toEqual(["create", "start", "rm", "inspect"]);
    expect((JSON.parse(stdout.trim()) as Record<string, unknown>).final_exit_code).toBe(1);
  } finally {
    if (child.exitCode === null) child.kill("SIGKILL");
    await child.exited;
  }
});
