import { describe, expect, test } from "bun:test";
import { createHash, randomUUID } from "node:crypto";
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
import { join, relative } from "node:path";
import process from "node:process";
import { parseJson, sha } from "../io.ts";
import {
  attachmentJson,
  command,
  decode,
  failureCheckpoint,
  fileSha,
  field,
  frameLine,
  identityGone,
  killTree,
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

// A process is alive unless /proc no longer lists it or lists it as dead.
function alive(pid: number): boolean {
  let stat: string;
  try {
    stat = readFileSync(`/proc/${String(pid)}/stat`, "latin1");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  return !["Z", "X"].includes(stat.charAt(stat.lastIndexOf(")") + 2));
}
// Every live process carrying the token in its environment, which survives
// exec and is inherited by every process of a tree.
const marked = (token: string) =>
  readdirSync("/proc")
    .filter((entry) => /^[0-9]+$/.test(entry))
    .map(Number)
    .filter((pid) => {
      try {
        return readFileSync(`/proc/${String(pid)}/environ`, "latin1").includes(token) && alive(pid);
      } catch (error) {
        // Gone, or another user's process.
        if (["ENOENT", "ESRCH", "EACCES"].includes(String((error as NodeJS.ErrnoException).code)))
          return false;
        throw error;
      }
    });
const sigkill = (pids: number[]) => {
  for (const pid of pids)
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // Already gone.
    }
};
// Runs the fixture driver in its own process group: a group interrupt reaches
// the wrapper and its foreground command like a terminal ^C; a direct one
// reaches the driver alone, which forwards it to the wrapper shell only.
async function interruptFixture(
  shape: "trap" | "ignore" | "escape" | "foreign",
  target: "group" | "direct",
  grace: number,
  check: (facts: {
    directory: string;
    token: string;
    result: unknown;
    elapsed: number;
    foreground: number;
  }) => void,
  cleanup: (directory: string) => void = () => undefined,
) {
  const directory = temporary();
  const token = `fvoci-wrapper-${randomUUID()}`;
  const fixture = join(import.meta.dir, "wrapper-interrupt.fixture.ts");
  const child = Bun.spawn([process.execPath, fixture, directory, shape, String(grace)], {
    env: { ...process.env, FVOCI_WRAPPER_TOKEN: token },
    stdout: "pipe",
    stderr: "inherit",
    detached: true,
  });
  try {
    const log = join(directory, "wrapper.log");
    while (!(existsSync(log) && readFileSync(log, "utf8").includes("ready"))) await Bun.sleep(20);
    const foreground = Number(readFileSync(join(directory, "foreground.pid"), "utf8"));
    const started = performance.now();
    process.kill(target === "group" ? -child.pid : child.pid, "SIGINT");
    const output = await new Response(child.stdout).text();
    expect(await child.exited).toBe(0);
    const elapsed = performance.now() - started;
    const result: unknown = JSON.parse(output.split("\n").filter(Boolean).at(-1) ?? "{}");
    check({ directory, token, result, elapsed, foreground });
  } finally {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      // The fixture group has already exited.
    }
    sigkill(marked(token));
    cleanup(directory);
    rmSync(directory, { recursive: true });
  }
}
const interrupted = { error: "KeyboardInterrupt", message: "selected driver interrupted" };
for (const [shape, target, grace, cleaned] of [
  ["ignore", "group", 1000, false],
  ["trap", "direct", 1000, false],
  ["trap", "group", 3000, true],
] as const)
  test(`a ${target} SIGINT to a ${shape === "trap" ? "trapping" : "SIGINT-ignoring"} fixture wrapper ends within its grace`, async () => {
    await interruptFixture(shape, target, grace, ({ directory, result, elapsed, foreground }) => {
      expect(result).toEqual(interrupted);
      // A cooperative wrapper is waited for, not killed; any other is
      // SIGKILLed with its foreground command once the grace has passed.
      expect(existsSync(join(directory, "cleaned"))).toBe(cleaned);
      if (cleaned) expect(elapsed).toBeLessThan(grace);
      else expect(elapsed).toBeGreaterThanOrEqual(grace);
      expect(elapsed).toBeLessThan(grace + 2000);
      expect(alive(foreground)).toBe(false);
    });
  });
// Each worker forks its sleeper and exits only once the wrapper has stopped,
// so the sleeper is orphaned while the walk is under way. The wrapper is a
// child subreaper and adopts it, where init would otherwise.
test("a SIGKILLed fixture wrapper leaves no orphan of its tree alive", async () => {
  await interruptFixture("escape", "direct", 300, ({ token, result, elapsed, foreground }) => {
    expect(result).toEqual(interrupted);
    expect(elapsed).toBeGreaterThanOrEqual(300);
    expect(elapsed).toBeLessThan(2300);
    expect(alive(foreground)).toBe(false);
    expect(marked(token)).toEqual([]);
  });
});
// A member the walk cannot signal survives the SIGKILL; the command reports
// that, not a clean interrupt. Linux runners have passwordless sudo, as the
// footer tests require.
test("a fixture wrapper tree that outlives its SIGKILL fails the command", async () => {
  let killed: number | null | undefined;
  await interruptFixture(
    "foreign",
    "direct",
    300,
    ({ directory, result }) => {
      expect(alive(Number(readFileSync(join(directory, "foreign.pid"), "utf8")))).toBe(true);
      expect(result).toEqual({
        error: "RuntimeError",
        message: expect.stringMatching(/^owned wrapper tree kill failed: /) as unknown,
      });
    },
    (directory) => {
      // sudo drops the token from the environment; the pid file names it.
      const recorded = join(directory, "foreign.pid");
      if (existsSync(recorded))
        killed = Bun.spawnSync([
          "sudo",
          "-n",
          "kill",
          "-KILL",
          readFileSync(recorded, "utf8").trim(),
        ]).exitCode;
    },
  );
  expect(killed).toBe(0);
});
test("a waited wrapper runs as a child subreaper in the driver's process group", async () => {
  const directory = temporary();
  try {
    const log = join(directory, "wrapper.log");
    // The subshell has exited before the read, so its sleep is already an
    // orphan, reparented to the nearest subreaper.
    const result = await command(
      [
        "bash",
        "-c",
        '(sleep 30 & echo $! > orphan); read -r stat < "/proc/$(cat orphan)/stat"; ' +
          'stat=${stat##*) }; set -- $stat; parent=$2; kill -KILL "$(cat orphan)"; ' +
          "read -r own < /proc/$$/stat; own=${own##*) }; set -- $own; " +
          'echo "orphan-parent=$parent wrapper=$$ pgid=$3 cwd=$PWD probe=$FVOCI_WRAPPER_PROBE"',
      ],
      {
        cwd: relative(process.cwd(), directory),
        env: { ...process.env, FVOCI_WRAPPER_PROBE: "probe value" },
        log,
        waitOnInterrupt: true,
      },
    );
    expect(result.returncode).toBe(0);
    const ownStat = readFileSync("/proc/self/stat", "latin1");
    const pgid = ownStat.slice(ownStat.lastIndexOf(")") + 2).split(" ")[2];
    const text = readFileSync(log, "utf8");
    const wrapper = / wrapper=([0-9]+) /.exec(text)?.[1];
    expect(text).toBe(
      `orphan-parent=${String(wrapper)} wrapper=${String(wrapper)} pgid=${String(pgid)} cwd=${directory} probe=probe value\n`,
    );
  } finally {
    rmSync(directory, { recursive: true });
  }
});
test("a waited wrapper that cannot be executed fails its setup, not with an exit code", async () => {
  const directory = temporary();
  try {
    const failure = await command([join(directory, "missing")], {
      log: join(directory, "wrapper.log"),
      required: false,
      waitOnInterrupt: true,
    }).catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).name).toBe("RuntimeError");
    expect((failure as Error).message).toBe(
      `owned wrapper exec setup failed; executable=${join(directory, "missing")}`,
    );
  } finally {
    rmSync(directory, { recursive: true });
  }
});
test("a waited wrapper finds its executable where Bun.spawn would", async () => {
  const directory = temporary();
  try {
    for (const [env, found] of [
      [{}, true],
      [{ PATH: "" }, true],
      [{ PATH: directory }, false],
    ] as const) {
      let direct: number | string;
      try {
        direct = Bun.spawnSync(["bash", "-c", "exit 3"], { env }).exitCode;
      } catch {
        direct = "not found";
      }
      const waited = await command(["bash", "-c", "exit 3"], {
        env,
        log: join(directory, "wrapper.log"),
        required: false,
        waitOnInterrupt: true,
      }).then(
        (completed) => completed.returncode,
        (error: unknown) => (error as Error).message,
      );
      expect([direct, waited]).toEqual(
        found ? [3, 3] : ["not found", "owned wrapper exec setup failed; executable=bash"],
      );
    }
  } finally {
    rmSync(directory, { recursive: true });
  }
});
// The tree keeps forking long-lived children while it reaps short-lived ones,
// so /proc children changes under a walk that reads it before the parent has
// stopped.
test("killTree leaves no process of a forking tree alive", async () => {
  const token = `fvoci-kill-tree-${randomUUID()}`;
  const loop = Bun.spawn(
    [
      "bash",
      "-c",
      'long=0; while :; do if [ "$long" -lt 300 ]; then sleep 60 & long=$((long + 1)); ' +
        '[ "$long" -eq 30 ] && echo ready; fi; /bin/true & done',
    ],
    {
      env: { ...process.env, FVOCI_KILL_TREE_TOKEN: token },
      stdin: "ignore",
      stdout: "pipe",
      stderr: "inherit",
    },
  );
  try {
    expect(decode((await loop.stdout.getReader().read()).value ?? new Uint8Array())).toBe(
      "ready\n",
    );
    killTree(loop.pid);
    expect(marked(token)).toEqual([]);
    await loop.exited;
    expect(loop.signalCode).toBe("SIGKILL");
  } finally {
    sigkill([loop.pid, ...marked(token)]);
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
  test("fileSha is io.ts sha across the reused buffer boundary", () => {
    const directory = temporary();
    try {
      for (const size of [0, 1, 1048575, 1048576, 1048577, 3 * 1048576 + 7]) {
        const path = join(directory, String(size));
        writeFileSync(path, Buffer.alloc(size, size % 251));
        expect(fileSha(path)).toBe(sha(path));
      }
      // A short file after a long one must not hash the long one's tail.
      expect(fileSha(join(directory, "1"))).toBe(sha(join(directory, "1")));
    } finally {
      rmSync(directory, { recursive: true });
    }
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
