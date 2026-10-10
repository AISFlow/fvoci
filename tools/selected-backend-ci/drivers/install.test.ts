import { afterEach, describe, expect, test } from "bun:test";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { root, sha } from "../io.ts";
import { laneRetirement } from "../runtime.ts";
import type { Current } from "./binding.ts";
import { failureCheckpoint, runtimeError } from "./common.ts";
import type { CommandOptions, Completed } from "./common.ts";
import {
  assertProcessReceipts,
  completeTestRun,
  finalize,
  main,
  positiveDockerAbsence,
  summary,
} from "./install.ts";
import type { Seam, State } from "./install.ts";

const SOURCE = "a".repeat(40),
  TREE = "b".repeat(40),
  OWNER = "pure-owned-control",
  PRIVATE = "SYNTHETIC_PRIVATE_SECRET";
const scratch: string[] = [];
afterEach(() => {
  for (const path of scratch.splice(0)) rmSync(path, { recursive: true, force: true });
});
function directory(): string {
  const path = mkdtempSync(join(tmpdir(), "fvoci-install-driver-"));
  scratch.push(path);
  return path;
}
const ok = (stdout = ""): Completed => ({ returncode: 0, stdout, stderr: "" });
const passed = "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
const rejection = (outcome: Promise<unknown>) =>
  outcome.then(
    (value: unknown) => ({ resolved: value }),
    (error: unknown) => error,
  );

interface World {
  base: string;
  run: string;
  driver: string;
  current: Current;
  source: Record<string, string>;
  calls: string[][];
  emitted: string[];
  seam: Seam;
}
// Docker and the install test executable are scripted; files, hashes and
// receipts are real. `retained` is what `docker cp` brings back from /fvoci/run.
function world(
  options: {
    exit?: number;
    log?: string;
    retained?: (directory: string) => void;
    command?: (args: string[]) => Completed | undefined;
  } = {},
): World {
  const base = directory();
  const runtime = join(base, "runtime");
  mkdirSync(runtime);
  const run = join(runtime, "root-current-install-0123456789ab");
  const target = join(base, "target");
  mkdirSync(target);
  const binaries: Record<string, unknown> = {};
  for (const name of [
    "fvoci-server",
    "fvoci-migrate",
    "collab-engine",
    "selected_install_lifetime",
  ]) {
    const path = join(target, name === "selected_install_lifetime" ? name + "-0123" : name);
    writeFileSync(path, "synthetic " + name);
    binaries[path] = { sha256: sha(path), target: { name } };
  }
  const driver = join(base, "driver.ts");
  writeFileSync(driver, "synthetic driver");
  const before = {
    head: SOURCE,
    tree: TREE,
    status: "",
    tracked: { "package.json": sha(join(root, "package.json")) },
    external: {},
    untracked: {},
  };
  const current = {
    manifest: {
      schema: 1,
      ready: true,
      flow: "on",
      source: SOURCE,
      tree: TREE,
      compiledSource: SOURCE,
    },
    manifestPath: join(base, "binding.json"),
    grant: { runId: "1", runAttempt: "1" },
    run,
    before,
    build: { binaries },
    compileReceipt: {},
    assets: { source: SOURCE, dist_files: {} },
    abi: { host_runtime_files: {} },
    flow: "on",
  } as unknown as Current;
  const calls: string[][] = [],
    emitted: string[] = [];
  const retain =
    options.retained ??
    ((destination: string) => {
      for (let index = 0; index < 15; index += 1) {
        const child = join(destination, "children", String(index));
        mkdirSync(child, { recursive: true });
        writeFileSync(join(child, "process.json"), JSON.stringify({ status: 0 }));
      }
    });
  const seam: Seam = {
    trap: () => undefined,
    loadCurrent: () => Promise.resolve(current),
    command(args: string[], commandOptions?: CommandOptions) {
      calls.push(args);
      const scripted = options.command?.(args);
      if (scripted !== undefined) return Promise.resolve(scripted);
      if (args.includes("--test-threads=1")) {
        if (commandOptions?.log) writeFileSync(commandOptions.log, options.log ?? passed);
        return Promise.resolve({ returncode: options.exit ?? 0, stdout: "", stderr: "" });
      }
      if (args[1] === "cp" && args[2]?.endsWith(":/fvoci/run")) {
        retain(args[3] as string);
        return Promise.resolve(ok());
      }
      if (args.includes("fvoci-runtime-abi")) return Promise.resolve(ok("qualified dependency\n"));
      if (args[0] === "git") return Promise.resolve(ok(SOURCE + "\n"));
      if (args[1] === "inspect")
        return Promise.resolve({ returncode: 1, stdout: "", stderr: "Error: No such object: x" });
      if (commandOptions?.log) writeFileSync(commandOptions.log, "");
      return Promise.resolve(ok());
    },
    diskFree: () => 123,
    emit(line) {
      emitted.push(line);
    },
  };
  return { base, run, driver, current, source: { FVOCI_CI_OWNER: OWNER }, calls, emitted, seam };
}
const receiptOf = (run: string) =>
  JSON.parse(readFileSync(join(run, "receipt.json"), "utf8")) as Record<string, unknown>;
const runMain = (fixture: World) => main(fixture.seam, fixture.driver, fixture.source);
const docker = (fixture: World) =>
  fixture.calls.filter((args) => args[0] === "docker").map((args) => args[1]);

describe("selected install driver body", () => {
  test("a pass is a qualified install retirement with 15 child receipts", async () => {
    const fixture = world();
    expect(await runMain(fixture)).toBe(0);
    const receipt = receiptOf(fixture.run);
    expect(receipt.actual_tests).toBe(4);
    expect(receipt.actual_owned_process_receipts).toBe(15);
    expect(receipt.cleanup_errors).toEqual([]);
    expect(receipt.final_source).toBe(SOURCE);
    expect(laneRetirement(fixture.run, "install", "on", SOURCE, TREE, OWNER, 0).qualified).toBe(
      true,
    );
    expect(statSync(join(fixture.run, "environment.private.json")).mode & 0o777).toBe(0o600);
    expect(fixture.calls.find((args) => args[1] === "create")).toContain("none");
    expect(JSON.parse(fixture.emitted.at(-1) as string)).toMatchObject({ final_exit_code: 0 });
  });

  test("a nonzero install body keeps its exit, phase and log digest", async () => {
    const fixture = world({ exit: 101, log: "test result: FAILED. 3 passed; 1 failed;\n" });
    expect(await runMain(fixture)).toBe(101);
    const packet = JSON.parse(
      readFileSync(join(fixture.run, "original-failure.private.json"), "utf8"),
    ) as Record<string, unknown>;
    expect(packet).toEqual({
      failed_phase: "install-body",
      observed_failed_exit: 101,
      failure_code: "SELECTED_BODY_NONZERO",
      original_driver_failure: {
        type: "ReturnedNonzero",
        phase: "install-body",
        observedExit: 101,
      },
      original_body_log_sha256: sha(join(fixture.run, "test.log")),
    });
    const receipt = receiptOf(fixture.run);
    expect(receipt.final_exit_code).toBe(101);
    expect(receipt.actual_tests).toBeUndefined();
    expect(receipt.owned_container_absent).toBe(true);
    expect(laneRetirement(fixture.run, "install", "on", SOURCE, TREE, OWNER, 101).qualified).toBe(
      false,
    );
  });

  test("a zero exit still needs the exact counts and every child receipt", async () => {
    for (const fixture of [
      world({ log: "test result: ok. 3 passed; 0 failed; 0 ignored;\n" }),
      world({ retained: () => undefined }),
      world({
        retained: (destination) => {
          mkdirSync(destination, { recursive: true });
          for (let index = 0; index < 15; index += 1)
            writeFileSync(
              join(destination, String(index) + "-process.json"),
              JSON.stringify({ status: null }),
            );
        },
      }),
    ]) {
      expect(await runMain(fixture)).toBe(1);
      const receipt = receiptOf(fixture.run);
      expect(receipt.failed_phase).toBe("install-body");
      expect(receipt.failure_code).toBe("SELECTED_DRIVER_EXCEPTION");
      expect(receipt.actual_owned_process_receipts).toBeUndefined();
    }
  });

  test("a refused grant creates no run root and no container", async () => {
    const fixture = world();
    fixture.seam.loadCurrent = () => Promise.reject(new Error("NOT GRANTED"));
    expect(await rejection(runMain(fixture))).toMatchObject({ message: "NOT GRANTED" });
    expect(fixture.calls).toEqual([]);
    expect(existsSync(fixture.run)).toBe(false);
  });

  test("an occupied run root is refused before any resource", async () => {
    const fixture = world();
    mkdirSync(fixture.run);
    expect(await rejection(runMain(fixture))).toMatchObject({
      message: "literal one-shot owned run; preserve original failures",
    });
    expect(fixture.calls).toEqual([]);
  });

  test("a failed docker create records the exception and removes nothing", async () => {
    const fixture = world({
      command: (args) =>
        args[1] === "create" ? { returncode: 125, stdout: "", stderr: PRIVATE } : undefined,
    });
    expect(await runMain(fixture)).toBe(1);
    const receipt = receiptOf(fixture.run);
    expect(receipt.failed_phase).toBe("container-prepare");
    expect(receipt.original_driver_failure).toMatchObject({ type: "RuntimeError" });
    expect(docker(fixture)).toEqual(["create"]);
    expect(receipt.owned_container_absent).toBeNull();
    expect(fixture.emitted.join("")).not.toContain(PRIVATE);
  });
});

describe("selected install finalization", () => {
  test("exact test count and child receipt checks", () => {
    expect(completeTestRun(passed)).toBe(true);
    expect(completeTestRun("test result: ok. 5 passed; 0 failed; 0 ignored;")).toBe(false);
    expect(completeTestRun("test result: ok. 4 passed; 0 failed; 1 ignored;")).toBe(false);
    assertProcessReceipts(Array.from({ length: 15 }, () => ({ status: 0 })));
    for (const records of [
      Array.from({ length: 14 }, () => ({ status: 0 })),
      Array.from({ length: 15 }, (_, index) => (index ? { status: 0 } : {})),
      Array.from({ length: 15 }, (_, index) => (index ? { status: 0 } : { status: null })),
    ])
      expect(() => {
        assertProcessReceipts(records);
      }).toThrow();
  });

  test("container absence requires a positive docker no-such answer", () => {
    for (const [returncode, stderr, expected] of [
      [1, "No such container: owned", true],
      [1, "permission denied", false],
      [1, "daemon unreachable", false],
      [0, "No such container: owned", false],
    ] as const)
      expect(positiveDockerAbsence({ returncode, stderr })).toBe(expected);
  });

  test("the summary line discloses digests only", () => {
    const line = JSON.stringify(
      summary(
        {
          source: SOURCE,
          original_driver_failure: { type: "AssertionError", message: PRIVATE },
          final_exit_code: 7,
        },
        7,
        [],
      ),
    );
    expect(line).not.toContain(PRIVATE);
    const parsed = JSON.parse(line) as Record<string, unknown>;
    expect(String(parsed.original_driver_failure_sha256)).toMatch(/^[0-9a-f]{64}$/);
    expect(parsed.failure_code).toBe("SELECTED_INSTALL_DRIVER_FAILED");
  });

  // The first original failure precedes every cleanup observation; each
  // secondary fault is recorded and never replaces the first exit.
  async function exercise(fault?: string) {
    const fixture = world();
    mkdirSync(fixture.run);
    writeFileSync(join(fixture.run, "test.log"), "private synthetic body log");
    const packet = join(fixture.run, "original-failure.private.json");
    const attempted: string[][] = [];
    const cleanupFault = () =>
      Object.assign(new Error("SECOND_PRIVATE_CLEANUP"), { name: "OSError" });
    if (fault === "receipt") writeFileSync(join(fixture.run, "receipt.json"), "occupied");
    const tracked = join(root, "package.json");
    const seam: Seam = {
      ...fixture.seam,
      command(args, options) {
        attempted.push(args.slice(0, 2));
        expect(statSync(packet).isFile()).toBe(true);
        if (fault === "remove" && args[1] === "rm") throw cleanupFault();
        if (fault === "inspect" && args[1] === "inspect") throw cleanupFault();
        if (fault === "copy" && args[1] === "cp") throw cleanupFault();
        return fixture.seam.command(args, options);
      },
    };
    const before =
      fault === "inputs"
        ? { ...fixture.current.before, tracked: { "package.json": "0".repeat(64) } }
        : fixture.current.before;
    const state = {
      current: fixture.current,
      run: fixture.run,
      name: "owned",
      before,
      receipt: {
        source: SOURCE,
        tree: TREE,
        root_owner: OWNER,
        phase: "install-body",
        exit_code: 7,
        owned_container_absent: null,
      },
      source: fixture.source,
      created: true,
      code: 7,
    } as unknown as State;
    failureCheckpoint(state.receipt, fixture.run, 7, {
      error: runtimeError("FIRST_PRIVATE_ORIGINAL"),
    });
    expect(await finalize(state, seam)).toBe(7);
    const published = JSON.parse(fixture.emitted.at(-1) as string) as Record<string, unknown>;
    const line = fixture.emitted.join("");
    expect(published.final_exit_code).toBe(7);
    expect(published.failed_phase).toBe("install-body");
    expect(line).not.toContain("FIRST_PRIVATE_ORIGINAL");
    expect(line).not.toContain("SECOND_PRIVATE_CLEANUP");
    if (fault === "inspect") expect(state.receipt.owned_container_absent).toBeNull();
    if (fault === "inputs") {
      expect(state.receipt.full_current_source_external_unchanged).toBe(false);
      expect(state.receipt.changed_inputs).toEqual(["package.json"]);
      expect(sha(tracked)).not.toBe("0".repeat(64));
    }
    expect(attempted).toContainEqual(["docker", "inspect"]);
    if (fault !== "receipt") expect(receiptOf(fixture.run).final_exit_code).toBe(7);
    return published;
  }
  test("finalization survives each secondary fault", async () => {
    expect((await exercise()).cleanup_failure_codes).toEqual([]);
    for (const [fault, code] of [
      ["remove", "owned-container-removal-failed"],
      ["inspect", "owned-container-absence-failed"],
      ["inputs", "install-post-input-unconfirmed"],
      ["receipt", "final-receipt-write-failed"],
      ["copy", "owned-receipt-copy-failed"],
    ] as const)
      expect((await exercise(fault)).cleanup_failure_codes).toContain(code);
  });
});

test("SIGINT kills and reaps the owned body child, then finalizes with exit 1", async () => {
  const base = directory();
  const child = Bun.spawn(
    [process.execPath, join(import.meta.dir, "install-interrupt.fixture.ts"), base],
    {
      cwd: root,
      env: { ...process.env, FVOCI_CI_OWNER: OWNER },
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
    const receipt = receiptOf(join(base, "runtime", "root-current-install-0123456789ab"));
    expect(receipt.failed_phase).toBe("container-prepare");
    expect(receipt.original_driver_failure).toMatchObject({ type: "KeyboardInterrupt" });
    expect(receipt.final_exit_code).toBe(1);
    expect(receipt.owned_container_absent).toBe(true);
    const calls = readFileSync(join(base, "calls.jsonl"), "utf8")
      .trim()
      .split("\n")
      .map((line) => (JSON.parse(line) as string[]).slice(0, 2).join(" "));
    expect(calls).toEqual([
      "docker create",
      "docker start",
      "docker cp",
      "docker rm",
      "docker inspect",
      "git -c",
    ]);
    expect((JSON.parse(stdout.trim()) as Record<string, unknown>).final_exit_code).toBe(1);
  } finally {
    if (child.exitCode === null) child.kill("SIGKILL");
    await child.exited;
  }
});
