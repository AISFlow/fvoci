// The run-web-e2e.sh selected footer with real kernel UID/GID 1000, sudo and
// setpriv, and the Bun runner CLI. A root preparation process with primary
// GID 1001 models a distinct CI preparation owner without creating an account.
// footer.fixture.ts replaces only identity, the browser copy and lane drivers.
import { spawnSync } from "bun";
import { afterAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { gid, root, sha, uid } from "./io.ts";

const SHA = "a".repeat(40),
  TREE = "b".repeat(40),
  DOCKER_GID = 986;
// A 0755 copy of this Bun under /tmp, so the 1000 actor never needs the
// preparation owner's home directory to start the runner.
const bunDirectory = mkdtempSync(join(tmpdir(), "fvoci-footer-bun-"));
chmodSync(bunDirectory, 0o755);
const bun = join(bunDirectory, "bun");
copyFileSync(process.execPath, bun);
chmodSync(bun, 0o755);
afterAll(() => {
  rmSync(bunDirectory, { recursive: true, force: true });
});

function sudo(...args: string[]): void {
  const result = spawnSync(["sudo", "-n", ...args], { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) throw new Error("sudo fixture step failed: " + String(args[0]));
}
function owned(path: string): { uid: number; gid: number; mode: number } | null {
  const result = spawnSync(["sudo", "-n", "stat", "-L", "-c", "%u %g %a", path], {
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) return null;
  const [owner, group, mode] = result.stdout.toString().trim().split(" ");
  return { uid: Number(owner), gid: Number(group), mode: parseInt(mode ?? "", 8) };
}
function optional(path: string): Record<string, unknown> | null {
  return existsSync(path)
    ? (JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>)
    : null;
}

interface Facts {
  diagnostics: ReturnType<typeof owned>;
  diagnosticModes: Record<string, number>;
  output: ReturnType<typeof owned>;
  sqlite: ReturnType<typeof owned>;
  header: ReturnType<typeof owned>;
  marker: Record<string, unknown> | null;
  stage: Record<string, unknown> | null;
  close: Record<string, unknown> | null;
  safeOwnership: Record<string, unknown> | null;
  safeLauncher: Record<string, unknown> | null;
  privatePublished: boolean;
  safePublished: boolean;
}
function footerCase(
  fault = "",
  exit = 0,
  pending = 0,
  preparationGroup = 1001,
): { status: number; stderr: string; facts: Facts } {
  expect(spawnSync(["sudo", "-n", "true"]).exitCode).toBe(0);
  const temp = mkdtempSync(join(tmpdir(), "fvoci-permission-fixture-"));
  try {
    const repo = join(temp, "repo"),
      scripts = join(repo, "scripts"),
      tools = join(repo, "tools/selected-backend-ci");
    mkdirSync(scripts, { recursive: true });
    mkdirSync(tools, { recursive: true });
    for (const name of readdirSync(import.meta.dir))
      if (name.endsWith(".ts") && !name.endsWith(".test.ts"))
        copyFileSync(join(import.meta.dir, name), join(tools, name));
    copyFileSync(
      join(root, "scripts/run-selected-backend-e2e.ts"),
      join(scripts, "selected-cli.ts"),
    );
    const entry = join(scripts, "run-selected-backend-e2e.ts");
    writeFileSync(
      entry,
      'import { footerEntry } from "../tools/selected-backend-ci/footer.fixture.ts";\n' +
        'import { parseCLI } from "./selected-cli.ts";\n' +
        "footerEntry(parseCLI);\n",
    );
    const output = join(temp, "fvoci-selected-current");
    mkdirSync(output, { mode: 0o700 });
    const sqlite = join(temp, "fvoci-sqlite"),
      lib = join(sqlite, "lib");
    mkdirSync(lib, { recursive: true });
    const header = join(repo, "header");
    writeFileSync(header, "qualified read-only fixture");
    const native = join(lib, "libsqlite3.a");
    writeFileSync(native, "NOT native; owned fixture");
    mkdirSync(join(repo, "browser"));
    const chrome = join(repo, "browser/chromium");
    writeFileSync(chrome, "#!/bin/sh\nexit 0\n");
    chmodSync(chrome, 0o755);
    const fake = join(repo, "bin");
    mkdirSync(fake);
    writeFileSync(
      join(fake, "stat"),
      '#!/bin/sh\nif [ "$1" = -c ] && [ "$2" = %g ] && [ "$3" = /var/run/docker.sock ]; then echo ' +
        String(DOCKER_GID) +
        '; else exec /usr/bin/stat "$@"; fi\n',
    );
    chmodSync(join(fake, "stat"), 0o755);
    const before = {
      head: SHA,
      tree: TREE,
      status: "",
      tracked: { "scripts/run-selected-backend-e2e.ts": sha(entry) },
      external: { [header]: sha(header), [native]: sha(native) },
      untracked: {},
      fixture_bun: bun,
      fixture_chrome: chrome,
      fixture_exit: exit,
      fixture_incomplete: fault === "incomplete",
      fixture_fault: fault,
    };
    for (const name of ["before.json", "after.json"]) {
      writeFileSync(join(output, name), JSON.stringify(before));
      chmodSync(join(output, name), 0o600);
    }
    writeFileSync(join(output, "bundle.json"), JSON.stringify({ binaries: { [chrome]: {} } }));
    chmodSync(join(output, "bundle.json"), 0o600);
    const source = readFileSync(join(root, "scripts/run-web-e2e.sh"), "utf8"),
      start = source.indexOf("selected_status=0\n");
    expect(source.slice(0, start)).toContain('\nSELECTED_PHASE="whole"\n');
    const script = join(temp, "footer.sh");
    writeFileSync(
      script,
      "set -euo pipefail\npending_status=" + String(pending) + "\n" + source.slice(start),
    );
    const destination = join(temp, "fvoci-selected-diagnostics");
    if (fault === "destination-symlink") sudo("ln", "-s", output, destination);
    else if (fault.startsWith("destination-")) {
      mkdirSync(destination, { mode: 0o700 });
      if (fault === "destination-mode") chmodSync(destination, 0o755);
    }
    sudo("chown", "-h", "-R", "0:" + String(preparationGroup), temp);
    if (fault === "destination-foreign") sudo("chown", "1001:1001", destination);
    sudo("chmod", "750", temp, repo);
    if (fault === "unreadable") sudo("chmod", "600", header);
    if (fault === "foreign") sudo("chown", "1001:1001", native);
    if (fault === "symlink") sudo("ln", "-s", header, join(sqlite, "foreign-link"));
    const environment: Record<string, string> = {
      PATH: fake + ":" + bunDirectory + ":/bin:/usr/bin",
      ROOT: repo,
      RUNNER_TEMP: temp,
      // Footer extraction omits the wrapper's real argument-parser default.
      SELECTED_PHASE: "whole",
      SELECTED_BACKENDS: "true",
      FVOCI_SELECTED_CI_OUTPUT: output,
      FVOCI_SELECTED_CI_SQLITE_PARENT: sqlite,
      SQLITE3_LIB_DIR: lib,
      CI: "true",
      GITHUB_ACTIONS: "true",
      GITHUB_JOB: "collaboration-flow",
      GITHUB_OUTPUT: join(temp, "github-output"),
      GITHUB_SHA: SHA,
      GITHUB_REPOSITORY: "fixture/owned",
      GITHUB_RUN_ID: "1",
      GITHUB_RUN_ATTEMPT: "1",
    };
    const result = spawnSync(
      [
        "sudo",
        "-n",
        "setpriv",
        "--reuid=0",
        "--regid=" + String(preparationGroup),
        "--clear-groups",
        "env",
        "-i",
        ...Object.entries(environment).map(([key, value]) => key + "=" + value),
        "/bin/bash",
        script,
      ],
      { stdout: "pipe", stderr: "pipe" },
    );
    // Ownership facts as the footer left them, then return the owned fixture
    // to this test's actor and read the receipts.
    const diagnosticModes: Record<string, number> = {};
    const listed = spawnSync(
      ["sudo", "-n", "find", destination, "-maxdepth", "1", "-type", "f", "-printf", "%f %m\\n"],
      { stdout: "pipe", stderr: "pipe" },
    );
    if (listed.exitCode === 0)
      for (const line of listed.stdout.toString().split("\n").filter(Boolean)) {
        const [name, mode] = line.split(" ");
        diagnosticModes[name ?? ""] = parseInt(mode ?? "", 8);
      }
    const ownership = {
      diagnostics: owned(destination),
      output: owned(output),
      sqlite: owned(sqlite),
      header: owned(header),
    };
    sudo("chown", "-h", "-R", String(uid()) + ":" + String(gid()), temp);
    const published = existsSync(join(temp, "github-output"))
      ? readFileSync(join(temp, "github-output"), "utf8")
      : "";
    const facts: Facts = {
      ...ownership,
      diagnosticModes,
      marker: optional(join(output, "fixture-marker.json")),
      stage: optional(join(output, "runtime-access-stage.json")),
      close: optional(join(output, "runtime-close-stage.json")),
      safeOwnership: fault.startsWith("destination-")
        ? null
        : optional(join(destination, "ownership-stage.json")),
      safeLauncher: fault.startsWith("destination-")
        ? null
        : optional(join(destination, "launcher-stage.json")),
      privatePublished: published.includes("selected-private-diagnostics<<"),
      safePublished: published.includes("selected-safe-diagnostics="),
    };
    expect(JSON.stringify(facts.stage)).not.toContain(temp);
    return { status: result.exitCode, stderr: result.stderr.toString(), facts };
  } finally {
    sudo("chown", "-h", "-R", String(uid()) + ":" + String(gid()), temp);
    rmSync(temp, { recursive: true, force: true });
  }
}

const transferred = { uid: 0, gid: 1001, mode: 0o700 },
  retained = { uid: 1000, gid: 1000, mode: 0o700 };

// sudo and setpriv account switching is Linux only; elsewhere these are NOTRUN
// (reported as skipped), never a pass. CI runs them on its Linux runners.
const linux = process.platform === "linux";
describe.skipIf(!linux).serial("selected footer with the Bun runner under real setpriv", () => {
  test("fixed footer: real access, then private owner round trip", () => {
    const { status, stderr, facts } = footerCase();
    expect(status, stderr).toBe(0);
    expect(facts.safeLauncher?.config_list_exit).toBeNull();
    expect(facts.marker).toEqual({ uid: 1000, gid: 1000, groups: [DOCKER_GID, 1001] });
    expect(facts.stage?.groups).toEqual([DOCKER_GID, 1001]);
    expect((facts.stage?.preflight as { missing: number }).missing).toBe(0);
    expect(facts.stage?.required_group_path_count as number).toBeGreaterThan(0);
    expect(facts.output).toEqual(transferred);
    expect([facts.sqlite?.uid, facts.sqlite?.gid]).toEqual([0, 1001]);
  });
  test("a privileged preparation group is refused before any transfer", () => {
    // GID 27 is sudo on Ubuntu runners; the actor must never receive it.
    const { status, facts } = footerCase("", 0, 0, 27);
    expect(status).not.toBe(0);
    expect(facts.marker).toBeNull();
    expect(facts.stage).toBeNull();
    expect(facts.output).toEqual({ uid: 0, gid: 27, mode: 0o700 });
  });
  test("failed child keeps its first status after settlement and round trip", () => {
    const { status, stderr, facts } = footerCase("", 7);
    expect(status, stderr).toBe(7);
    expect(facts.marker).not.toBeNull();
    expect(facts.output).toEqual(transferred);
  });
  test("waited launcher without resource proof keeps private ownership", () => {
    const { status, stderr, facts } = footerCase("incomplete", 7);
    expect(status, stderr).toBe(7);
    expect(stderr).toContain("resource retirement proof incomplete");
    expect(facts.marker).not.toBeNull();
    expect(facts.output).toEqual(retained);
  });
  test("exact closed receipts admit the return", () => {
    const { status, stderr, facts } = footerCase("closed");
    expect(status, stderr).toBe(0);
    expect(facts.close?.closed_current_runs).toEqual([
      { lane: "install", flow: "on" },
      { lane: "postgres", flow: "on" },
      { lane: "sqlite", flow: "on" },
      { lane: "postgres", flow: "off" },
      { lane: "sqlite", flow: "off" },
    ]);
    expect(facts.output).toEqual(transferred);
  });
  for (const fault of [
    "wrong-source",
    "missing-process",
    "live",
    "wrong-flow",
    "dropped-off",
    "duplicate-root",
    "foreign-owner",
  ])
    test("closure refuses " + fault, () => {
      const { status, facts } = footerCase(fault);
      expect(status).not.toBe(0);
      expect(facts.close).toBeNull();
      expect(facts.output).toEqual(retained);
    });
  test("a declared group cannot override a private unreadable input", () => {
    const { status, facts } = footerCase("unreadable");
    expect(status).not.toBe(0);
    expect((facts.stage?.preflight as { missing: number }).missing).toBe(1);
    expect(facts.marker).toBeNull();
    expect(facts.header?.mode).toBe(0o600);
    expect(facts.output).toEqual(transferred);
  });
  for (const fault of ["foreign", "symlink"])
    test(fault + " native input refused before any transfer", () => {
      const { status, facts } = footerCase(fault);
      expect(status).not.toBe(0);
      expect(facts.marker).toBeNull();
      expect(facts.stage).toBeNull();
      expect(facts.output).toEqual(transferred);
    });
  for (const fault of ["missing-port", "invalid-port", "missing-pid", "unsafe-canary"])
    test("invalid closure " + fault + " keeps the driver and launcher error", () => {
      const { status, stderr, facts } = footerCase(fault, 7);
      expect(status, stderr).toBe(7);
      expect(facts.close).toBeNull();
      expect(facts.output).toEqual(retained);
      expect(facts.diagnostics).toEqual(transferred);
      expect(facts.privatePublished).toBe(false);
      expect(facts.safePublished).toBe(true);
      expect(Object.values(facts.diagnosticModes).every((mode) => mode === 0o600)).toBe(true);
      const summary = facts.safeOwnership as {
          ownership_return_qualified: boolean;
          phase: string;
          lanes: Record<string, unknown>[];
        },
        lane = summary.lanes.at(-1) as Record<string, unknown>;
      expect(summary.ownership_return_qualified).toBe(false);
      expect(summary.phase).toBe("sqlite");
      expect(lane.launcher_observed_driver_exit).toBe(7);
      expect(lane.receipt_final_exit).toBe(7);
      expect(lane.receipt_sha256 as string).toHaveLength(64);
      expect(facts.safeLauncher).toEqual({
        actual_launcher_exit: 7,
        ownership_return_exit: 1,
        selected_final_exit: 7,
        pending_exit: 0,
        config_list_exit: null,
      });
      expect(JSON.stringify(summary)).not.toContain("PRIVATE_CANARY");
      const key =
        fault === "missing-pid"
          ? "recorded_process_identities_retired"
          : "owned_loopback_port_closed";
      expect(
        lane[fault === "invalid-port" ? "invalid_required_fields" : "missing_required_fields"],
      ).toContain(key);
      expect((lane.closure_facts as Record<string, unknown>)[key]).toBeNull();
    });
  test("absent port proof cannot convert a zero launcher to success", () => {
    const { status, stderr, facts } = footerCase("missing-port");
    expect(status, stderr).toBe(1);
    expect(facts.safeLauncher?.actual_launcher_exit).toBe(0);
    expect(facts.safeOwnership?.ownership_return_qualified).toBe(false);
    expect(facts.privatePublished).toBe(false);
  });
  test("qualified return publishes the allowlist; partial refusal only the safe summary", () => {
    const closed = footerCase("closed", 7);
    expect(closed.status, closed.stderr).toBe(7);
    expect(closed.facts.safeOwnership?.ownership_return_qualified).toBe(true);
    expect(closed.facts.privatePublished).toBe(true);
    for (const fault of ["incomplete", "wrong-source", "missing-process", "live"]) {
      const { status, stderr, facts } = footerCase(fault, 7);
      expect(status, stderr).toBe(7);
      expect(facts.safeOwnership?.ownership_return_qualified).toBe(false);
      expect(facts.privatePublished).toBe(false);
      expect(facts.safeLauncher?.actual_launcher_exit).toBe(7);
    }
  });
  test("pending first error is kept with observed selected and closure errors", () => {
    const { status, stderr, facts } = footerCase("missing-port", 7, 13);
    expect(status, stderr).toBe(13);
    expect(facts.safeLauncher?.actual_launcher_exit).toBe(7);
    expect(facts.safeLauncher?.selected_final_exit).toBe(7);
    expect(facts.safeLauncher?.pending_exit).toBe(13);
    expect(facts.safeOwnership?.ownership_return_qualified).toBe(false);
  });
  for (const fault of [
    "destination-occupied",
    "destination-symlink",
    "destination-foreign",
    "destination-mode",
  ])
    test("diagnostics " + fault + " refused before transfer", () => {
      const { status, facts } = footerCase(fault);
      expect(status).not.toBe(0);
      expect(facts.marker).toBeNull();
      expect(facts.safeLauncher).toBeNull();
      expect(facts.safePublished).toBe(false);
      expect(facts.privatePublished).toBe(false);
      expect(facts.output).toEqual(transferred);
    });
});

describe.skipIf(!linux).serial("diagnostic upload access is decided by owner, not path", () => {
  test("private diagnostics owner controls nonroot upload access", () => {
    const parent = mkdtempSync(join(tmpdir(), "fvoci-upload-permission-")),
      output = join(parent, "output");
    mkdirSync(output, { mode: 0o700 });
    writeFileSync(join(output, "safe-stage.json"), "{}");
    try {
      sudo("chmod", "755", parent);
      sudo("chown", "-R", "1000:1000", output);
      const list = () =>
        spawnSync(
          ["sudo", "-n", "setpriv", "--reuid=1001", "--regid=1001", "--clear-groups", "ls", output],
          { stdout: "pipe", stderr: "pipe" },
        );
      expect(list().exitCode).not.toBe(0);
      sudo("chown", "-R", "1001:1001", output);
      const fixed = list();
      expect(fixed.exitCode).toBe(0);
      expect(fixed.stdout.toString()).toContain("safe-stage.json");
      expect(owned(output)?.mode).toBe(0o700);
    } finally {
      sudo("chown", "-h", "-R", String(uid()) + ":" + String(gid()), parent);
      rmSync(parent, { recursive: true, force: true });
    }
  });
  test("nonroot uploader reads only its owned safe prefix", () => {
    const parent = mkdtempSync(join(tmpdir(), "fvoci-safe-upload-fixture-")),
      safe = join(parent, "safe"),
      privateRuntime = join(parent, "private");
    mkdirSync(safe, { mode: 0o700 });
    mkdirSync(privateRuntime, { mode: 0o700 });
    writeFileSync(
      join(safe, "ownership-stage.json"),
      JSON.stringify({ ownership_return_qualified: false, lanes: [] }),
      { mode: 0o600 },
    );
    writeFileSync(join(privateRuntime, "receipt.json"), "PRIVATE_CANARY_SECRET");
    try {
      sudo("chmod", "755", parent);
      sudo("chown", "-R", "1001:1001", safe);
      sudo("chown", "-R", "1000:1000", privateRuntime);
      const as1001 = (...command: string[]) =>
        spawnSync(
          ["sudo", "-n", "setpriv", "--reuid=1001", "--regid=1001", "--clear-groups", ...command],
          { stdout: "pipe", stderr: "pipe" },
        );
      const receipt = as1001("cat", join(safe, "ownership-stage.json"));
      expect(receipt.exitCode).toBe(0);
      expect(JSON.parse(receipt.stdout.toString())).toEqual({
        ownership_return_qualified: false,
        lanes: [],
      });
      const denied = as1001("ls", privateRuntime);
      expect(denied.exitCode).not.toBe(0);
      expect(denied.stdout.toString()).not.toContain("PRIVATE_CANARY");
      expect(owned(privateRuntime)?.uid).toBe(1000);
      expect(owned(safe)).toEqual({ uid: 1001, gid: 1001, mode: 0o700 });
    } finally {
      sudo("chown", "-h", "-R", String(uid()) + ":" + String(gid()), parent);
      rmSync(parent, { recursive: true, force: true });
    }
  });
});
