import { afterEach, describe, expect, test } from "bun:test";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { root, sha } from "../io.ts";
import { laneRetirement } from "../runtime.ts";
import type { Current } from "./binding.ts";
import { runtimeError } from "./common.ts";
import type { Child, CommandOptions, Completed, Row } from "./common.ts";
import {
  checkpointPrefix,
  expectedMigrations,
  finalize,
  main,
  positiveDockerAbsence,
  preparationSteps,
  prepareContainer,
  publicCheckpoint,
  recordFailure,
  retiredIdentities,
  shellExports,
  summary,
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
    restart: () => Promise.resolve({ restartBrowserExit: 0 }),
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
  for (const flow of ["on", "off"] as const)
    test(`an ${flow} pass is a qualified runtime retirement`, async () => {
      const fixture = world(flow);
      expect(await runMain(fixture)).toBe(0);
      const receipt = receiptOf(fixture.run);
      expect(receipt.final_exit_code).toBe(0);
      expect(receipt.cleanup_errors).toEqual([]);
      expect(receipt.actual_browser_tests).toBe(flow === "off" ? 8 : 1);
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
