// Test-only checks for scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh:
// `bun tools/web-e2e/shard-fixture.ts <command> ...`. Each command runs real
// shell fragments of the web e2e scripts with their runtime stubbed and exits
// 1 with one stderr line on the first broken expectation.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { TIMER_FILTERS } from "./groups.ts";

class FixtureError extends Error {}

function check(condition: boolean, message: string): asserts condition {
  if (!condition) throw new FixtureError(message);
}

const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const same = (left: unknown, right: unknown): boolean =>
  JSON.stringify(left) === JSON.stringify(right);

type Env = Record<string, string | undefined>;

function runBash(
  argv: string[],
  env: Env,
): { status: number | null; stdout: string; stderr: string } {
  const result = Bun.spawnSync(["bash", ...argv], {
    env,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    status: result.exitCode,
    stdout: UTF8.decode(result.stdout),
    stderr: UTF8.decode(result.stderr),
  };
}

function bashOk(argv: string[], env: Env): string {
  const result = runBash(argv, env);
  check(
    result.status === 0,
    `bash ${argv.join(" ")} exited ${String(result.status)}: ${result.stderr}`,
  );
  return result.stdout;
}

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
  const env: Env = { ...process.env, ROOT: root, CARGO_TARGET_DIR: `${root}/target` };
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
    const rows = argvRows(bashOk([script, ...args], env));
    check(
      same(rows, [args]),
      `explicit selection changed: ${JSON.stringify(args)} -> ${JSON.stringify(rows)}`,
    );
  }
  const pendingArgs = ["v050-task-timer.spec.ts", "--workers=1"];
  const pending = argvRows(bashOk([script, ...pendingArgs], { ...env, FVOCI_E2E_PENDING: "1" }));
  check(same(pending, [pendingArgs]), `pending selection changed: ${JSON.stringify(pending)}`);
  const runs = argvRows(bashOk([script, "v050-task-timer.spec.ts"], env));
  check(runs.length === 4, `expected 4 timer runs: ${JSON.stringify(runs)}`);
  runs.forEach((run, index) => {
    const result = runBash([script, "v050-task-timer.spec.ts"], {
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

/** The real export statement: a per-run default beneath playwright-output; explicit paths survive. */
function evidenceExport(statement: string, root: string): void {
  const script = 'RUN_DIR="$1"; source "$2"; printf "%s" "$FVOCI_W5_EVIDENCE_DIR"';
  const clean: Env = { ...process.env };
  delete clean.FVOCI_W5_EVIDENCE_DIR;
  const evidence = (run: string, env: Env): string =>
    bashOk(["-c", script, "fixture", run, statement], env);
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
function retention(fn: string, root: string): void {
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
  bashOk(
    ["-eu", "-c", 'source "$1"; retain_failure_artifacts; rm -rf "$RUN_DIR"', "fixture", fn],
    env,
  );
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
}

const COMMANDS: Record<string, { arity: number; run: (...args: string[]) => void }> = {
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
    arity: 2,
    run: (statement = "", root = "") => {
      evidenceExport(statement, root);
    },
  },
  retention: {
    arity: 2,
    run: (fn = "", root = "") => {
      retention(fn, root);
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
    command.run(...args);
  } catch (error) {
    process.stderr.write(
      `shard-fixture ${name}: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exit(1);
  }
}
