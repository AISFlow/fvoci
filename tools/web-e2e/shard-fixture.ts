// Test-only checks for scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh:
// `bun tools/web-e2e/shard-fixture.ts <command> ...`. Each command runs real
// shell fragments of the web e2e scripts with their runtime stubbed and exits
// 1 with one stderr line on the first broken expectation.
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  chmodSync,
  rmSync,
  symlinkSync,
  readFileSync,
  realpathSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import process from "node:process";
import {
  TIMER_FILTERS,
  PAIR_FIRST,
  PAIR_SECOND,
  DEFAULT_SHARD_COUNT,
  shardPlanLines,
} from "./groups.ts";
import { groupDatabaseStatement } from "./database-urls.ts";
import { groupEnvironment, retainFailureArtifacts } from "./run-web-e2e.ts";

// Substitute only the Bun child executable in this fixture's private copied
// runtime. Production always uses process.execPath; native binaries stay real
// paths (the existing controlled migrate/server fixtures provide those files).
function fixtureRuntime(source: string, destination: string, executable: string): void {
  check(
    source !== destination &&
      realpathSync(destination) === destination &&
      /\/fvoci-web-e2e-failure-output\.[^/]+\/root\/tools\/web-e2e\/run-web-e2e\.ts$/.test(
        destination,
      ) &&
      statSync(destination).uid === process.getuid?.() &&
      realpathSync(executable) === executable,
    "fixture-owned runtime copy required",
  );
  const shared = join(dirname(dirname(destination)), "selected-backend-ci");
  mkdirSync(shared, { recursive: true });
  writeFileSync(
    join(shared, "types.ts"),
    readFileSync(join(dirname(source), "../selected-backend-ci/types.ts")),
  );
  const text = readFileSync(source, "utf8"),
    start = text.indexOf("async function groupRuntime("),
    end = text.indexOf("\nfunction checkedOutput(", start);
  check(start >= 0 && end > start, "group runtime boundary missing");
  const body = text.slice(start, end);
  check(body.split("process.execPath").length === 5, "four native Bun child boundaries required");
  // The copied wrapper/retention now also launches Bun children. Substitute
  // only this fixture-owned copy; native binaries remain direct paths.
  writeFileSync(destination, text.replaceAll("process.execPath", JSON.stringify(executable)));
}

async function fakeBun(args: string[]): Promise<void> {
  const state = process.env.FVOCI_FIXTURE_NET_STATE;
  if (!state) throw new FixtureError("fixture state required");
  if (
    args.length === 3 &&
    args[0] === "--no-env-file" &&
    args[1]?.endsWith("/tools/web-e2e/database-urls.ts")
  ) {
    const sql = groupDatabaseStatement(args[2] ?? "", process.env)
      .replace(/ PASSWORD '[^']*'/, " ")
      .replace(/\s+/g, " ");
    appendFileSync(state + ".sql", sql + "\n");
    process.exit(args[2] === "drop-database" && process.env.FVOCI_FIXTURE_DROP === "fail" ? 3 : 0);
  }
  if (
    args.some((arg) => arg.endsWith("/tools/web-e2e/trace-summary.ts")) &&
    process.env.FVOCI_FIXTURE_TRACE_SUMMARY === "fail"
  ) {
    process.stderr.write(
      "fixture summarizer error postgres://u:fixture-secret@h/db MEILI_MASTER_KEY=fixture-meili-master-secret\n",
    );
    process.exit(3);
  }
  if (args.slice(0, 5).join(" ") === "--bun x --no-install playwright test") {
    if (process.env.FVOCI_FIXTURE_RECORD_PLAYWRIGHT === "1") {
      appendFileSync(state + ".playwright-launches", "launch\n");
      if (args[5] === "--config=e2e/perf/perf.config.ts") process.exit(0);
    }
    if (process.env.FVOCI_FIXTURE_PLAYWRIGHT_EXIT)
      process.exit(Number(process.env.FVOCI_FIXTURE_PLAYWRIGHT_EXIT));
  }
  const child = Bun.spawn([process.execPath, ...args], {
    env: process.env,
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  if (args[0]?.endsWith("/tools/web-e2e/smtp-sink.ts"))
    writeFileSync(state + ".smtp-pid", String(child.pid));
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const)
    process.once(signal, () => {
      child.kill(signal);
    });
  process.exit(await child.exited);
}

class FixtureError extends Error {}

// Executables live only in the owned copied fixture. The production entry and
// all of its argument/qualification logic execute, with downstream tools stubbed.
export async function entryTool(tool: string, args: string[]): Promise<void> {
  if (tool === "ip") return;
  if (tool === "bun" && args[1]?.endsWith("/tools/web-e2e/run-web-e2e.ts")) {
    const mode = args[2];
    if (mode === "group-wrapper") {
      if (process.env.FVOCI_TEST_CANCEL_PENDING === "1") {
        const retired = new Promise<void>((resolve) => process.once("SIGTERM", resolve));
        process.kill(process.ppid, "SIGTERM");
        await retired;
        console.log("cancelled pending child normal exit");
        return;
      }
      console.log("fvoci-web-e2e-run-group " + args.slice(3).join(" "));
      process.exit(Number(process.env.FVOCI_TEST_RUN_GROUP_EXIT ?? "0"));
    }
    if (mode === "group-runtime" && process.env.FVOCI_FIXTURE_TIMER === "1") {
      if (process.env.FVOCI_FIXTURE_TIMER_NAMESPACES)
        appendFileSync(
          process.env.FVOCI_FIXTURE_TIMER_NAMESPACES,
          JSON.stringify({
            run: process.env.RUN_DIR,
            evidence: process.env.FVOCI_W5_EVIDENCE_DIR,
            root: process.env.ROOT,
            target: process.env.CARGO_TARGET_DIR,
          }) + "\n",
        );
      console.log(JSON.stringify(args.slice(3)));
      process.exit(args.includes(process.env.FVOCI_TEST_TIMER_FAIL_FILTER ?? "unset") ? 7 : 0);
    }
  }
  if (tool === "cargo") {
    console.error("fvoci-web-e2e-fake-cargo " + args.join(" "));
    process.exit(Number(process.env.FVOCI_TEST_CARGO_EXIT ?? "0"));
  }
  if (tool === "bash") {
    const name = args[0] ?? "";
    if (name.endsWith("/prepare-sqlite-ci.sh")) {
      check(args[1] === "--env-file" && !!args[2], "SQLite prerequisite argv");
      console.error("fvoci-web-e2e-fake-sqlite-prep");
      writeFileSync(
        args[2],
        "export SQLITE3_LIB_DIR=/fixture/sqlite/lib\nexport SQLITE3_INCLUDE_DIR=/fixture/sqlite/include\nexport SQLITE3_STATIC=1\nexport SQLITE3_NO_PKG_CONFIG=1\n",
      );
      return;
    }
    if (name.endsWith("/generate-api.sh")) {
      console.error("fvoci-web-e2e-fake-generate-api");
      return;
    }
    const child = Bun.spawn(["/bin/bash", ...args], {
      stdin: "inherit",
      stdout: "inherit",
      stderr: "inherit",
    });
    process.exit(await child.exited);
  }
  if (args.join(" ") === "--bun x --no-install playwright --version") return;
  if (args[0]?.endsWith("/run-selected-backend-e2e.ts")) {
    if (["record-before", "record-after", "stage"].includes(args[1] ?? "")) return;
    console.error("unexpected selected runtime after pending cancellation");
    process.exit(79);
  }
  if (args.join(" ") === "--bun run build") {
    console.error("fvoci-web-e2e-fake-bun-build");
    if (process.env.FVOCI_TEST_DIRTY_API_DURING_BUILD === "1")
      appendFileSync("src/generated/api.ts", "// dirty during build\n");
    process.exit(Number(process.env.FVOCI_TEST_BUN_BUILD_EXIT ?? "0"));
  }
  if (args[0]?.endsWith("/handoff.ts") && ["admit", "consume"].includes(args[1] ?? "")) {
    check(process.env.FVOCI_WEB_BUILD_PHASE === "consume", "handoff consume phase");
    console.log("fvoci-web-e2e-fake-handoff-" + String(args[1]));
    process.exit(args[1] === "consume" ? Number(process.env.FVOCI_TEST_HANDOFF_EXIT ?? "0") : 0);
  }
  const child = Bun.spawn([process.execPath, ...args], {
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  process.exit(await child.exited);
}

async function ciShardFixture(source: string): Promise<void> {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "fvoci-web-e2e-fixture.")));
  const checkout = join(directory, "checkout"),
    bin = join(directory, "bin");
  const previousPath = process.env.PATH;
  try {
    for (const path of [
      "scripts",
      "apps/web/src/generated",
      "tools/web-e2e",
      "tools/selected-backend-ci",
      "bin",
    ])
      mkdirSync(path === "bin" ? bin : join(checkout, path), { recursive: true });
    for (const file of [
      "scripts/run-web-e2e.sh",
      "tools/selected-backend-ci/types.ts",
      ...["groups.ts", "compat.ts", "run-web-e2e.ts"].map((name) => "tools/web-e2e/" + name),
    ]) {
      const text = readFileSync(join(source, file), "utf8");
      writeFileSync(
        join(checkout, file),
        file.endsWith("run-web-e2e.ts")
          ? text.replaceAll("process.execPath", JSON.stringify(join(bin, "bun")))
          : text,
      );
    }
    for (const tool of ["bun", "bash", "cargo", "ip"]) {
      const file = join(bin, tool);
      writeFileSync(
        file,
        "#!" +
          process.execPath +
          "\nimport {entryTool} from " +
          JSON.stringify(join(import.meta.dir, "shard-fixture.ts")) +
          ";await entryTool(" +
          JSON.stringify(tool) +
          ",process.argv.slice(2));\n",
      );
      chmodSync(file, 0o755);
    }
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      PATH: bin + ":" + (previousPath ?? ""),
      CARGO_TARGET_DIR: join(checkout, "target"),
    };
    for (const key of Object.keys(env))
      if (key.startsWith("GITHUB_") || key.startsWith("SQLITE3_") || key === "FVOCI_E2E_PENDING")
        Reflect.deleteProperty(env, key);
    const git = (...args: string[]) => {
      const out = Bun.spawnSync(["git", "-C", checkout, ...args], {
        stdout: "pipe",
        stderr: "pipe",
      });
      check(out.exitCode === 0, "fixture git command failed");
      return out.stdout.toString().trim();
    };
    const populate = () => {
      const e2e = join(checkout, "apps/web/e2e");
      mkdirSync(e2e, { recursive: true });
      for (const name of [
        PAIR_FIRST,
        PAIR_SECOND,
        ...Array.from({ length: 26 }, (_, index) => `extra-${String(index + 1)}-flow.spec.ts`),
      ])
        writeFileSync(join(e2e, name), "// fixture\n");
    };
    const invoke = (args: string[], extra: NodeJS.ProcessEnv = {}) => {
      const out = Bun.spawnSync(["/bin/bash", join(checkout, "scripts/run-web-e2e.sh"), ...args], {
        cwd: checkout,
        env: { ...env, ...extra },
        stdin: "ignore",
        stdout: "pipe",
        stderr: "pipe",
      });
      return { status: out.exitCode, text: out.stdout.toString() + out.stderr.toString() };
    };
    populate();
    const planned = shardPlanLines(join(checkout, "apps/web/e2e"), 0, DEFAULT_SHARD_COUNT).map(
      (line) => line.specs.join(" "),
    );
    const rows = (text: string) =>
      text
        .split("\n")
        .filter((line) => line.startsWith("fvoci-web-e2e-run-group "))
        .map((line) => line.slice("fvoci-web-e2e-run-group ".length));
    const count = (text: string, marker: string) => text.split(marker).length - 1;
    let result = invoke(["--host", "--ci-shard", "0"]);
    check(
      result.status === 0 &&
        count(result.text, "fvoci-web-e2e-fake-generate-api") === 1 &&
        same(rows(result.text), planned),
      "complete shard requires one build and exact ordered groups: " + result.text,
    );
    rmSync(join(checkout, "apps/web/e2e"), { recursive: true });
    mkdirSync(join(checkout, "apps/web/e2e"));
    result = invoke(["--host", "--ci-shard", "0"]);
    check(
      result.status !== 0 && !result.text.includes("fvoci-web-e2e-fake-generate-api"),
      "empty plan reached build",
    );
    populate();
    check(
      invoke(["--host", "--ci-shard", "0"], { FVOCI_TEST_RUN_GROUP_EXIT: "1" }).status === 1,
      "group failure must propagate",
    );
    check(
      invoke(["--host", "--ci-shard", "0"], { FVOCI_WEB_E2E_SHARD_COUNT: "16" }).status !== 0,
      "shard override accepted",
    );
    writeFileSync(join(checkout, "apps/web/openapi.json"), '{"openapi":"3.1.0"}\n');
    writeFileSync(join(checkout, "apps/web/src/generated/api.ts"), "// committed fixture types\n");
    git("init", "-q");
    git("add", "scripts", "apps");
    const commit = () =>
      git(
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "API fixture",
      );
    commit();
    const ci = (): NodeJS.ProcessEnv => ({
      CI: "true",
      GITHUB_ACTIONS: "true",
      GITHUB_JOB: "workspace-browser-shard",
      GITHUB_SHA: git("rev-parse", "HEAD"),
      FVOCI_SELECTED_CI_OUTPUT: join(checkout, "browser-output"),
      FVOCI_WEB_BUILD_HANDOFF: join(checkout, "browser-packet"),
      FVOCI_WEB_BUILD_HANDOFF_SHA256: "0".repeat(64),
    });
    const consumeArgs = ["--ci-use-committed-api", "--ci-consume-browser", "--ci-shard", "0"];
    const noBuild = (text: string) =>
      !/fvoci-web-e2e-fake-(generate-api|bun-build|cargo)/.test(text);
    result = invoke(consumeArgs, ci());
    check(
      result.status === 0 &&
        noBuild(result.text) &&
        count(result.text, "fvoci-web-e2e-fake-handoff-consume") === 1 &&
        same(rows(result.text), planned),
      "committed consumer must consume exact groups without build: " + result.text,
    );
    check(
      /web-e2e stage=browser-handoff-consume elapsed_seconds=[0-9.]+ exit=0/.test(result.text),
      "consumer timing missing",
    );
    const reject = (label: string, extra: NodeJS.ProcessEnv = {}, args: string[] = []) => {
      const out = invoke([...consumeArgs, ...args], { ...ci(), ...extra });
      check(
        out.status !== 0 && noBuild(out.text) && rows(out.text).length === 0,
        "invalid committed fixture reached preparation: " + label,
      );
    };
    reject("missing digest", { FVOCI_WEB_BUILD_HANDOFF_SHA256: "" });
    result = invoke(consumeArgs, { ...ci(), FVOCI_TEST_HANDOFF_EXIT: "7" });
    check(
      result.status === 7 && noBuild(result.text) && rows(result.text).length === 0,
      "artifact refusal did not propagate without fallback",
    );
    for (const extra of [
      { CI: "false" },
      { GITHUB_ACTIONS: "false" },
      { GITHUB_JOB: "web-checks" },
      { GITHUB_SHA: "0".repeat(40) },
    ])
      reject("authority", extra);
    reject("duplicate flag", {}, ["--ci-use-committed-api"]);
    const api = "apps/web/src/generated/api.ts",
      openapi = "apps/web/openapi.json";
    appendFileSync(join(checkout, api), "// dirty\n");
    reject("dirty");
    git("add", api);
    reject("staged dirty");
    git("restore", "--staged", "--worktree", api);
    git("update-index", "--assume-unchanged", api);
    appendFileSync(join(checkout, api), "// hidden dirty\n");
    reject("hidden dirty");
    git("update-index", "--no-assume-unchanged", api);
    git("restore", api);
    rmSync(join(checkout, openapi));
    reject("missing");
    git("restore", openapi);
    writeFileSync(join(checkout, openapi), "");
    reject("empty");
    git("restore", openapi);
    writeFileSync(join(checkout, "copied-openapi.json"), readFileSync(join(checkout, openapi)));
    rmSync(join(checkout, openapi));
    symlinkSync("../../copied-openapi.json", join(checkout, openapi));
    reject("symlink");
    rmSync(join(checkout, openapi));
    git("restore", openapi);
    git("rm", "--cached", "-q", api);
    reject("missing index");
    commit();
    reject("absent HEAD");
    git("add", api);
    commit();
    appendFileSync(join(checkout, "apps/web/e2e", PAIR_FIRST), "// dirty source\n");
    reject("dirty source");
    git("restore", "apps/web/e2e/" + PAIR_FIRST);
    const sqlite = {
      SQLITE3_LIB_DIR: "/fixture/sqlite/lib",
      SQLITE3_INCLUDE_DIR: "/fixture/sqlite/include",
      SQLITE3_STATIC: "1",
      SQLITE3_NO_PKG_CONFIG: "1",
    };
    const selected = {
      ...ci(),
      ...sqlite,
      GITHUB_JOB: "collaboration-flow",
      FVOCI_E2E_PENDING: "1",
      FVOCI_TEST_HANDOFF_EXIT: "7",
    };
    const selectedArgs = ["--ci-use-committed-api", "--ci-consume-selected"];
    result = invoke(selectedArgs, selected);
    check(
      result.status === 7 &&
        !/fvoci-web-e2e-fake-(sqlite-prep|generate-api|cargo)/.test(result.text) &&
        count(result.text, "fvoci-web-e2e-fake-bun-build") === 1 &&
        count(result.text, "committed API outputs match") === 2 &&
        count(result.text, "fvoci-web-e2e-fake-handoff-admit") === 1 &&
        count(result.text, "fvoci-web-e2e-fake-handoff-consume") === 1 &&
        rows(result.text).length === 0,
      "selected consumer rebuild or authority changed: " + result.text,
    );
    for (const key of ["SQLITE3_LIB_DIR", "SQLITE3_STATIC"]) {
      const extra: NodeJS.ProcessEnv = { ...selected };
      Reflect.deleteProperty(extra, key);
      result = invoke(selectedArgs, extra);
      check(
        result.status !== 0 &&
          result.text.includes(
            "selected consumer requires the workflow-prepared SQLite environment",
          ) &&
          !/fvoci-web-e2e-fake-(sqlite-prep|bun-build|cargo|handoff-consume)/.test(result.text),
        "missing selected SQLite export reached build",
      );
    }
    result = invoke(["--with-selected-backends"], {
      ...ci(),
      GITHUB_JOB: "collaboration-flow",
      FVOCI_E2E_PENDING: "1",
      FVOCI_TEST_CANCEL_PENDING: "1",
    });
    check(
      result.status === 143 &&
        result.text.includes("cancelled pending child normal exit") &&
        !result.text.includes("unexpected selected runtime"),
      "pending cancellation launched selected work or became success",
    );
    // Execute the real Bun wrapper, keeping exact filters, fresh initial
    // namespaces, explicit selection, stop-on-failure and retained evidence.
    process.env.PATH = env.PATH;
    process.env.FVOCI_FIXTURE_TIMER = "1";
    mkdirSync(join(checkout, "apps/web/dist"), { recursive: true });
    writeFileSync(join(checkout, "apps/web/dist/index.html"), "fixture");
    const timer = join(checkout, "tools/web-e2e/run-web-e2e.ts");
    const namespaces = join(directory, "timer-namespaces.jsonl");
    const timerRun = Bun.spawnSync(
      [
        process.execPath,
        "--no-env-file",
        timer,
        "group-wrapper",
        "--workers=1",
        "e2e/v050-task-timer.spec.ts",
        "--retries=0",
        "--trace=on",
      ],
      {
        env: {
          ...process.env,
          ROOT: checkout,
          CARGO_TARGET_DIR: env.CARGO_TARGET_DIR,
          FVOCI_E2E_SHARED_SERVICES: "1",
          TEST_DATABASE_URL: "fixture",
          FVOCI_TEST_PG_CONTAINER: "fixture",
          FVOCI_MEILI_URL: "fixture",
          FVOCI_MEILI_KEY: "fixture",
          MEILI_MASTER_KEY: "fixture",
          FVOCI_TEST_MEILI_CONTAINER: "fixture",
          FVOCI_FIXTURE_TIMER_NAMESPACES: namespaces,
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    check(timerRun.exitCode === 0, "timer dispatch failed");
    const timerLog = join(directory, "timer.jsonl");
    writeFileSync(timerLog, timerRun.stdout);
    timerRows(timerLog, join(source, "apps/web/e2e/v050-task-timer.spec.ts"));
    const contexts = readFileSync(namespaces, "utf8")
      .trim()
      .split("\n")
      .map(
        (line) =>
          JSON.parse(line) as { run: string; evidence: string; root: string; target: string },
      );
    check(
      contexts.length === 4 && new Set(contexts.map((context) => context.run)).size === 4,
      "timer invocations reused a runtime namespace",
    );
    check(
      contexts.every(
        (context) =>
          context.root === checkout &&
          context.target === env.CARGO_TARGET_DIR &&
          context.evidence === join(context.run, "playwright-output/w5-evidence") &&
          !existsSync(context.run),
      ),
      "timer initial environment or cleanup differs",
    );
    dispatchCases(timer, checkout);
    evidenceExport(checkout);
    await retention(checkout);
    console.log("run-ci-shard-fixture-test: ok");
  } finally {
    if (previousPath === undefined) Reflect.deleteProperty(process.env, "PATH");
    else process.env.PATH = previousPath;
    Reflect.deleteProperty(process.env, "FVOCI_FIXTURE_TIMER");
    rmSync(directory, { recursive: true, force: true });
  }
}

function check(condition: boolean, message: string): asserts condition {
  if (!condition) throw new FixtureError(message);
}

const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const same = (left: unknown, right: unknown): boolean =>
  JSON.stringify(left) === JSON.stringify(right);

type Env = Record<string, string | undefined>;

function argvRows(text: string): string[][] {
  if (text === "") return [];
  const lines = (text.endsWith("\n") ? text.slice(0, -1) : text).split("\n");
  return lines.map((line) => {
    const row: unknown = JSON.parse(line);
    check(
      Array.isArray(row) && row.every((item) => typeof item === "string"),
      `not an argv row: ${line}`,
    );
    return row;
  });
}

function timerTitles(spec: string): string[] {
  return [...readFileSync(spec, "utf8").matchAll(/^test\("([^"\n]+)"/gm)].map(
    (match) => match[1] ?? "",
  );
}

/** The four timer dispatch rows: original argv, groups.ts filters, a disjoint cover of the titles. */
function timerRows(log: string, spec: string): void {
  const rows = argvRows(readFileSync(log, "utf8"));
  const original = ["--workers=1", "e2e/v050-task-timer.spec.ts", "--retries=0", "--trace=on"];
  check(rows.length === 4, `expected 4 timer runs: ${JSON.stringify(rows)}`);
  check(
    rows.every((row) => same(row.slice(0, -2), original)),
    `timer runs changed the argv: ${JSON.stringify(rows)}`,
  );
  const f = TIMER_FILTERS;
  const expected = [
    ["--grep-invert", [f.new_control, f.recovery, f.restart].join("|")],
    ["--grep", f.recovery],
    ["--grep", f.restart],
    ["--grep", f.new_control],
  ];
  const actual = rows.map((row) => row.slice(-2));
  check(
    same(actual, expected),
    `timer dispatch filters differ from groups.ts: ${JSON.stringify(actual)}`,
  );
  const titles = timerTitles(spec);
  check(titles.length === 22, `expected 22 timer titles: ${JSON.stringify(titles)}`);
  const groups = rows.map((row) => {
    const pattern = new RegExp(row.at(-1) ?? "");
    return titles.filter((title) => pattern.test(title) === (row.at(-2) === "--grep"));
  });
  check(
    same(
      groups.map((group) => group.length),
      [9, 7, 1, 5],
    ),
    `timer group sizes: ${JSON.stringify(groups)}`,
  );
  const covered = new Set(groups.flat());
  check(
    same([...covered].sort(), [...new Set(titles)].sort()),
    "timer groups do not cover every title",
  );
  check(groups.flat().length === covered.size, "timer groups overlap");
  // The restart group consumes the whole DB graph and retires the group's
  // original server; it must stay the native restart fixture alone.
  const restart = titles.filter((title) => new RegExp(f.restart).test(title));
  const pinned =
    "native same-database restart preserves paused and running anchors for genuine new clients";
  check(
    same(restart, [pinned]),
    `restart group is not the native restart fixture alone: ${JSON.stringify(restart)}`,
  );
}

/** Explicit selections pass through once unchanged; each timer run's failure stops the dispatch. */
function dispatchCases(script: string, root: string): void {
  const env: Env = {
    ...process.env,
    ROOT: root,
    CARGO_TARGET_DIR: `${root}/target`,
    FVOCI_E2E_SHARED_SERVICES: "1",
    TEST_DATABASE_URL: "fixture",
    FVOCI_TEST_PG_CONTAINER: "fixture",
    FVOCI_MEILI_URL: "fixture",
    FVOCI_MEILI_KEY: "fixture",
    MEILI_MASTER_KEY: "fixture",
    FVOCI_TEST_MEILI_CONTAINER: "fixture",
  };
  const invoke = (args: string[], variables = env) => {
    const child = Bun.spawnSync(
      [process.execPath, "--no-env-file", script, "group-wrapper", ...args],
      { env: variables, stdin: "ignore", stdout: "pipe", stderr: "pipe" },
    );
    return {
      status: child.exitCode,
      stdout: UTF8.decode(child.stdout),
      stderr: UTF8.decode(child.stderr),
    };
  };
  const successful = (args: string[], variables = env) => {
    const result = invoke(args, variables);
    check(result.status === 0, `group dispatch failed: ${result.stderr}`);
    return result.stdout;
  };
  delete env.FVOCI_E2E_PENDING;
  delete env.FVOCI_TEST_TIMER_FAIL_FILTER;
  const cases = [
    ["v050-task-timer.spec.ts", "--grep", "literal owner title", "--workers=1"],
    ["--grep=literal owner title", "e2e/v050-task-timer.spec.ts"],
    ["v050-task-timer.spec.ts", "--grep-invert", "literal owner title"],
    ["v050-task-timer.spec.ts", "--grep-invert=literal owner title"],
    ["v050-task-timer.spec.ts", "-g", "literal owner title"],
    ["v050-task-timer.spec.ts", "-gliteral owner title"],
    ["v050-task-timer.spec.ts", "--shard=1/2"],
    ["v050-task-timer.spec.ts", "--shard", "1/2"],
    ["v050-task-timer.spec.ts", "--list"],
    ["v050-task-timer.spec.ts", "--", "literal owner title"],
    ["v050-task-timer.spec.ts", "other-flow.spec.ts"],
    ["other-flow.spec.ts", "--workers=1"],
    [],
  ];
  for (const args of cases) {
    const rows = argvRows(successful(args));
    check(
      same(rows, [args]),
      `explicit selection changed: ${JSON.stringify(args)} -> ${JSON.stringify(rows)}`,
    );
  }
  const pendingArgs = ["v050-task-timer.spec.ts", "--workers=1"];
  const pending = argvRows(successful(pendingArgs, { ...env, FVOCI_E2E_PENDING: "1" }));
  check(same(pending, [pendingArgs]), `pending selection changed: ${JSON.stringify(pending)}`);
  const runs = argvRows(successful(["v050-task-timer.spec.ts"]));
  check(runs.length === 4, `expected 4 timer runs: ${JSON.stringify(runs)}`);
  runs.forEach((run, index) => {
    const result = invoke(["v050-task-timer.spec.ts"], {
      ...env,
      FVOCI_TEST_TIMER_FAIL_FILTER: run.at(-1),
    });
    check(
      result.status === 7,
      `timer run ${String(index + 1)} failure exited ${String(result.status)}: ${result.stderr}`,
    );
    check(
      argvRows(result.stdout).length === index + 1,
      `timer runs continued after failure ${String(index + 1)}`,
    );
  });
}

/** The production initial namespace: per-run defaults; explicit paths survive. */
function evidenceExport(root: string): void {
  const clean: Env = { ...process.env };
  delete clean.FVOCI_W5_EVIDENCE_DIR;
  const evidence = (run: string, env: Env): string =>
    groupEnvironment(run, [], env).FVOCI_W5_EVIDENCE_DIR ?? "";
  for (const run of ["run-first", "run-second"]) {
    const directory = join(root, run);
    const value = evidence(directory, clean);
    check(value === `${directory}/playwright-output/w5-evidence`, `default evidence dir: ${value}`);
  }
  const explicit = join(root, "caller-owned evidence");
  const kept = evidence(`${root}/run-third`, { ...clean, FVOCI_W5_EVIDENCE_DIR: explicit });
  check(kept === explicit, `explicit evidence dir replaced: ${kept}`);
  const empty = evidence(`${root}/run-empty`, { ...clean, FVOCI_W5_EVIDENCE_DIR: "" });
  check(
    empty === `${root}/run-empty/playwright-output/w5-evidence`,
    `empty evidence dir: ${empty}`,
  );
}

/** The real retention function copies evidence files before the runtime directory is removed. */
async function retention(root: string): Promise<void> {
  const run = join(root, "run-retention");
  const evidence = join(run, "playwright-output", "w5-evidence");
  mkdirSync(evidence, { recursive: true });
  const files: Record<string, string> = {
    "native-proof.json": '{"fixture":true}',
    "zoom200.png": "fixture screenshot bytes",
  };
  for (const [name, value] of Object.entries(files)) writeFileSync(join(evidence, name), value);
  const temporary = join(root, "retained-tmp");
  mkdirSync(temporary);
  const output = join(root, "retention-github-output");
  const env: Env = {
    ...process.env,
    RUN_DIR: run,
    SERVER_LOG: join(run, "missing-server.log"),
    NET_MONITOR_LOG: join(run, "missing-net.log"),
    NET_MARKS_LOG: join(run, "missing-marks.log"),
    GROUP_LABEL: "timer-evidence-fixture",
    TMPDIR: temporary,
    GITHUB_OUTPUT: output,
  };
  check((await retainFailureArtifacts(env)) === 0, "production evidence retention failed");
  rmSync(run, { recursive: true });
  const line = readFileSync(output, "utf8")
    .split("\n")
    .find((entry) => entry.startsWith("failure-artifacts="));
  check(line !== undefined, "retention published no failure-artifacts output");
  const retained = line.slice("failure-artifacts=".length);
  check(!existsSync(run), "runtime directory survived retention");
  for (const [name, value] of Object.entries(files)) {
    const copy = join(retained, "playwright-output", "w5-evidence", name);
    check(readFileSync(copy, "utf8") === value, `retained ${name} differs`);
  }
  // A failed publication/redaction still attempts later copying/merging and
  // never creates a placeholder server log.
  mkdirSync(evidence, { recursive: true });
  for (const [name, value] of Object.entries(files)) writeFileSync(join(evidence, name), value);
  const serverDirectory = join(run, "server.log");
  mkdirSync(serverDirectory);
  const net = join(run, "net-monitor.log");
  writeFileSync(net, "[2026-01-01T00:00:00.000000] probe postgres://u:private-canary@h/db\n");
  const previous = new Set(readdirSync(temporary));
  check(
    (await retainFailureArtifacts({
      ...env,
      GITHUB_OUTPUT: root,
      SERVER_LOG: serverDirectory,
      NET_MONITOR_LOG: net,
    })) === 1,
    "retention errors became success",
  );
  const created = readdirSync(temporary).filter((name) => !previous.has(name));
  check(created.length === 1, "failed retention did not retain its directory");
  const incomplete = join(temporary, created[0] ?? "");
  check(!existsSync(join(incomplete, "server.log")), "failed redaction wrote a placeholder");
  for (const [name, value] of Object.entries(files))
    check(
      readFileSync(join(incomplete, "playwright-output/w5-evidence", name), "utf8") === value,
      "retention stopped after an earlier error",
    );
  check(
    !readFileSync(join(incomplete, "net-events.log"), "utf8").includes("private-canary"),
    "netlink diagnostics disclosed a private canary",
  );
}

const COMMANDS: Record<string, { arity: number; run: (...args: string[]) => unknown }> = {
  "ci-shard": {
    arity: 1,
    run: (root = "") => {
      return ciShardFixture(root);
    },
  },
  "fixture-runtime": {
    arity: 3,
    run: (source = "", destination = "", executable = "") => {
      fixtureRuntime(source, destination, executable);
    },
  },
  "fake-bun": { arity: -1, run: (...args) => fakeBun(args) },
  "record-args": { arity: -1, run: (...args) => process.stdout.write(`${JSON.stringify(args)}\n`) },
  "timer-rows": {
    arity: 2,
    run: (log = "", spec = "") => {
      timerRows(log, spec);
    },
  },
  "dispatch-cases": {
    arity: 2,
    run: (script = "", root = "") => {
      dispatchCases(script, root);
    },
  },
  "evidence-export": {
    arity: 1,
    run: (root = "") => {
      evidenceExport(root);
    },
  },
  retention: {
    arity: 1,
    run: (root = "") => {
      return retention(root);
    },
  },
};

if (import.meta.main) {
  const [name = "", ...args] = process.argv.slice(2);
  const command = Object.hasOwn(COMMANDS, name) ? COMMANDS[name] : undefined;
  if (!command || (command.arity >= 0 && args.length !== command.arity)) {
    process.stderr.write(`usage: shard-fixture.ts {${Object.keys(COMMANDS).join("|")}} ARGS...\n`);
    process.exit(2);
  }
  try {
    await command.run(...args);
  } catch (error) {
    process.stderr.write(
      `shard-fixture ${name}: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exit(1);
  }
}
