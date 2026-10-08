import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../../../..");
const source = readFileSync(resolve(root, "scripts/run-web-e2e.sh"), "utf8");
const parser = source.slice(
  source.indexOf("# CI matrix runs"),
  source.indexOf("\nverify_committed_api() {"),
);
const allocated = { CI: "true", GITHUB_ACTIONS: "true", GITHUB_JOB: "collaboration-flow" };
const consume = ["--ci-use-committed-api", "--ci-consume-selected", "--ci-consume-part"];

function parse(args: string[], env: Record<string, string> = allocated) {
  return Bun.spawnSync(
    [
      "bash",
      "-c",
      `${parser}\nprintf '%s|%s|%s' "$SELECTED_PHASE" "$SELECTED_PART" "\${FVOCI_COLLAB_FLOW_PART:-}"`,
      "collab-part-fixture",
      ...args,
    ],
    { env: { PATH: process.env.PATH, ...env }, stdout: "pipe", stderr: "pipe" },
  );
}

for (const part of ["pending", "restart"]) {
  test(`${part} is an explicit allocated consumer part`, () => {
    const result = parse([...consume, part]);
    expect(result.exitCode).toBe(0);
    expect(result.stdout.toString()).toBe(`consume|${part}|${part}`);
    expect(result.stderr.toString()).toBe("");
  });
}

test("missing, duplicate, unknown, mixed and unallocated parts fail before preparation", () => {
  for (const args of [
    consume,
    [...consume, "unknown"],
    [...consume, "pending", "--ci-consume-part", "restart"],
    ["--ci-consume-selected", "--ci-consume-part", "pending"],
    ["--ci-use-committed-api", "--ci-prepare-selected", "--ci-consume-part", "pending"],
    [...consume, "pending", "--ci-shard", "0"],
    [...consume, "restart", "--ci-consume-browser"],
    [...consume, "restart", "--grep", "subset"],
    ["--ci-use-committed-api", "--ci-consume-selected"],
  ]) {
    expect(parse(args).exitCode).not.toBe(0);
  }
  for (const env of [
    {},
    { ...allocated, CI: "false" },
    { ...allocated, GITHUB_JOB: "collaboration-build" },
    { ...allocated, FVOCI_COLLAB_FLOW_PART: "restart" },
  ]) {
    expect(parse([...consume, "pending"], env).exitCode).not.toBe(0);
  }
});

function dispatch(part: string, pendingExit = 0, restartExit = 0) {
  const start = source.indexOf("pending_status=0\n");
  const body = source.indexOf("  # Mandatory companion", start);
  const finish = source.lastIndexOf('if [[ "$pending_status" -ne 0 ]]');
  // Execute the actual selection and status footer; stub only the existing
  // group/native boundaries. This exercises no DB, browser or ownership change.
  const script = `set -euo pipefail
SELECTED_PART="$PART"
SELECTED_BACKENDS=true
ROOT=/unused
SPEC_ARGS=()
bash() { printf 'pending\n'; return "$PENDING_EXIT"; }
${source.slice(start, body)}
  printf 'restart\n'
  selected_status="$RESTART_EXIT"
fi
${source.slice(finish)}`;
  return Bun.spawnSync(["bash", "-c", script], {
    env: {
      PATH: process.env.PATH,
      PART: part,
      PENDING_EXIT: String(pendingExit),
      RESTART_EXIT: String(restartExit),
    },
    stdout: "pipe",
    stderr: "pipe",
  });
}

test("pending and restart execute disjoint boundaries with the same union as whole", () => {
  const pending = dispatch("pending");
  const restart = dispatch("restart");
  const whole = dispatch("whole");
  for (const result of [pending, restart, whole]) expect(result.exitCode).toBe(0);
  expect(pending.stdout.toString()).toBe("pending\n");
  expect(restart.stdout.toString()).toBe("restart\n");
  expect(pending.stdout.toString() + restart.stdout.toString()).toBe(whole.stdout.toString());
});

test("each part propagates failure; whole still attempts restart after pending failure", () => {
  expect(dispatch("pending", 7).exitCode).toBe(7);
  expect(dispatch("restart", 0, 9).exitCode).toBe(9);
  const whole = dispatch("whole", 7, 9);
  expect(whole.exitCode).toBe(7);
  expect(whole.stdout.toString()).toBe("pending\nrestart\n");
});

test("both parts retain admission/consumption and separate diagnostic paths", () => {
  expect(source).toContain("run_stage selected-handoff-admit python3");
  expect(source).toContain("run_stage selected-handoff-consume python3");
  expect(source).toContain('safe_diagnostics="$diagnostics_temp/fvoci-selected-diagnostics"');
  expect(source).toContain('diagnostics_temp="$RUNNER_TEMP/fvoci-collab-$SELECTED_PART"');
  expect(source.match(/--preserve-env=[^\n]*FVOCI_COLLAB_FLOW_PART/g)).toHaveLength(3);
});

test("part diagnostic paths cannot collide or overwrite an occupied capture", () => {
  const directory = mkdtempSync(join(tmpdir(), "collab-part-diagnostics-"));
  const start = source.indexOf('  diagnostics_temp="$RUNNER_TEMP"');
  const end = source.indexOf("\nPY_DIAGNOSTICS", start) + "\nPY_DIAGNOSTICS".length;
  // Execute the existing reviewed diagnostic block verbatim, including its
  // ownership/mode checks. No Python implementation is added to this test.
  const block = source.slice(start, end);
  try {
    for (const part of ["pending", "restart"]) {
      const parent = join(directory, `fvoci-collab-${part}`);
      mkdirSync(parent, { mode: 0o700 });
      const output = join(directory, `${part}.output`);
      const run = () =>
        Bun.spawnSync(["bash", "-c", `set -euo pipefail\n${block}`], {
          env: {
            PATH: process.env.PATH,
            RUNNER_TEMP: directory,
            TMPDIR: parent,
            SELECTED_PART: part,
            GITHUB_OUTPUT: output,
          },
          stdout: "pipe",
          stderr: "pipe",
        });
      expect(run().exitCode).toBe(0);
      expect(statSync(join(parent, "fvoci-selected-diagnostics")).mode & 0o777).toBe(0o700);
      const receipt = readFileSync(output, "utf8");
      expect(receipt).toBe(`selected-safe-diagnostics=${parent}/fvoci-selected-diagnostics\n`);
      expect(run().exitCode).not.toBe(0);
      expect(readFileSync(output, "utf8")).toBe(receipt);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
