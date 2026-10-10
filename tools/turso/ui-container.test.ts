import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { sha } from "../selected-backend-ci/io.ts";
import { UiError, type Record_ } from "./ui-common.ts";
import {
  DAEMON_CAPS,
  FIXED_LAUNCHER,
  FIXTURE_BUDGET,
  GO_MARKER,
  QUALIFIED_CANONICAL_IMAGE,
  STAT_BODY_MAX,
  STAT_READ_BOUND,
  acceptStatPayload,
  admitPublishedConfig,
  bindPausedShell,
  capsuleText,
  captureOwnedDaemon,
  cleanupOwned,
  containerArgv,
  creationIdentity,
  docker,
  finishedOneShot,
  goOpenFlags,
  lease,
  localFixture,
  localServerStart,
  nativeFailureCode,
  openReferencedGrant,
  parseNspid,
  parseStatusFields,
  preparePrivateFifos,
  proveNormalDaemon,
  publishContainer,
  qualifyDaemonExit,
  readShellProof,
  releaseFailedPublish,
  reportCleanup,
  runningDaemonSample,
  shellQuote,
  statOpenFlags,
  writeGoOnce,
} from "./ui-container.ts";
import { Captured, exitedChild, failureOf, fakeScope, must } from "./ui-fakes.ts";
import type { Child } from "./ui-processes.ts";
import { SERVER_BUDGET } from "./ui-start-diagnostic.ts";
import { constants } from "node:fs";

let directory: string;
const realClient = docker.client;
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "fvoci-ui-container-"));
  delete process.env.FVOCI_SELECTED_EXECUTION_MODE;
});
afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
  docker.client = realClient;
  delete process.env.FVOCI_SELECTED_EXECUTION_MODE;
});
const argvArgs: [string, string, string, string, string[], string[]] = [
  "fvoci-tui-aa",
  "/host/launcher.sh",
  "/host/native-env.sh",
  "/fvoci-current/bin/fvoci-migrate",
  [] as string[],
  ["--start"],
];
const ids = () => [1000, 1000, 1000, 1000];
const paused = (clientPid = 77) =>
  bindPausedShell(
    "1 (sh) S 0 0 0 0 -1 0 0 0 0 0 0 0 0 0 0 20 0 1 999",
    { State: { Pid: 50, Running: true, OOMKilled: false } },
    { pid: 50, startTicks: "999", comm: "sh" },
    [50, 1],
    { ...DAEMON_CAPS },
    clientPid,
    { uid: ids(), gid: ids() },
  );

describe("container shape", () => {
  test("the shell proof comes only from the grant and the config env stays clean", () => {
    const environment = {
      FVOCI_LIBSQL_URL: "libsql://invented.invalid",
      FVOCI_LIBSQL_AUTH_TOKEN: "invented'token",
    };
    expect(capsuleText(environment)).toContain(
      "FVOCI_LIBSQL_AUTH_TOKEN=" + shellQuote("invented'token"),
    );
    expect(shellQuote("invented'token")).toBe("'invented'\"'\"'token'");
    expect(shellQuote("")).toBe("''");
    expect(shellQuote("plain/path-1.0")).toBe("plain/path-1.0");
    const argv = containerArgv(...argvArgs);
    for (const flag of ["--cpus", "--cpu-quota", "--cpu-period", "--cpuset-cpus"])
      expect(argv).not.toContain(flag);
    expect(argv[argv.indexOf("--memory") + 1]).toBe("12884901888");
    expect(argv[argv.indexOf("--memory-swap") + 1]).toBe("12884901888");
    expect(argv[argv.indexOf("--pids-limit") + 1]).toBe("128");
    expect(DAEMON_CAPS["cpu.max"]).toBe("max 100000");
    admitPublishedConfig(
      argv,
      ["PATH=/usr/bin:/bin", "FVOCI_COLLAB_ENGINE=/opt/fvoci/bin/collab-engine"],
      ["invented'token", "libsql://invented.invalid"],
    );
    expect(argv).not.toContain("--env-file");
    expect(argv.join("\n")).not.toContain("invented'token");
    expect(() => {
      admitPublishedConfig(argv, ["FVOCI_LIBSQL_AUTH_TOKEN=invented"], ["invented"]);
    }).toThrow("UI_DOCKER_ENV_SECRET_REFUSED");
    expect(() => readShellProof({ imageId: QUALIFIED_CANONICAL_IMAGE })).toThrow(
      "UI_CANONICAL_SHELL_UNAVAILABLE",
    );
    const proof = join(directory, "shell-proof.json");
    writeFileSync(
      proof,
      JSON.stringify({
        Config: {
          Image: QUALIFIED_CANONICAL_IMAGE,
          Cmd: ["/bin/sh", "/acceptance/run.sh"],
          Env: ["PATH=/usr/bin:/bin"],
        },
      }),
    );
    expect(
      readShellProof({
        imageId: QUALIFIED_CANONICAL_IMAGE,
        shellProof: { path: proof, sha256: sha(proof) },
      }),
    ).toBe("/bin/sh");
    expect(() =>
      readShellProof({
        imageId: QUALIFIED_CANONICAL_IMAGE,
        shellProof: { path: proof, sha256: "0".repeat(64) },
      }),
    ).toThrow("UI_CANONICAL_SHELL_UNAVAILABLE");
  });

  test("the capsule refuses LOCPATH, unlisted secrets and multi-line values", () => {
    const base = { FVOCI_LIBSQL_URL: "u", FVOCI_LIBSQL_AUTH_TOKEN: "t" };
    expect(() => capsuleText({ ...base, LOCPATH: "/x" })).toThrow("UI_LOCPATH_REFUSED");
    expect(() => capsuleText({ ...base, GITHUB_TOKEN: "x" })).toThrow(
      "UI_DOCKER_ENV_SECRET_REFUSED",
    );
    expect(() => capsuleText({ ...base, RUST_LOG: "a\nb" })).toThrow(
      "UI_DOCKER_ENV_SECRET_REFUSED",
    );
    expect(() => capsuleText({ FVOCI_LIBSQL_URL: "u" })).toThrow("UI_DOCKER_ENV_SECRET_REFUSED");
  });

  test("the local profile removes only the hosted memory flags", () => {
    const grant = { canonicalRuntime: { networkAuthorized: true } };
    process.env.FVOCI_SELECTED_EXECUTION_MODE = "orca-local";
    expect(() => containerArgv(...argvArgs)).toThrow("UI_LOCAL_ALLOCATION_REFUSED");
    const argv = containerArgv(...argvArgs, grant);
    expect(argv).not.toContain("--memory");
    expect(argv).not.toContain("--memory-swap");
    expect(argv[argv.indexOf("--pids-limit") + 1]).toBe("128");
    for (const required of ["--read-only", "no-new-privileges", "--cap-drop", "1000:1000"])
      expect(argv).toContain(required);
    for (const flag of ["--cpus", "--cpu-quota", "--cpu-period", "--cpuset-cpus"])
      expect(argv).not.toContain(flag);
    const proc = { pid: 50, startTicks: "100", comm: "fvoci-server", exeInspection: "UNAVAILABLE" };
    const running = { State: { Pid: 50, Running: true, OOMKilled: false } };
    const localCaps = { ...DAEMON_CAPS, "memory.max": "max", "memory.swap.max": "max" };
    const sample = runningDaemonSample(
      running,
      localCaps,
      proc,
      1,
      77,
      "/fvoci-current/bin/fvoci-server",
      true,
      grant,
    );
    expect(sample.caps).toEqual(localCaps);
    expect(() =>
      runningDaemonSample(
        running,
        { ...DAEMON_CAPS },
        proc,
        1,
        77,
        "/fvoci-current/bin/fvoci-server",
        true,
        grant,
      ),
    ).toThrow("UI_DAEMON_CAP_REFUSED");
    process.env.FVOCI_SELECTED_EXECUTION_MODE = "other";
    expect(() => containerArgv(...argvArgs, grant)).toThrow("UI_EXECUTION_MODE_REFUSED");
  });

  test("a one-shot without a cgroup sample stays blocked", () => {
    const created = {
      State: { Pid: 0, Running: false },
      Config: {
        Image: QUALIFIED_CANONICAL_IMAGE,
        Entrypoint: ["/bin/sh"],
        Cmd: ["/fvoci-current/launcher.sh"],
        Env: ["PATH=/usr/bin:/bin"],
        User: "1000:1000",
      },
      HostConfig: {
        ReadonlyRootfs: true,
        CapDrop: ["ALL"],
        Privileged: false,
        Memory: 12884901888,
      },
    };
    const argv = [
      "docker",
      "create",
      "--entrypoint",
      "/bin/sh",
      QUALIFIED_CANONICAL_IMAGE,
      "/fvoci-current/launcher.sh",
    ];
    const creation = creationIdentity(created, argv, ["invented-token"], "fixture");
    expect(creation.qualification).toBe("BLOCKED");
    expect(creation.cgroupCaps).toBe("not-observed");
    expect(creation).not.toHaveProperty("cpuUnlimited");
    const unlimited = {
      ...created,
      HostConfig: { ...created.HostConfig, CpuQuota: 0, NanoCpus: 0, CpusetCpus: "" },
    };
    const admitted = creationIdentity(unlimited, argv, ["invented-token"], "fixture");
    expect(admitted.cgroupCaps).toBe("not-observed");
    expect(admitted).not.toHaveProperty("cpuUnlimited");
    for (const restricted of [
      { CpuQuota: 200000 },
      { NanoCpus: 2000000000 },
      { CpusetCpus: "0-1" },
      { CpuQuota: true },
    ])
      expect(() =>
        creationIdentity(
          { ...created, HostConfig: { ...created.HostConfig, ...restricted } },
          argv,
          ["invented-token"],
          "fixture",
        ),
      ).toThrow("UI_DAEMON_CAP_REFUSED");
    expect(creation).not.toHaveProperty("caps");
    const done = finishedOneShot(creation as unknown as Record_, 0);
    expect(done).toEqual({
      productExit: 0,
      liveDaemon: "unsupported-before-execution",
      qualification: "BLOCKED",
    });
    expect(JSON.stringify(done)).not.toContain("accepted");
    const proc = { pid: 50, startTicks: "100", comm: "fvoci-server", exeInspection: "UNAVAILABLE" };
    const running = { State: { Pid: 50, Running: true, OOMKilled: false } };
    expect(() =>
      runningDaemonSample(
        running,
        created.HostConfig,
        proc,
        1,
        77,
        "/fvoci-current/bin/fvoci-server",
        true,
      ),
    ).toThrow("UI_DAEMON_CAP_REFUSED");
    const sample = runningDaemonSample(
      running,
      { ...DAEMON_CAPS },
      proc,
      1,
      77,
      "/fvoci-current/bin/fvoci-server",
      true,
    );
    expect(sample.qualification).toBe("daemon-observed");
    expect(() =>
      runningDaemonSample(
        running,
        { ...DAEMON_CAPS, "cpu.max": "200000 100000" },
        proc,
        1,
        77,
        "/fvoci-current/bin/fvoci-server",
        true,
      ),
    ).toThrow("UI_DAEMON_CAP_REFUSED");
    expect(sample.startTicks).toBe("100");
    expect(() =>
      runningDaemonSample(
        running,
        { ...DAEMON_CAPS },
        proc,
        1,
        77,
        "/fvoci-current/bin/fvoci-server",
        false,
      ),
    ).toThrow("UI_DAEMON_WAIT_MISSING");
  });
});

describe("local grant and publish", () => {
  test("repeated local fixtures of one mode each get their own allocation directory", async () => {
    // A consumer calls owner, observe and baseline more than once per run.
    const proof = join(directory, "shell-proof.json");
    writeFileSync(
      proof,
      JSON.stringify({ Config: { Image: QUALIFIED_CANONICAL_IMAGE, Cmd: ["/bin/sh"], Env: [] } }),
    );
    const binary = join(directory, "fvoci-e2e-fixture");
    writeFileSync(binary, "");
    process.env.FVOCI_SELECTED_EXECUTION_MODE = "orca-local";
    lease.load = () =>
      ({
        canonicalRuntime: {
          imageId: QUALIFIED_CANONICAL_IMAGE,
          networkAuthorized: true,
          shellProof: { path: proof, sha256: sha(proof) },
        },
      }) as never;
    const created: string[][] = [];
    docker.client = (_scope, args) => {
      created.push([...args]);
      return Promise.reject(new UiError("UI_DOCKER_CLIENT_FAILED"));
    };
    const root = join(directory, "root");
    mkdirSync(root, 0o700);
    const { scope } = fakeScope({ root: () => root });
    const environment = {
      FVOCI_LIBSQL_URL: "libsql://invented.invalid",
      FVOCI_LIBSQL_AUTH_TOKEN: "invented",
      FVOCI_STORAGE_DIR: "/work/storage",
    };
    const manifest = { binaries: { "fvoci-e2e-fixture": { path: binary } } } as never;
    try {
      for (let i = 0; i < 2; i++)
        expect(await failureOf(localFixture(scope, manifest, "owner", environment))).toBe(
          "UI_DOCKER_CLIENT_FAILED",
        );
      expect(created.map((args) => args[1])).toEqual(["create", "create"]);
    } finally {
      lease.load = undefined;
    }
  });

  test("opening the grant keeps the local guard and does not borrow run", () => {
    const calls: string[] = [];
    const load = (mode: string) => {
      calls.push(mode);
      return { canonicalRuntime: {} } as never;
    };
    expect(() => openReferencedGrant("fixture", load)).toThrow("UI_EXECUTION_MODE_REFUSED");
    expect(calls).toEqual([]);
    process.env.FVOCI_SELECTED_EXECUTION_MODE = "orca-local";
    expect(() => openReferencedGrant("fixture", load)).toThrow("UI_LOCAL_NETWORK_NOT_GRANTED");
    expect(calls).toEqual(["fixture"]);
    // Until the admission loader accepts the turso-ui consumer, the local mode fails closed.
    expect(() => openReferencedGrant("fixture", undefined)).toThrow("UI_LOCAL_ALLOCATION_REFUSED");
  });

  test("publish without a grant does not call docker and a mocked create hides the token", async () => {
    const token = "invented'token";
    const calls: string[][] = [];
    docker.client = () => {
      throw new Error("docker");
    };
    process.env.FVOCI_SELECTED_EXECUTION_MODE = "github-ci";
    expect(
      await failureOf(
        publishContainer(
          null,
          "/unused",
          {
            FVOCI_LIBSQL_URL: "libsql://invented.invalid",
            FVOCI_LIBSQL_AUTH_TOKEN: token,
            FVOCI_STORAGE_DIR: "/work/storage",
          },
          { binaries: { "fvoci-e2e-fixture": { path: "/host/bin/fvoci-e2e-fixture" } } } as never,
          "fvoci-e2e-fixture",
          ["baseline"],
          "fixture",
        ),
      ),
    ).toContain("UI_EXECUTION_MODE_REFUSED");
    const created = {
      State: { Pid: 0, Running: false, OOMKilled: false },
      Config: {
        Image: QUALIFIED_CANONICAL_IMAGE,
        Entrypoint: ["/bin/sh"],
        Cmd: ["/fvoci-current/launcher.sh"],
        Env: ["PATH=/usr/bin:/bin"],
        User: "1000:1000",
      },
      HostConfig: {
        ReadonlyRootfs: true,
        CapDrop: ["ALL"],
        Privileged: false,
        CpuQuota: 0,
        NanoCpus: 0,
        CpusetCpus: "",
      },
    };
    docker.client = (_scope, args) => {
      calls.push([...args]);
      return Promise.resolve(
        new TextEncoder().encode(
          args[1] === "create" ? "a".repeat(64) + "\n" : JSON.stringify([created]),
        ),
      );
    };
    process.env.FVOCI_SELECTED_EXECUTION_MODE = "orca-local";
    const binary = join(directory, "fvoci-e2e-fixture");
    writeFileSync(binary, "");
    const grant = {
      source: "a".repeat(40),
      tree: "b".repeat(40),
      runId: "run_a1",
      dispatchId: "ctx_b2",
      canonicalRuntime: { imageId: QUALIFIED_CANONICAL_IMAGE, networkAuthorized: true },
    };
    const { scope } = fakeScope();
    await publishContainer(
      scope,
      directory,
      {
        FVOCI_LIBSQL_URL: "libsql://invented.invalid",
        FVOCI_LIBSQL_AUTH_TOKEN: token,
        FVOCI_STORAGE_DIR: "/work/storage",
      },
      { binaries: { "fvoci-e2e-fixture": { path: binary } } } as never,
      "fvoci-e2e-fixture",
      ["baseline"],
      "fixture",
      () => grant,
    );
    const blob = calls.flat().join("\n");
    expect(blob).not.toContain(token);
    expect(JSON.stringify(created.Config.Env)).not.toContain(token);
    expect(blob + "\n").toContain("type=bind,src=/work/storage,dst=/work/storage\n");
    expect(blob).not.toContain("--env-file");
    expect(existsSync(join(directory, "creation-identity.private.json"))).toBe(true);
  });

  test("a failed publish removes its container and FIFOs", async () => {
    const calls: string[][] = [];
    docker.client = (_scope, args) => {
      calls.push([...args]);
      return Promise.resolve(new Uint8Array());
    };
    const cause = new UiError("UI_DAEMON_OBSERVATION_UNSUPPORTED");
    preparePrivateFifos(directory);
    const { scope } = fakeScope();
    expect(await releaseFailedPublish(scope, directory, "ab".repeat(32), cause)).toBe(cause);
    expect(calls).toEqual([["docker", "rm", "ab".repeat(32)]]);
    expect(existsSync(join(directory, "stat.ready"))).toBe(false);
    expect(existsSync(join(directory, "exec.go"))).toBe(false);
  });
});

describe("paused shell release", () => {
  test("the launcher is bounded and releases only on exact GO", () => {
    const text = FIXED_LAUNCHER;
    expect(text.indexOf('. "$1"')).toBeLessThan(text.indexOf("/proc/$$/stat"));
    expect(text.indexOf("/fvoci-private/stat.ready")).toBeLessThan(
      text.indexOf("/fvoci-private/exec.go"),
    );
    expect(text).toContain('[ "${#fvoci_stat}" -le 511 ]');
    expect(text).toContain('[ "$fvoci_go" = GO ]');
    expect(text).not.toContain("/proc/self");
    expect(new TextDecoder().decode(GO_MARKER)).toBe("GO\n");
    expect([STAT_BODY_MAX, STAT_READ_BOUND, FIXTURE_BUDGET, SERVER_BUDGET]).toEqual([
      511, 512, 120, 10,
    ]);
    const encode = (s: string) => new TextEncoder().encode(s);
    acceptStatPayload(encode("a".repeat(511) + "\n"));
    for (const refused of ["", "\n", "a".repeat(512) + "\n", "GO\nextra\n"])
      expect(() => acceptStatPayload(encode(refused))).toThrow("UI_DAEMON_PID_REFUSED");
    expect(statOpenFlags() & (constants.O_WRONLY | constants.O_RDWR)).toBe(0);
    expect(goOpenFlags() & constants.O_RDWR).toBe(constants.O_RDWR);
    const writes: [number, string][] = [];
    writeGoOnce(4, (fd, data) => (writes.push([fd, new TextDecoder().decode(data)]), 3));
    expect(writes).toEqual([[4, "GO\n"]]);
    expect(() => {
      writeGoOnce(4, () => 2);
    }).toThrow("UI_DAEMON_OBSERVATION_UNSUPPORTED");
  });

  test("the owned capture index is per client and the sample keeps caps and ids", () => {
    const first = exitedChild(null, "", 1),
      second = exitedChild(null, "", 2);
    const recorded: (number | null)[] = [];
    const scope = {
      allocations: [
        { process: first, label: "a", closed: false, forced: false },
        { process: second, label: "b", closed: false, forced: false },
      ],
      capture: (_row: unknown, _label: string, allocation: number | null = null) => (
        recorded.push(allocation),
        ""
      ),
    };
    expect(
      captureOwnedDaemon(scope, first, { pid: 50, parentPid: 1, startTicks: "9", state: "S" }),
    ).toBe(0);
    expect(
      captureOwnedDaemon(scope, second, { pid: 60, parentPid: 1, startTicks: "8", state: "S" }),
    ).toBe(1);
    expect(recorded).toEqual([0, 1]);
    const bound = paused();
    expect(bound.caps).toEqual({ ...DAEMON_CAPS });
    expect(bound.uid).toEqual(ids());
    expect(bound.gid).toEqual(ids());
    expect(bound.maintenance.pid).toBe(1);
    expect(bound.retirement.pid).toBe(50);
    expect(() =>
      bindPausedShell(
        "1 (sh) S 0 0 0 0 -1 0 0 0 0 0 0 0 0 0 0 20 0 1 999",
        { State: { Pid: 50, Running: true, OOMKilled: false } },
        { pid: 50, startTicks: "999", comm: "sh" },
        [50, 1],
        { ...DAEMON_CAPS },
        77,
        { uid: ids(), gid: [0, 0, 0, 0] },
      ),
    ).toThrow("UI_DAEMON_PID_REFUSED");
    const status = "NSpid:\t50\t1\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n";
    expect(parseStatusFields(status, "Uid:")).toEqual(ids());
    expect(parseNspid(status)).toEqual([50, 1]);
    // Structural: the normal proof is "retired", never the blocked one-shot record,
    // and the local start owns its cleanup instead of a daemon sample.
    expect(proveNormalDaemon.toString()).toContain('"retired"');
    expect(proveNormalDaemon.toString()).not.toContain("BLOCKED");
    expect(localFixture.toString()).toContain("daemon-completion.private.json");
    expect(localFixture.toString()).not.toContain("one-shot-blocked.private.json");
    expect(localServerStart.toString()).toContain("cleanup");
    expect(localServerStart.toString()).not.toContain("runningDaemonSample");
  });

  test("the normal proof needs client exit and retirement and is not blocked", async () => {
    const client = exitedChild(0, "", 77);
    const sample = { ...paused(), allocation: 0 };
    let clientExit = 0;
    let retired = true;
    const { scope, calls } = fakeScope({
      spawn: () => client,
      finish: () => clientExit,
      retired: () => retired,
    });
    scope.spawn(["docker"], "fixture", { env: {} });
    const other = exitedChild(null, "", 99);
    scope.allocations.push({ process: other, label: "other", closed: false, forced: false });
    scope.capture({ pid: 50, parentPid: 1, startTicks: "999", state: "S" }, "container-init", 0);
    scope.capture({ pid: 77, parentPid: 1, startTicks: "3", state: "S" }, "client", 0);
    const state = { Pid: 0, ExitCode: 0, OOMKilled: false };
    const result = await proveNormalDaemon(scope, client, sample, state, false);
    expect(result.qualification).toBe("retired");
    expect(result.clientExit).toBe(0);
    expect(result.productExit).toBe(0);
    expect(result.caps).toEqual({ ...DAEMON_CAPS });
    expect(must(scope.allocations[1]).closed).toBe(false);
    clientExit = 9;
    must(scope.allocations[0]).closed = false;
    expect(await failureOf(proveNormalDaemon(scope, client, sample, state, false))).toContain(
      "UI_PROCESS_CLOSURE_FAILED",
    );
    retired = false;
    expect(await failureOf(proveNormalDaemon(scope, client, sample, state, false))).toContain(
      "UI_PROCESS_CLOSURE_FAILED",
    );
    const finishes = calls.finish.length;
    expect(await failureOf(proveNormalDaemon(scope, client, sample, state, true))).toContain(
      "UI_DAEMON_OBSERVATION_UNSUPPORTED",
    );
    expect(calls.finish).toHaveLength(finishes);
    expect(qualifyDaemonExit(state, true)).toEqual([false, 0]);
    expect(qualifyDaemonExit({ Pid: 0, ExitCode: 137, OOMKilled: false }, false)).toEqual([
      false,
      137,
    ]);
  });

  test("cleanup removes only the owned container and keeps the original", async () => {
    let running = true;
    const client = {
      ...exitedChild(null, "", 77),
      get exitCode() {
        return running ? null : 0;
      },
    } as unknown as Child;
    const calls: string[][] = [];
    docker.client = (_scope, args) => {
      calls.push([...args]);
      return Promise.resolve(new Uint8Array());
    };
    const { scope } = fakeScope({
      spawn: () => client,
      finish: (target) => {
        expect(target).toBe(client);
        running = false;
        return 0;
      },
    });
    scope.spawn(["docker"], "server", { env: {} });
    scope.allocations.push({
      process: exitedChild(null),
      label: "other",
      closed: false,
      forced: false,
    });
    scope.capture({ pid: 50, parentPid: 1, startTicks: "1", state: "S" }, "container-init", 0);
    const errors = await cleanupOwned(scope, "a".repeat(64), client, directory, []);
    expect(errors).toEqual([]);
    expect(calls.map((args) => args[1])).toEqual(["stop", "rm"]);
    expect(must(scope.allocations[1]).closed).toBe(false);
    const output = new Captured();
    const cause = new UiError("UI_SERVER_START_FAILED");
    expect(reportCleanup({ io: { ...scope.io, output } }, cause, ["UI_DOCKER_CLIENT_FAILED"])).toBe(
      cause,
    );
    expect(output.all).toContain("UI_SERVER_START_FAILED");
  });

  test("a nonzero native exit keeps the allowlisted cause", () => {
    expect(nativeFailureCode({ originalFailure: "TURSO_UI_ROWS" }, 3)).toBe("TURSO_UI_ROWS");
    expect(nativeFailureCode({}, 2)).toBe("UI_NATIVE_FIXTURE_FAILED");
    expect(nativeFailureCode({ originalFailure: "TURSO_UI_ROWS" }, 0)).toBeNull();
    expect(() => nativeFailureCode({ originalFailure: "not-a-code" }, 4)).toThrow(
      "UI_NATIVE_FAILURE_CODE_REFUSED",
    );
    const fixtureSource = localFixture.toString();
    expect(fixtureSource.indexOf("nativeFailureCode")).toBeLessThan(
      fixtureSource.indexOf("proveNormalDaemon"),
    );
    const output = new Captured();
    const cause = new UiError("TURSO_UI_ROWS");
    expect(
      reportCleanup({ io: { output, write: () => {}, root: () => "" } }, cause, [
        "UI_PROCESS_CLOSURE_FAILED",
      ]),
    ).toBe(cause);
    expect(output.all).toContain("TURSO_UI_ROWS");
    expect(output.all).toContain("UI_PROCESS_CLOSURE_FAILED");
    const serverSource = localServerStart.toString();
    expect(serverSource.indexOf("try {")).toBeLessThan(serverSource.indexOf("openSync(logpath"));
    const publishSource = publishContainer.toString();
    expect(publishSource.indexOf("try {")).toBeLessThan(publishSource.indexOf("docker.client"));
    expect(publishSource).toContain("releaseFailedPublish");
  });
});
