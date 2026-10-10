import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { parseJson } from "../io.ts";
import {
  attachmentJson,
  command,
  decode,
  failureCheckpoint,
  field,
  frameLine,
  identityGone,
  knownBrowserCheckpoint,
  knownOnBrowserTest,
  lines,
  ownedObjectAbsent,
  ownedRows,
  pythonJson,
  shellQuote,
  type Command,
  type Receipt,
  type Row,
} from "./common.ts";

const secret = "http://secret.example/a cookie=PRIVATE_BROWSER_SECRET";
const specFile = "/opt/fvoci/apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts";
const temporary = () => mkdtempSync(join(tmpdir(), "fvoci-driver-common-"));
// Reports reach the projection through io.read, which keeps integer tokens.
const report = (result: unknown, title = knownOnBrowserTest, workers = "1") =>
  parseJson(
    `{"config":{"workers":${workers}},"suites":[{"specs":[${JSON.stringify({
      title,
      file: specFile,
      tests: [{ results: [result] }],
    })}]}]}`,
  );
const project = (result: unknown) => knownBrowserCheckpoint(report(result));

describe("known browser checkpoint", () => {
  test("reads only the emitted location fields", () => {
    const found = project({
      status: "failed",
      errorLocation: { file: specFile, line: 413, column: 5 },
      error: { message: secret, stack: secret, snippet: secret },
      errors: [{ message: secret, location: { file: specFile, line: 9, column: 1 } }],
    });
    expect(found).toEqual({
      known_browser_test: knownOnBrowserTest,
      known_browser_status: "failed",
      known_browser_checkpoint: "e2e-pending/workspace-wiki-selected-backend.spec.ts:413",
      browser_report_state: "matched",
    });
    expect(JSON.stringify(found)).not.toContain("PRIVATE");
  });
  test("a missing, zero or out-of-range location stays null", () => {
    const missing = project({ status: "failed", errors: [{ message: secret }] });
    expect(missing.known_browser_status).toBe("failed");
    expect(missing.known_browser_checkpoint).toBeNull();
    expect(JSON.stringify(missing)).not.toContain("PRIVATE");
    for (const line of ["0", "10001", "4.0", "-1"])
      expect(
        knownBrowserCheckpoint(
          parseJson(
            `{"config":{"workers":1},"suites":[{"specs":[{"title":${JSON.stringify(knownOnBrowserTest)},"file":${JSON.stringify(specFile)},"tests":[{"results":[{"status":"failed","errorLocation":{"file":${JSON.stringify(specFile)},"line":${line}}}]}]}]}]}`,
          ),
        ).known_browser_checkpoint,
      ).toBeNull();
  });
  test("timedOut, the auxiliary source and the first error location", () => {
    const timed = project({
      status: "timedOut",
      errorLocation: { file: specFile, line: 323, column: 1 },
    });
    expect(timed.known_browser_status).toBe("timedOut");
    expect(timed.known_browser_checkpoint).toBe(
      "e2e-pending/workspace-wiki-selected-backend.spec.ts:323",
    );
    const auxiliary = project({
      status: "failed",
      errors: [
        { location: { file: "C:\\w\\e2e-pending\\workspace-wiki-selected-auxiliary.ts", line: 7 } },
      ],
    });
    expect(auxiliary.known_browser_checkpoint).toBe(
      "e2e-pending/workspace-wiki-selected-auxiliary.ts:7",
    );
  });
  test("refusals keep their own state", () => {
    expect(knownBrowserCheckpoint(null).browser_report_state).toBe("report-unreadable");
    expect(
      knownBrowserCheckpoint(report({ status: "failed" }, knownOnBrowserTest, "2"))
        .browser_report_state,
    ).toBe("workers-not-one");
    // A float token is not the integer 1 the reporter writes.
    expect(
      knownBrowserCheckpoint(report({ status: "failed" }, knownOnBrowserTest, "1.0"))
        .browser_report_state,
    ).toBe("workers-not-one");
    expect(
      knownBrowserCheckpoint(report({ status: "failed" }, "other title")).browser_report_state,
    ).toBe("spec-mismatch");
    expect(project({ status: "passed" }).browser_report_state).toBe("status-not-known");
    expect(
      knownBrowserCheckpoint(parseJson('{"config":{"workers":1},"suites":[1]}'))
        .browser_report_state,
    ).toBe("report-unreadable");
    const restart = knownBrowserCheckpoint(
      report(
        { status: "interrupted" },
        "selected normal main restart: fresh actor reads persisted native history and manual revision",
      ),
      true,
    );
    expect(restart.browser_report_state).toBe("matched");
  });
});

describe("first failure packet", () => {
  test("an exception keeps the first outcome; a later failure does not replace it", () => {
    const directory = temporary();
    try {
      const receipt: Receipt = { phase: "server-ready" };
      const first = new Error("FIRST_PRIVATE");
      first.name = "AssertionError";
      failureCheckpoint(receipt, directory, null, { error: first });
      receipt.phase = "browser";
      failureCheckpoint(receipt, directory, 7, { error: new Error("SECOND") });
      expect(receipt.failed_phase).toBe("server-ready");
      expect(receipt.failure_code).toBe("SELECTED_DRIVER_EXCEPTION");
      expect(receipt.original_driver_failure).toEqual({
        type: "AssertionError",
        message: "FIRST_PRIVATE",
      });
      const path = join(directory, "original-failure.private.json");
      expect(statSync(path).mode & 0o777).toBe(0o600);
      expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({
        failed_phase: "server-ready",
        observed_failed_exit: null,
        failure_code: "SELECTED_DRIVER_EXCEPTION",
        original_driver_failure: { type: "AssertionError", message: "FIRST_PRIVATE" },
        original_body_log_sha256: null,
      });
      expect(receipt.original_failure_checkpoint_sha256).toBe(
        createHash("sha256").update(readFileSync(path)).digest("hex"),
      );
    } finally {
      rmSync(directory, { recursive: true });
    }
  });
  test("a nonzero body records its exit and log digest with extra keys", () => {
    const directory = temporary();
    try {
      writeFileSync(join(directory, "body.log"), "private body log");
      const receipt: Receipt = { phase: "browser", browser_report_state: "report-missing" };
      failureCheckpoint(receipt, directory, 7, {
        bodyLog: join(directory, "body.log"),
        packetName: "parent-original-failure.private.json",
        extraKeys: ["browser_report_state", "known_browser_test"],
      });
      const packet = JSON.parse(
        readFileSync(join(directory, "parent-original-failure.private.json"), "utf8"),
      ) as Receipt;
      expect(packet.original_driver_failure).toEqual({
        type: "ReturnedNonzero",
        phase: "browser",
        observedExit: 7,
      });
      expect(packet.failure_code).toBe("SELECTED_BODY_NONZERO");
      expect(packet.original_body_log_sha256).toBe(
        createHash("sha256").update("private body log").digest("hex"),
      );
      expect(packet.browser_report_state).toBe("report-missing");
      expect(packet.known_browser_test).toBeNull();
    } finally {
      rmSync(directory, { recursive: true });
    }
  });
  test("unwritable packet and missing log are diagnostics, not exceptions", () => {
    const receipt: Receipt = { phase: "browser" };
    failureCheckpoint(receipt, "/proc/1/fvoci-no-such-directory", 3, {
      bodyLog: "/proc/1/fvoci-no-such-log",
    });
    expect(receipt.diagnostic_errors).toEqual([
      "original-body-log-hash-failed",
      "original-failure-checkpoint-write-failed",
    ]);
    expect(receipt.original_body_log_sha256).toBeNull();
  });
});

describe("owned commands", () => {
  test("exit codes, signals, logs and the RuntimeError name", async () => {
    const directory = temporary();
    try {
      expect(
        (await command(["sh", "-c", "printf out; printf err >&2; exit 3"], { required: false }))
          .returncode,
      ).toBe(3);
      const captured = await command(["sh", "-c", "printf 'a\\nb'; printf e >&2"]);
      expect(captured).toEqual({ returncode: 0, stdout: "a\nb", stderr: "e" });
      expect((await command(["sh", "-c", "kill -TERM $$"], { required: false })).returncode).toBe(
        -15,
      );
      const log = join(directory, "owned.log");
      writeFileSync(log, "stale contents that must be truncated");
      await command(["sh", "-c", "printf one; printf two >&2"], { log });
      expect(readFileSync(log, "utf8")).toBe("onetwo");
      expect((await command(["cat"], { input: "stdin text" })).stdout).toBe("stdin text");
      const failure = await command(["false"]).catch((error: unknown) => error);
      expect(failure).toBeInstanceOf(Error);
      expect((failure as Error).name).toBe("RuntimeError");
      expect((failure as Error).message).toBe("owned command failed exit=1; executable=false");
      // Captured output is strict UTF-8; a BOM is data.
      expect((await command(["printf", "\\357\\273\\277x"])).stdout).toBe("\uFEFFx");
      expect(
        await command(["printf", "\\377"]).catch((error: unknown) => (error as Error).name),
      ).toBe("TypeError");
    } finally {
      rmSync(directory, { recursive: true });
    }
  });
  test("the first SIGINT kills the body child; cleanup commands still run", async () => {
    const fixture = join(import.meta.dir, "interrupt.fixture.ts");
    const child = Bun.spawn([process.execPath, fixture], { stdout: "pipe", stderr: "inherit" });
    const reader = child.stdout.getReader();
    const first = await reader.read();
    const head = decode(first.value ?? new Uint8Array());
    expect(head).toContain("started");
    const started = performance.now();
    child.kill("SIGINT");
    let output = head;
    for (;;) {
      const next = await reader.read();
      if (next.done) break;
      output += decode(next.value);
    }
    expect(await child.exited).toBe(0);
    expect(performance.now() - started).toBeLessThan(20_000);
    const result = JSON.parse(output.split("\n").filter(Boolean).at(-1) ?? "{}") as Record<
      string,
      unknown
    >;
    expect(result).toEqual({
      body: "KeyboardInterrupt",
      laterBody: "KeyboardInterrupt",
      cleanup: 0,
    });
  });
});

for (const target of ["group", "direct"] as const)
  test(`a ${target} SIGINT lets an owned fixture wrapper run its EXIT cleanup`, async () => {
    const directory = temporary();
    try {
      const fixture = join(import.meta.dir, "wrapper-interrupt.fixture.ts");
      // Its own process group: a group interrupt reaches the wrapper like a
      // terminal ^C; a direct one reaches the driver alone and is forwarded.
      const child = Bun.spawn([process.execPath, fixture, directory, target], {
        stdout: "pipe",
        stderr: "inherit",
        detached: true,
      });
      const log = join(directory, "wrapper.log");
      while (!(existsSync(log) && readFileSync(log, "utf8").includes("ready"))) await Bun.sleep(20);
      process.kill(target === "group" ? -child.pid : child.pid, "SIGINT");
      const output = await new Response(child.stdout).text();
      expect(await child.exited).toBe(0);
      expect(JSON.parse(output.split("\n").filter(Boolean).at(-1) ?? "{}")).toEqual({
        error: "KeyboardInterrupt",
      });
      expect(existsSync(join(directory, "cleaned"))).toBe(true);
    } finally {
      rmSync(directory, { recursive: true });
    }
  });

describe("Docker process identities", () => {
  const top =
    (stdout: string): Command =>
    () =>
      Promise.resolve({ returncode: 0, stdout, stderr: "" });
  test("a live row carries its namespace pid and start ticks; a vanished pid is retired", async () => {
    const own = process.pid;
    const rows = await ownedRows(
      "owned",
      top(
        `PID PPID UID GID COMMAND\n${String(own)} 1 1000 1000 /fvoci/bin/fvoci-server --flag  \n999999999 1 0 0 sh\n`,
      ),
    );
    expect(rows[0]?.pid).toBe(own);
    expect(rows[0]?.args).toBe("/fvoci/bin/fvoci-server --flag  ");
    expect(rows[0]?.namespace_pid).toBe(own);
    expect(typeof rows[0]?.start_ticks).toBe("string");
    expect(rows[1]).toEqual({
      pid: 999999999,
      parent: 1,
      uid: 0,
      gid: 0,
      args: "sh",
      already_retired_at_observation: true,
    });
    expect(identityGone(rows[0] as never)).toBe(false);
    expect(identityGone({ ...(rows[0] as Row), start_ticks: "0" })).toBe(true);
    expect(identityGone(rows[1] as never)).toBe(true);
    for (const row of ["1 2 3", "+1 2 3 4 x"])
      expect(
        await ownedRows("owned", top("PID PPID UID GID COMMAND\n" + row + "\n")).then(
          () => "accepted",
          () => "refused",
        ),
      ).toBe("refused");
  });
  test("only Docker's positive no-such answer proves absence", async () => {
    for (const [status, message, expected] of [
      [1, "Error: No such container: owned", true],
      [1, "Error: No such object: owned", true],
      [1, "Error: no such volume", true],
      [1, "permission denied", false],
      [1, "daemon unreachable", false],
      [0, "No such container: owned", false],
    ] as const) {
      const run: Command = () =>
        Promise.resolve({ returncode: status, stdout: "", stderr: message });
      expect(await ownedObjectAbsent(["docker", "inspect", "owned"], run)).toBe(expected);
    }
  });
});

describe("encodings", () => {
  test("python json bytes, shlex quoting, splitlines and strict base64", () => {
    expect(pythonJson({ a: ["é", 1], b: {} })).toBe(
      '{\n  "a": [\n    "\\u00e9",\n    1\n  ],\n  "b": {}\n}\n',
    );
    expect(shellQuote("")).toBe("''");
    expect(shellQuote("abc-1.2/x=y:z,@%+")).toBe("abc-1.2/x=y:z,@%+");
    expect(shellQuote(`{"k": "it's"}`)).toBe(`'{"k": "it'"'"'s"}'`);
    expect(lines("a\r\nb\rc\n")).toEqual(["a", "b", "c"]);
    expect(lines("")).toEqual([]);
    expect(attachmentJson(Buffer.from('{"a":1}').toString("base64"))).toEqual({ a: 1 });
    for (const body of ["eyJhIjoxfQ", "eyJhIjo xfQ==", 7, "eyJh\nIjoxfQ=="])
      expect(() => attachmentJson(body)).toThrow();
  });
  test("a required JSON member refuses when absent, even as undefined", () => {
    expect(field({ a: null }, "a")).toBeNull();
    for (const value of [{}, { b: 1 }, null, [1], "a"]) expect(() => field(value, "a")).toThrow();
  });
  test("frameLine names only this file's innermost frame", () => {
    const thrown = (() => {
      try {
        throw new Error("x");
      } catch (error) {
        return error;
      }
    })();
    expect(frameLine(thrown, import.meta.path)).toBeGreaterThan(0);
    expect(frameLine(thrown, "/elsewhere.ts")).toBeNull();
    expect(frameLine("not an error", import.meta.path)).toBeNull();
  });
});

// node:assert's equal/deepEqual family formats both operands into the
// message, which lands in receipts and driver logs; drivers use assert.ok.
test("driver sources never format assertion operands", () => {
  for (const name of readdirSync(import.meta.dir).filter(
    (entry) => entry.endsWith(".ts") && !entry.endsWith(".test.ts"),
  )) {
    const source = readFileSync(join(import.meta.dir, name), "utf8");
    expect([
      name,
      /assert\.(?:equal|deepEqual|strictEqual|notEqual|notDeepEqual|deepStrictEqual|match)\(/.test(
        source,
      ),
    ]).toEqual([name, false]);
    // Stdout carries only the summary line; the browser probe string is data.
    expect([name, /^\s*console\./m.test(source)]).toEqual([name, false]);
  }
});
