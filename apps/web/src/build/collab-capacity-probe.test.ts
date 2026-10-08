import assert from "node:assert/strict";
import {
  accessSync,
  constants,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import test from "node:test";
import { spawnSync } from "bun";

const root = resolve(import.meta.dir, "../../../..");
const probe = join(root, "scripts/collab-capacity-probe.sh");
const driver = join(root, "scripts/start-test-postgres.sh");
// Bash's non-POSIX literal function syntax accepts this word. Quoted/dynamic
// function names do not; reject any path needing shell escaping before spawning.
assert.match(driver, /^\/[a-zA-Z0-9_./-]+$/);
for (const utility of ["bash", "dirname", "mkdir", "date", "tee", "awk"]) {
  accessSync(`/usr/bin/${utility}`, constants.X_OK);
}
const evidence = join(root, "target/harness-capacity-count-2");
mkdirSync(evidence, { recursive: true });
const fixtures = mkdtempSync(join(evidence, "fixtures-"));

function quote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

const childCheck = `
set -euo pipefail
[[ -d "$PATH" && ! -e "$PATH/cargo" && ! -e "$PATH/docker" && ! -e "$PATH/bash" ]] || exit 90
for mock in cargo bash docker mark require_mock; do
  require_mock "$mock"
done
readonly -f cargo bash docker mark require_mock
[[ $(cargo fixture-ready) == fixture-cargo ]] || exit 90
mark child:ready
`;

const setup = `
set -euo pipefail
function mark { printf '%s\\n' "$1" >> "$FIXTURE_EVENTS"; }
function require_mock {
  declare -F "$1" >/dev/null || exit 90
  [[ $(type -t "$1") == function ]] || exit 90
}
function docker { mark forbidden:docker; exit 90; }
function cargo {
  if [[ "$*" == fixture-ready ]]; then
    printf '%s' fixture-cargo
    return 0
  fi
  printf '%s\\0' cargo "$@" >> "$FIXTURE_ARGV"
  printf '\\0' >> "$FIXTURE_ARGV"
  case "$*" in
    'build --locked --release --manifest-path crates/collab-engine/Cargo.toml --features worker --bin collab-engine')
      return "$FIXTURE_HELPER_EXIT" ;;
    'build --locked --release --features db-tests --test collab_capacity_probe')
      return "$FIXTURE_BUILD_EXIT" ;;
    'test --release --features db-tests --test collab_capacity_probe -- --nocapture')
      printf '%s\\0' "$COLLAB_PROBE_ROOMS" "$COLLAB_PROBE_PEERS" "$COLLAB_PROBE_DURATION_SECS" \\
        "$COLLAB_PROBE_OPEN_CONCURRENCY" "$FVOCI_COLLAB_MAX_ROOMS" "$FVOCI_TEST_PG_MAX_CONNECTIONS" \\
        "$RUST_LOG" "$FVOCI_COLLAB_ENGINE" > "$FIXTURE_PROBE_ENV"
      printf '%s' "$FIXTURE_STDERR" >&2
      printf '%s' "$FIXTURE_STDOUT"
      return "$FIXTURE_TEST_EXIT" ;;
    *) mark forbidden:cargo-argv; exit 90 ;;
  esac
}
function bash {
  [[ $# == 2 && $1 == -c ]] || exit 90
  mark mock:bash
  /usr/bin/bash --noprofile --norc -c "$FIXTURE_CHILD_CHECK"$'\\n'"$2"
}
function ${driver} {
  [[ $# == 3 && $1 == bash && $2 == -c ]] || exit 90
  require_mock cargo
  require_mock bash
  mark mock:driver
  "$@"
}
function dirname { /usr/bin/dirname "$@"; }
function mkdir { /usr/bin/mkdir "$@"; }
function date { /usr/bin/date "$@"; }
function tee { /usr/bin/tee "$@"; }
function awk { /usr/bin/awk "$@"; }
[[ -d "$PATH" && ! -e "$PATH/cargo" && ! -e "$PATH/docker" && ! -e "$PATH/bash" ]] || exit 90
for mock in ${quote(driver)} cargo bash docker dirname mkdir date tee awk mark require_mock; do
  require_mock "$mock"
done
readonly -f ${quote(driver)} cargo bash docker dirname mkdir date tee awk mark require_mock
export -f cargo bash docker mark require_mock
[[ $(cargo fixture-ready) == fixture-cargo ]] || exit 90
/usr/bin/bash --noprofile --norc -c "$FIXTURE_CHILD_CHECK"
mark parent:ready
`;

interface Fixture {
  stdout: string;
  stderr?: string;
  testExit?: number;
  helperExit?: number;
  buildExit?: number;
}

function runFixture(fixture: Fixture, mode: "preflight" | "probe" = "probe") {
  const directory = mkdtempSync(join(fixtures, "case-"));
  const emptyPath = join(directory, "empty-path");
  mkdirSync(emptyPath);
  const events = join(directory, "events.log");
  const argv = join(directory, "argv.log");
  writeFileSync(events, "");
  writeFileSync(argv, "");
  const result = spawnSync(
    [
      "/usr/bin/bash",
      "--noprofile",
      "--norc",
      "-c",
      `${setup}\n${mode === "probe" ? 'source "$0"' : "exit 0"}`,
      probe,
    ],
    {
      cwd: root,
      env: {
        PATH: emptyPath,
        FVOCI_EVIDENCE_DIR: directory,
        FIXTURE_CHILD_CHECK: childCheck,
        FIXTURE_EVENTS: events,
        FIXTURE_ARGV: argv,
        FIXTURE_PROBE_ENV: join(directory, "probe-env.log"),
        FIXTURE_STDOUT: fixture.stdout,
        FIXTURE_STDERR: fixture.stderr ?? "fixture cargo stderr\n",
        FIXTURE_TEST_EXIT: String(fixture.testExit ?? 0),
        FIXTURE_HELPER_EXIT: String(fixture.helperExit ?? 0),
        FIXTURE_BUILD_EXIT: String(fixture.buildExit ?? 0),
      },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  writeFileSync(join(directory, "stdout.log"), result.stdout);
  writeFileSync(join(directory, "stderr.log"), result.stderr);
  return {
    exit: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
    events: readFileSync(events, "utf8").trim().split("\n"),
    argv: readFileSync(argv, "utf8")
      .split("\0\0")
      .filter(Boolean)
      .map((record) => record.split("\0")),
    directory,
  };
}

// Mandatory preparation, separate from guard tests. A failure throws before any
// test can source the product; every later source repeats both readiness checks.
const readiness = runFixture({ stdout: "" }, "preflight");
assert.equal(readiness.exit, 0, "fixture preflight failed; product entrypoint blocked");
assert.deepEqual(readiness.events, ["child:ready", "parent:ready"]);
assert.deepEqual(readiness.argv, []);
console.log("Fixture preflight PASS; product entrypoint not called.");

const target =
  "     Running tests/collab_capacity_probe.rs (target/release/deps/collab_capacity_probe-fixture)\n";
const other = "     Running tests/unrelated.rs (target/release/deps/unrelated-fixture)\n";
const passed =
  "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 180.00s\n";
const zero =
  "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";

const cases: { name: string; log: string; exit: number }[] = [
  { name: "one passed target", log: target + "running 1 test\n" + passed, exit: 0 },
  { name: "zero passed", log: target + zero, exit: 1 },
  { name: "ignored only", log: target + zero.replace("0 ignored", "1 ignored"), exit: 1 },
  {
    name: "filtered only",
    log: target + zero.replace("0 filtered out", "1 filtered out"),
    exit: 1,
  },
  {
    name: "failed summary with zero cargo exit",
    log: target + passed.replace("ok.", "FAILED.").replace("0 failed", "1 failed"),
    exit: 1,
  },
  { name: "multiple tests", log: target + passed.replace("1 passed", "2 passed"), exit: 1 },
  {
    name: "one passed plus ignored",
    log: target + passed.replace("0 ignored", "1 ignored"),
    exit: 1,
  },
  { name: "no target summary", log: target + "probe passed successfully\n", exit: 1 },
  { name: "unattributed summary", log: passed, exit: 1 },
  { name: "unrelated successful target", log: other + passed, exit: 1 },
  {
    name: "unrelated success cannot hide target zero",
    log: target + zero + other + passed,
    exit: 1,
  },
  { name: "unrelated zero before matched target", log: other + zero + target + passed, exit: 0 },
  { name: "unrelated zero after matched target", log: target + passed + other + zero, exit: 0 },
  { name: "last target summary wins", log: target + passed + zero, exit: 1 },
  { name: "later target run without summary", log: target + passed + target, exit: 1 },
  { name: "arbitrary success words", log: target + `diagnostic: ${passed}`, exit: 1 },
];

for (const fixture of cases) {
  await test(fixture.name, () => {
    const result = runFixture({ stdout: fixture.log });
    assert.equal(result.exit, fixture.exit);
    assert.deepEqual(result.events, [
      "child:ready",
      "parent:ready",
      "mock:driver",
      "mock:bash",
      "child:ready",
    ]);
    assert.equal(result.stdout.includes("collab capacity probe passed;"), fixture.exit === 0);
    if (fixture.exit !== 0)
      assert.match(result.stderr, /must report exactly 1 passed, 0 failed, 0 ignored/);
    const logs = readdirSync(result.directory).filter((name) =>
      name.startsWith("collab-capacity-probe-"),
    );
    assert.equal(logs.length, 1);
    assert.equal(
      readFileSync(join(result.directory, logs[0] ?? "missing"), "utf8"),
      "fixture cargo stderr\n" + fixture.log,
    );
    assert.ok(result.stdout.includes("fixture cargo stderr\n" + fixture.log));
  });
}

await test("source entrypoint preserves cargo argv and probe defaults in its child", () => {
  const result = runFixture({ stdout: passed, stderr: target });
  assert.equal(result.exit, 0);
  assert.deepEqual(result.argv, [
    [
      "cargo",
      "build",
      "--locked",
      "--release",
      "--manifest-path",
      "crates/collab-engine/Cargo.toml",
      "--features",
      "worker",
      "--bin",
      "collab-engine",
    ],
    [
      "cargo",
      "build",
      "--locked",
      "--release",
      "--features",
      "db-tests",
      "--test",
      "collab_capacity_probe",
    ],
    [
      "cargo",
      "test",
      "--release",
      "--features",
      "db-tests",
      "--test",
      "collab_capacity_probe",
      "--",
      "--nocapture",
    ],
  ]);
  assert.deepEqual(readFileSync(join(result.directory, "probe-env.log"), "utf8").split("\0"), [
    "64",
    "2",
    "180",
    "8",
    "64",
    "150",
    "collab.stage=info",
    join(root, "crates/collab-engine/target/release/collab-engine"),
    "",
  ]);
});

await test("cargo nonzero propagates even with a successful summary", () => {
  const result = runFixture({ stdout: target + passed, testExit: 23 });
  assert.equal(result.exit, 23);
  assert.match(result.stderr, /failed \(exit 23\)/);
  assert.ok(!result.stdout.includes("collab capacity probe passed;"));
});

for (const build of ["helperExit", "buildExit"] as const) {
  await test(`${build} stops before the driver`, () => {
    const result = runFixture({ stdout: target + passed, [build]: 17 });
    assert.equal(result.exit, 17);
    assert.deepEqual(result.events, ["child:ready", "parent:ready"]);
    assert.equal(result.argv.length, build === "helperExit" ? 1 : 2);
  });
}
