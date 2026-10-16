// run-web-e2e.ts checks on temporary checkouts and directories; the shard
// fixture (scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh) drives the
// same commands through the real wrapper.
import { afterEach, describe, expect, test } from "bun:test";
import {
  chmodSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  CheckError,
  CI_WEB_IMAGE,
  containerImage,
  containerRequest,
  containerFailureArtifacts,
  webRequest,
  containerShard,
  containerArchitecture,
  containerImageSource,
  qualifyContainerDaemon,
  commandProcess,
  qualifyCohortHostProgram,
  qualifyImageSource,
  sqliteEnvironment,
  committedApiJobs,
  createSafeDiagnostics,
  launcherReceipt,
  normalizePath,
  planSpecs,
  qualifyCommittedApi,
  requirePathWithin,
  resolvePath,
  writeLauncherReceipt,
  type CommittedApiRequest,
} from "./run-web-e2e.ts";
import { nativeArchitecture, nativeLinux } from "../selected-backend-ci/types.ts";

const CLI = join(import.meta.dir, "run-web-e2e.ts");
const scratch: string[] = [];

function tempDir(): string {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "fvoci-run-web-e2e.")));
  scratch.push(directory);
  return directory;
}

afterEach(() => {
  for (const directory of scratch.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function refused(body: () => unknown, message: string | RegExp): void {
  expect(body).toThrow(CheckError);
  expect(body).toThrow(message);
}

describe("committedApiJobs", () => {
  const lanes = [
    "collaboration-install-on",
    "collaboration-postgres-on",
    "collaboration-sqlite-on",
    "collaboration-postgres-off",
    "collaboration-sqlite-off",
  ];
  const cases: [CommittedApiRequest, string | undefined, string[]][] = [
    [
      { shard: "0", selected: false, browserPhase: "consume" },
      "workspace-browser-shard",
      ["workspace-browser-shard"],
    ],
    [
      { shard: "", selected: false, browserPhase: "prepare" },
      "workspace-browser-build",
      ["workspace-browser-build"],
    ],
    [
      { shard: "", selected: true, browserPhase: "" },
      "collaboration-flow",
      ["collaboration-flow", ...lanes],
    ],
    [
      { shard: "", selected: true, browserPhase: "" },
      "collaboration-build",
      ["collaboration-build"],
    ],
    [
      { shard: "3", selected: false, browserPhase: "" },
      "collaboration-build",
      ["workspace-browser-shard"],
    ],
  ];
  for (const [request, job, allowed] of cases) {
    test(`${JSON.stringify(request)} in ${String(job)}`, () => {
      expect([...committedApiJobs(request, job)].sort()).toEqual([...allowed].sort());
    });
  }

  test("refuses a request with no shard, companion or producer phase", () => {
    refused(
      () =>
        committedApiJobs(
          { shard: "", selected: false, browserPhase: "consume" },
          "collaboration-flow",
        ),
      "requires a browser shard or selected companion",
    );
  });
});

function git(root: string, ...args: string[]): string {
  const result = Bun.spawnSync(["git", "-C", root, ...args], { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) throw new Error(`git ${args.join(" ")}: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
}

function committedCheckout(): { root: string; sha: string } {
  const root = tempDir();
  git(root, "init", "-q");
  mkdirSync(join(root, "apps/web/src/generated"), { recursive: true });
  writeFileSync(join(root, "apps/web/openapi.json"), '{"openapi":"3.1.0"}\n');
  writeFileSync(join(root, "apps/web/src/generated/api.ts"), "// committed\n");
  git(root, "add", "apps");
  git(
    root,
    "-c",
    "user.name=Fixture",
    "-c",
    "user.email=fixture@example.invalid",
    "-c",
    "commit.gpgsign=false",
    "commit",
    "-qm",
    "outputs",
  );
  return { root, sha: git(root, "rev-parse", "HEAD") };
}

describe("qualifyCommittedApi", () => {
  const request: CommittedApiRequest = { shard: "0", selected: false, browserPhase: "consume" };
  const ciEnv = (sha: string) => ({
    CI: "true",
    GITHUB_ACTIONS: "true",
    GITHUB_JOB: "workspace-browser-shard",
    GITHUB_SHA: sha,
  });

  test("accepts a clean checkout at the tested SHA", () => {
    const { root, sha } = committedCheckout();
    expect(qualifyCommittedApi(root, request, ciEnv(sha))).toBe(sha);
  });

  test("local ordinary browser identity is distinct from GitHub and still checks the exact source", () => {
    const { root, sha } = committedCheckout();
    const local = {
      FVOCI_WEB_CONTAINER_LOCAL: "1",
      FVOCI_WEB_CONTAINER_JOB: "workspace-browser-shard",
      FVOCI_WEB_CONTAINER_IMAGE: CI_WEB_IMAGE,
      FVOCI_WEB_CONTAINER_SDK_SHA256: "b".repeat(64),
      FVOCI_WEB_CONTAINER_SOURCE: sha,
    };
    expect(qualifyCommittedApi(root, request, local)).toBe(sha);
    expect(() =>
      qualifyCommittedApi(root, request, { ...local, GITHUB_ACTIONS: "true" }),
    ).toThrow();
    expect(() =>
      qualifyCommittedApi(root, request, { ...local, FVOCI_WEB_CONTAINER_SOURCE: "0".repeat(40) }),
    ).toThrow();
    expect(() =>
      qualifyCommittedApi(root, request, { ...local, FVOCI_WEB_CONTAINER_IMAGE: "latest" }),
    ).toThrow();
  });

  test("refuses outside CI, in another job, at another SHA or through a symlinked root", () => {
    const { root, sha } = committedCheckout();
    refused(
      () => qualifyCommittedApi(root, request, { ...ciEnv(sha), CI: undefined }),
      "requires GitHub CI",
    );
    refused(
      () => qualifyCommittedApi(root, request, { ...ciEnv(sha), GITHUB_JOB: undefined }),
      "allocated browser job",
    );
    refused(
      () => qualifyCommittedApi(root, request, ciEnv("0".repeat(40))),
      "checkout HEAD differs",
    );
    refused(
      () => qualifyCommittedApi(root, request, ciEnv(sha.toUpperCase())),
      "checkout HEAD differs",
    );
    const link = join(tempDir(), "link");
    symlinkSync(root, link);
    refused(
      () => qualifyCommittedApi(link, request, ciEnv(sha)),
      "wrapper must belong to this checkout",
    );
  });

  test("refuses a symlinked or empty output and a failing git", () => {
    const { root, sha } = committedCheckout();
    const output = join(root, "apps/web/openapi.json");
    const original = readFileSync(output);
    const copy = join(root, "copied.json");
    writeFileSync(copy, original);
    rmSync(output);
    symlinkSync("../../copied.json", output);
    git(root, "update-index", "--assume-unchanged", "apps/web/openapi.json");
    refused(
      () => qualifyCommittedApi(root, request, ciEnv(sha)),
      "must be a physical regular output",
    );
    rmSync(output);
    writeFileSync(output, "");
    refused(
      () => qualifyCommittedApi(root, request, ciEnv(sha)),
      "physical bytes differ from HEAD",
    );
    const failing = () => ({ status: 128, stdout: new Uint8Array() });
    refused(
      () => qualifyCommittedApi(root, request, ciEnv(sha), failing),
      "git rev-parse HEAD failed with exit 128",
    );
  });
});

test("containers admit complete migrated shards, never partial or required host fixtures", () => {
  for (const value of ["1", "2", "4", "6", "7"]) expect(containerShard(value)).toBe(value);
  for (const value of [undefined, "", "0", "3", "5", "-1", "6 --grep=skip", "8"])
    expect(() => containerShard(value)).toThrow(CheckError);
});

test("image source qualification covers copied helper support and catalogs without unrelated tests", () => {
  const { root } = committedCheckout();
  for (const directory of ["xtask/tests", "tests/support"])
    mkdirSync(join(root, directory), { recursive: true });
  for (const file of ["xtask/catalog.tsv", "tests/support/official_sqld.rs"])
    writeFileSync(join(root, file), "original input\n");
  const commit = () => {
    git(root, "add", ".");
    git(
      root,
      "-c",
      "user.name=Fixture",
      "-c",
      "user.email=fixture@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "-qm",
      "image input fixture",
    );
    return git(root, "rev-parse", "HEAD");
  };
  const image = commit();
  writeFileSync(join(root, "xtask/tests/caller.rs"), "unrelated test\n");
  expect(() => {
    qualifyImageSource(root, image, commit());
  }).not.toThrow();
  for (const file of [
    "xtask/catalog.tsv",
    "tests/support/official_sqld.rs",
    "xtask/new-catalog.tsv",
  ]) {
    writeFileSync(join(root, file), "changed copied input\n");
    expect(() => {
      qualifyImageSource(root, image, commit());
    }).toThrow(CheckError);
    if (file === "xtask/new-catalog.tsv") rmSync(join(root, file));
    else writeFileSync(join(root, file), "original input\n");
    commit();
  }
});

test("real commands preserve normal and nonzero exits", () => {
  for (const code of [0, 7]) {
    const source = `import {command} from ${JSON.stringify(CLI)};process.exit(await command([process.execPath,"-e",${JSON.stringify(`process.exit(${String(code)})`)}],process.cwd()));`;
    const child = Bun.spawnSync([process.execPath, "-e", source], {
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(child.exitCode).toBe(code);
  }
});

for (const [signal, status] of [
  ["SIGINT", 130],
  ["SIGTERM", 143],
] as const)
  test(`real ${signal} remains cancellation after child exit0 and stops new work`, async () => {
    const directory = tempDir(),
      bad = join(directory, "new-work"),
      cleanup = join(directory, "cleanup");
    const body = `await new Promise(resolve=>{process.once(${JSON.stringify(signal)},resolve);console.log("READY");});console.log("CHILD_NORMAL_EXIT");process.exit(0);`;
    const write = (file: string) =>
      `require("node:fs").writeFileSync(${JSON.stringify(file)},"observed");`;
    const source = `import {command} from ${JSON.stringify(CLI)};
const first=await command([process.execPath,"-e",${JSON.stringify(body)}],process.cwd());
const next=await command([process.execPath,"-e",${JSON.stringify(write(bad))}],process.cwd());
const clean=await command([process.execPath,"-e",${JSON.stringify(write(cleanup))}],process.cwd(),process.env,{stdout:"inherit",stderr:"inherit"},true);
console.log(JSON.stringify({first,next,clean}));process.exit(first);`;
    const child = Bun.spawn([process.execPath, "-e", source], { stdout: "pipe", stderr: "pipe" });
    try {
      const reader = child.stdout.getReader(),
        decoder = new TextDecoder();
      let output = "";
      while (!output.includes("READY\n")) {
        const chunk = await reader.read();
        if (chunk.done) throw new Error("child exited before ready");
        output += decoder.decode(chunk.value);
      }
      child.kill(signal);
      for (;;) {
        const chunk = await reader.read();
        if (chunk.done) break;
        output += decoder.decode(chunk.value);
      }
      expect(await child.exited).toBe(status);
      expect(output).toContain("CHILD_NORMAL_EXIT");
      expect(output).toContain(JSON.stringify({ first: status, next: status, clean: 0 }));
      expect(() => readFileSync(bad)).toThrow();
      expect(readFileSync(cleanup, "utf8")).toBe("observed");
    } finally {
      if (child.exitCode === null) child.kill("SIGTERM");
      await child.exited;
    }
  });

test("maintained default preserves the full hybrid suite and exact host selections", () => {
  expect(webRequest([], {}).route).toBe("hybrid");
  for (const shard of ["1", "2", "4", "6", "7"])
    expect(webRequest(["--ci-shard", shard], {}).route).toBe("docker");
  for (const shard of ["0", "3", "5"])
    expect(webRequest(["--ci-shard", shard], {}).route).toBe("host");
  const args = ["e2e/v050-task-timer.spec.ts", "--grep", "exact original filter", "--workers=1"];
  expect(webRequest(args, {}).specs).toEqual(args);
  expect(webRequest(args, {}).route).toBe("host");
  expect(webRequest([], { FVOCI_E2E_PENDING: "1" }).route).toBe("host");
  expect(webRequest(["--host"], {}).route).toBe("host");
  expect(webRequest(["--ci-shard", "2"], { GITHUB_ACTIONS: "true" }).route).toBe("host");
  for (const args of [
    ["--ci-shard", "8"],
    ["--ci-shard", "2", "x.spec.ts"],
    ["--host", "--local-image-sdk"],
    ["--ci-shard", "0", "--image-pin", "local"],
    ["--ci-shard-count", "16"],
  ])
    expect(() => webRequest(args, {})).toThrow(CheckError);
});

test("sealed SDK selection distinguishes local engine IDs from registry and CI authority", () => {
  const seal = "b".repeat(64),
    image = "sha256:" + "a".repeat(64);
  expect(containerImage(image, seal, true, {})).toEqual({ image, sdk: seal, local: true });
  expect(containerImage(CI_WEB_IMAGE, seal, false, { CI: "true" }).image).toBe(CI_WEB_IMAGE);
  for (const [pin, sdk, local, env] of [
    [image, seal, false, {}],
    [CI_WEB_IMAGE, seal, true, {}],
    [image, seal, true, { CI: "true" }],
    [image, seal, true, { GITHUB_ACTIONS: "true" }],
    [CI_WEB_IMAGE, "", false, {}],
    ["latest", seal, false, {}],
  ] as const)
    expect(() => containerImage(pin, sdk, local, env)).toThrow(CheckError);
});

test("published defaults bind native SDK seals in both local and actual CI requests", () => {
  const ci = {
    CI: "true",
    GITHUB_ACTIONS: "true",
    GITHUB_JOB: "workspace-browser-container-build",
    GITHUB_SHA: "c".repeat(40),
    GITHUB_RUN_ID: "12",
    GITHUB_RUN_ATTEMPT: "1",
    GITHUB_REPOSITORY: "owner/repo",
  };
  expect(CI_WEB_IMAGE).toBe(
    "ghcr.io/aisflow/fvoci-ci-web@sha256:cb1f5290f97ad5fb156b852ac0d21466e91571063979af4e9838172f41d8e23c",
  );
  for (const [aliases, sdk] of [
    [
      ["x64", "x86_64", "amd64"],
      "dfd2e7b84d19f6e8ca03b80ded497d69606df8685ed6f1a929b6e8ecaba35892",
    ],
    [["arm64", "aarch64"], "171384584c2a218a591e6d42784578c28ab1f64741487090b285006022260cca"],
  ] as const) {
    for (const architecture of aliases) {
      const local = containerRequest(["/checkout", "--ci-shard", "2"], {}, architecture);
      const producer = containerRequest(
        ["/checkout", "--prepare", "--handoff", "/owned/packet"],
        ci,
        architecture,
      );
      const consumer = containerRequest(
        [
          "/checkout",
          "--consume",
          "2",
          "--handoff",
          "/owned/packet",
          "--handoff-sha256",
          "d".repeat(64),
        ],
        { ...ci, GITHUB_JOB: "workspace-browser-shard" },
        architecture,
      );
      expect(local.sdk).toEqual({ image: CI_WEB_IMAGE, sdk, local: false });
      expect(producer.sdk).toEqual(local.sdk);
      expect(consumer.sdk).toEqual(local.sdk);
    }
  }
  for (const args of [
    ["--image-pin", CI_WEB_IMAGE],
    ["--sdk-sha256", "b".repeat(64)],
    ["--local-image-sdk"],
  ])
    expect(() => containerRequest(["/checkout", "--ci-shard", "2", ...args], {})).toThrow(
      CheckError,
    );
  expect(() => containerRequest(["/checkout", "--ci-shard", "2"], {}, "riscv64")).toThrow();
});

test("separate image phases require actual CI allocation and immutable packet admission", () => {
  const pin = "ghcr.io/aisflow/fvoci-ci-web@sha256:" + "a".repeat(64),
    seal = "b".repeat(64);
  const prepare = [
    "/checkout",
    "--prepare",
    "--handoff",
    "/owned/packet",
    "--image-pin",
    pin,
    "--sdk-sha256",
    seal,
  ];
  const ci = {
    CI: "true",
    GITHUB_ACTIONS: "true",
    GITHUB_JOB: "workspace-browser-container-build",
    GITHUB_SHA: "c".repeat(40),
    GITHUB_RUN_ID: "12",
    GITHUB_RUN_ATTEMPT: "1",
    GITHUB_REPOSITORY: "owner/repo",
  };
  expect(containerRequest(prepare, ci).phase).toBe("prepare");
  const consume = [
    "/checkout",
    "--consume",
    "2",
    ...prepare.slice(2),
    "--handoff-sha256",
    "d".repeat(64),
  ];
  expect(
    containerRequest(consume, { ...ci, GITHUB_JOB: "workspace-browser-shard" }).shards,
  ).toEqual(["2"]);
  for (const changed of [
    { GITHUB_JOB: "workspace-browser-build" },
    { GITHUB_SHA: "main" },
    { GITHUB_RUN_ATTEMPT: "0" },
    { CI: "false" },
    { GITHUB_ACTIONS: "false" },
    { GITHUB_REPOSITORY: "foreign" },
  ])
    expect(() => containerRequest(prepare, { ...ci, ...changed })).toThrow(CheckError);
  for (const args of [
    consume.slice(0, -2),
    consume.map((v) => (v === "2" ? "0" : v)),
    [...prepare, "--local-image-sdk"],
    prepare.filter((v) => v !== "--handoff" && v !== "/owned/packet"),
  ])
    expect(() => containerRequest(args, ci)).toThrow();
  expect(() =>
    containerRequest(["/checkout", "--ci-shard", "2", ...prepare.slice(4)], ci),
  ).toThrow();
});

test("container diagnostics expose only the owned mapped failure directory", () => {
  const run = tempDir();
  mkdirSync(join(run, "tmp"));
  const retained = join(run, "tmp/fvoci-collab-e2e-fail.Abc123");
  mkdirSync(retained, { mode: 0o700 });
  expect(
    containerFailureArtifacts(
      "failure-artifacts=/fvoci-run/tmp/fvoci-collab-e2e-fail.Abc123\n",
      run,
    ),
  ).toBe(retained);
  for (const output of [
    "failure-artifacts=/foreign\n",
    "failure-artifacts=/fvoci-run/tmp/../foreign\n",
    "failure-artifacts=/fvoci-run/tmp/fvoci-collab-e2e-fail.Abc123\nfailure-artifacts=/foreign\n",
    "x".repeat(16385),
  ])
    expect(() => containerFailureArtifacts(output, run)).toThrow(CheckError);
});

test("dependency phases refuse credentials and cross targets and preserve fetch failures", async () => {
  const child = Bun.spawn(
    [process.execPath, "--no-env-file", CLI, "browser-phase", "/unused", "inputs", "2"],
    {
      env: {
        ...process.env,
        FVOCI_WEB_CONTAINER_IMAGE: CI_WEB_IMAGE,
        TEST_DATABASE_URL: "PRIVATE_DEPENDENCY_CANARY",
      },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const [code, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
  ]);
  expect(code).toBe(1);
  expect(stdout).toBe("");
  expect(stderr).toContain("credential-free phase");
  expect(stderr).not.toContain("PRIVATE_DEPENDENCY_CANARY");

  const { root, sha } = committedCheckout(),
    bin = tempDir(),
    calls = join(bin, "calls");
  writeFileSync(
    join(bin, "cargo"),
    "#!" +
      process.execPath +
      '\nconst fs=require("node:fs"),args=process.argv.slice(2);fs.appendFileSync(process.env.FIXTURE_CALLS,JSON.stringify({args,cwd:process.cwd(),offline:process.env.CARGO_NET_OFFLINE})+"\\n");process.exit(args.includes("--manifest-path")?17:0);\n',
    { mode: 0o755 },
  );
  const target = nativeArchitecture(process.arch) + "-unknown-linux-gnu";
  for (const phase of ["prepare", "inputs"]) {
    for (const crossTarget of [false, true]) {
      writeFileSync(calls, "");
      const result = Bun.spawn(
        [
          process.execPath,
          "--no-env-file",
          CLI,
          "browser-phase",
          root,
          phase,
          ...(phase === "inputs" ? ["2"] : []),
        ],
        {
          env: {
            ...process.env,
            PATH: bin + ":" + (process.env.PATH ?? ""),
            FIXTURE_CALLS: calls,
            FVOCI_WEB_CONTAINER_LOCAL: "1",
            FVOCI_WEB_CONTAINER_JOB:
              phase === "prepare" ? "workspace-browser-build" : "workspace-browser-shard",
            FVOCI_WEB_CONTAINER_SOURCE: sha,
            FVOCI_WEB_CONTAINER_IMAGE: CI_WEB_IMAGE,
            FVOCI_WEB_CONTAINER_SDK_SHA256: "b".repeat(64),
            CARGO_BUILD_TARGET: crossTarget ? "wasm32-unknown-unknown" : target,
          },
          stdout: "pipe",
          stderr: "pipe",
        },
      );
      const [exit, out, err] = await Promise.all([
        result.exited,
        new Response(result.stdout).text(),
        new Response(result.stderr).text(),
      ]);
      expect(exit, err).toBe(crossTarget ? 1 : 17);
      expect(out).toBe("");
      const recorded = readFileSync(calls, "utf8");
      if (crossTarget) {
        expect(recorded).toBe("");
        expect(err).toContain("native Linux runtime, kernel and Rust target must match");
      } else {
        expect(
          recorded
            .trim()
            .split("\n")
            .map((line) => JSON.parse(line) as unknown),
        ).toEqual([
          { args: ["fetch", "--locked", "--target", target], cwd: root, offline: "false" },
          {
            args: [
              "fetch",
              "--locked",
              "--target",
              target,
              "--manifest-path",
              join(root, "crates/collab-engine/Cargo.toml"),
            ],
            cwd: root,
            offline: "false",
          },
        ]);
        expect(err).toContain("stage=engine-fetch");
        expect(err).not.toContain("stage=dependencies");
      }
    }
  }
});

test("modeled native Linux/Darwin clients, daemon/image identity and ABI closure", () => {
  const pin = "ghcr.io/aisflow/fvoci-ci-web@sha256:" + "a".repeat(64);
  for (const aliases of [
    ["x64", "x86_64", "amd64"],
    ["arm64", "aarch64"],
  ]) {
    const machine = nativeArchitecture(aliases[0] as string);
    for (const alias of aliases) {
      expect(containerArchitecture("linux", alias, machine)).toBe(machine);
      expect(
        containerArchitecture("darwin", alias, machine, machine === "aarch64" ? "1" : "0"),
      ).toBe(machine);
      qualifyContainerDaemon("unix:///owned/docker.sock", "linux " + alias, machine);
      expect(
        containerImageSource(
          "sha256:" +
            "b".repeat(64) +
            " linux " +
            alias +
            " " +
            "c".repeat(40) +
            " " +
            JSON.stringify([pin]),
          { image: pin, local: false },
          machine,
        ),
      ).toBe("c".repeat(40));
      const native = nativeLinux("linux", alias, machine, machine + "-unknown-linux-gnu");
      expect(native.target).toBe(machine + "-unknown-linux-gnu");
      expect(native.abi).toEqual([
        machine === "x86_64" ? "/lib64/ld-linux-x86-64.so.2" : "/lib/ld-linux-aarch64.so.1",
        ...["libc.so.6", "libm.so.6", "libgcc_s.so.1"].map(
          (name) => "/lib/" + machine + "-linux-gnu/" + name,
        ),
      ]);
    }
  }
  for (const args of [
    ["linux", "x64", "aarch64"],
    ["darwin", "x64", "x86_64", "1"],
    ["darwin", "arm64", "aarch64", "0"],
    ["darwin", "arm64", "aarch64"],
    ["win32", "x64", "x86_64"],
    ["linux", "riscv64", "riscv64"],
  ])
    expect(() =>
      containerArchitecture(args[0] as string, args[1] as string, args[2] as string, args[3]),
    ).toThrow();
  for (const args of [
    ["linux", "arm64", "x86_64"],
    ["darwin", "x64", "x86_64"],
    ["linux", "x64", "x86_64", "aarch64-unknown-linux-gnu"],
    ["linux", "arm64", "aarch64", "x86_64-unknown-linux-gnu"],
    ["linux", "x64", "x86_64", ""],
  ])
    expect(() =>
      nativeLinux(args[0] as string, args[1] as string, args[2] as string, args[3]),
    ).toThrow();
  for (const endpoint of [
    "tcp://PRIVATE_ENDPOINT_CANARY",
    "ssh://PRIVATE_ENDPOINT_CANARY",
    "unix://foreign/socket",
    "unix:///socket\n",
  ])
    expect(() => {
      qualifyContainerDaemon(endpoint, "linux amd64", "x86_64");
    }).toThrow();
  expect(commandProcess(["docker", "pull", pin], "darwin")).toEqual({
    argv: ["docker", "pull", pin],
    group: false,
  });
  expect(commandProcess(["docker", "pull", pin], "linux").group).toBe(true);
  expect(() => commandProcess(["bash", "host-only"], "darwin")).toThrow(/host-only/);
});

for (const mode of [
  "normal",
  "published-default",
  ...(nativeArchitecture(process.arch) === "x86_64" ? ["amd64", "x64"] : ["arm64"]),
  "inputs-fail",
  "daemon-mismatch",
  "image-mismatch",
  "foreign-digest",
  "remote-endpoint",
])
  test("isolated CI Docker topology: " + mode, async () => {
    const { root, sha } = committedCheckout(),
      bin = tempDir(),
      state = join(bin, "state.json"),
      calls = join(bin, "calls");
    const packet = join(bin, "packet"),
      output = join(bin, "github-output");
    mkdirSync(packet, { mode: 0o700 });
    writeFileSync(join(packet, "handoff.json"), "{}");
    writeFileSync(join(packet, "payload.tar"), "");
    writeFileSync(state, "{}");
    mkdirSync(join(bin, "tmp"), { mode: 0o711 });
    const pin =
      mode === "published-default"
        ? CI_WEB_IMAGE
        : "ghcr.io/aisflow/fvoci-ci-web@sha256:" + "a".repeat(64);
    const docker = String.raw`
const fs=require("node:fs"),a=process.argv.slice(2),file=process.env.FIXTURE_STATE,s=JSON.parse(fs.readFileSync(file,"utf8"));
const save=()=>fs.writeFileSync(file,JSON.stringify(s));
const name=a[a.indexOf("--name")+1];
const row={op:a.slice(0,2),name:a.includes("--name")?name:undefined};
if(a[0]==="run") {const phase=a.at(-2)==="inputs"?"inputs":a.at(-2)==="consume"?"consume":"service";row.phase=phase;row.network=a.includes("--network")?a[a.indexOf("--network")+1]:null;const env=fs.readFileSync(a[a.indexOf("--env-file")+1],"utf8");row.hasDatabase=env.includes("TEST_DATABASE_URL=");row.hasMeili=env.includes("MEILI_MASTER_KEY=");if(phase!=="service"){row.sdk=env.match(/^FVOCI_WEB_CONTAINER_SDK_SHA256=([a-f0-9]+)$/m)?.[1];row.imagePinned=a.includes(process.env.FIXTURE_PIN);}}
if(a[0]==="pull"||a[0]==="run")row.platform=a[a.indexOf("--platform")+1];
fs.appendFileSync(process.env.FIXTURE_CALLS,JSON.stringify(row)+"\n");
if(a[0]==="pull"){}
else if(a[0]==="context"&&a[1]==="inspect")console.log(JSON.stringify(process.env.FIXTURE_MODE==="remote-endpoint"?"tcp://PRIVATE_ENDPOINT_CANARY":"unix:///fixture/docker.sock"));
else if(a[0]==="info")console.log("linux "+(process.env.FIXTURE_MODE==="daemon-mismatch"?process.env.FIXTURE_OTHER_ARCH:["amd64","x64","arm64"].includes(process.env.FIXTURE_MODE)?process.env.FIXTURE_MODE:process.env.FIXTURE_ARCH));
else if(a[0]==="image"&&a[1]==="inspect")console.log("sha256:"+"b".repeat(64)+" linux "+(process.env.FIXTURE_MODE==="image-mismatch"?process.env.FIXTURE_OTHER_ARCH:process.env.FIXTURE_ARCH)+" "+process.env.FIXTURE_SHA+" "+JSON.stringify([process.env.FIXTURE_MODE==="foreign-digest"?"foreign":process.env.FIXTURE_PIN]));
else if(a[0]==="network"){}
else if(a[0]==="run") {s[name]=a.includes("-d");save();if(row.phase==="inputs"&&process.env.FIXTURE_MODE==="inputs-fail")process.exit(17);}
else if(a[0]==="exec"){if(a.includes("SHOW server_version_num"))console.log("180006");}
else if(a[0]==="inspect")console.log((s[a.at(-1)]?"true":"false")+" "+(!s[a.at(-1)]&&a.at(-1).includes("meili")?"143":"0")+" false");
else if(a[0]==="stop"){s[a.at(-1)]=false;save();}
else if(a[0]==="rm"){delete s[a.at(-1)];save();}
else process.exit(42);
`;
    writeFileSync(join(bin, "docker"), "#!" + process.execPath + "\n" + docker, { mode: 0o755 });
    const child = Bun.spawn(
      [
        process.execPath,
        "--no-env-file",
        CLI,
        "container-browser",
        root,
        "--consume",
        "2",
        "--handoff",
        packet,
        "--handoff-sha256",
        new Bun.CryptoHasher("sha256").update("{}").digest("hex"),
        ...(mode === "published-default"
          ? []
          : ["--image-pin", pin, "--sdk-sha256", "c".repeat(64)]),
      ],
      {
        env: {
          ...process.env,
          TMPDIR: join(bin, "tmp"),
          PATH: bin + ":" + (process.env.PATH ?? ""),
          CI: "true",
          GITHUB_ACTIONS: "true",
          GITHUB_JOB: "workspace-browser-shard",
          GITHUB_SHA: sha,
          GITHUB_REPOSITORY: "fixture/repo",
          GITHUB_RUN_ID: "12",
          GITHUB_RUN_ATTEMPT: "1",
          GITHUB_OUTPUT: output,
          FIXTURE_STATE: state,
          FIXTURE_CALLS: calls,
          FIXTURE_SHA: sha,
          FIXTURE_PIN: pin,
          FIXTURE_MODE: mode,
          FIXTURE_ARCH: nativeArchitecture(process.arch),
          FIXTURE_OTHER_ARCH: nativeArchitecture(process.arch) === "x86_64" ? "aarch64" : "x86_64",
          DOCKER_CONTEXT: "fixture",
          DOCKER_HOST: "",
          DOCKER_DEFAULT_PLATFORM:
            "linux/" + (nativeArchitecture(process.arch) === "x86_64" ? "amd64" : "arm64"),
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    const [code, stdout, stderr] = await Promise.all([
      child.exited,
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
    ]);
    const refused = [
      "daemon-mismatch",
      "image-mismatch",
      "foreign-digest",
      "remote-endpoint",
    ].includes(mode);
    expect(code, stderr).toBe(refused ? 1 : mode === "inputs-fail" ? 17 : 0);
    expect(stdout + stderr).not.toContain("PRIVATE_DEPENDENCY_CANARY");
    expect(stdout + stderr).not.toContain("PRIVATE_ENDPOINT_CANARY");
    const rows = readFileSync(calls, "utf8")
      .trim()
      .split("\n")
      .map(
        (line) =>
          JSON.parse(line) as {
            op: string[];
            phase?: string;
            network?: string | null;
            hasDatabase?: boolean;
            hasMeili?: boolean;
            platform?: string;
            sdk?: string;
            imagePinned?: boolean;
          },
      );
    expect(rows.filter((row) => row.op[0] === "pull")).toHaveLength(
      refused ? (["daemon-mismatch", "remote-endpoint"].includes(mode) ? 0 : 1) : 3,
    );
    if (refused) {
      expect(rows.some((row) => row.op[0] === "run" || row.op[0] === "network")).toBe(false);
      expect(JSON.parse(readFileSync(state, "utf8"))).toEqual({});
      return;
    }
    for (const row of rows.filter((row) => ["pull", "run"].includes(row.op[0] ?? "")))
      expect(row.platform).toBe(
        "linux/" + (nativeArchitecture(process.arch) === "x86_64" ? "amd64" : "arm64"),
      );
    if (mode === "published-default")
      for (const row of rows.filter((row) => row.phase === "inputs" || row.phase === "consume")) {
        expect(row.imagePinned).toBe(true);
        expect(row.sdk).toBe(
          nativeArchitecture(process.arch) === "x86_64"
            ? "dfd2e7b84d19f6e8ca03b80ded497d69606df8685ed6f1a929b6e8ecaba35892"
            : "171384584c2a218a591e6d42784578c28ab1f64741487090b285006022260cca",
        );
      }
    const inputs = rows.findIndex((row) => row.phase === "inputs"),
      consume = rows.findIndex((row) => row.phase === "consume"),
      network = rows.findIndex((row) => row.op[0] === "network" && row.op[1] === "create");
    expect(inputs).toBeGreaterThan(-1);
    expect(rows[inputs]?.network).toBeNull();
    expect(rows[inputs]?.hasDatabase).toBe(false);
    expect(rows[inputs]?.hasMeili).toBe(false);
    if (mode !== "inputs-fail") {
      expect(network).toBeGreaterThan(inputs);
      expect(consume).toBeGreaterThan(network);
      expect(rows[consume]?.network).toMatch(/^fvoci-browser-/);
      expect(rows[consume]?.hasDatabase).toBe(true);
    } else {
      expect(network).toBe(-1);
      expect(consume).toBe(-1);
    }
    expect(JSON.parse(readFileSync(state, "utf8"))).toEqual({});
  });

test("SQLite exports are four data fields, never shell evaluation", () => {
  const valid =
    "export SQLITE3_LIB_DIR=/owned/lib\nexport SQLITE3_INCLUDE_DIR=/owned/include\nexport SQLITE3_STATIC=1\nexport SQLITE3_NO_PKG_CONFIG=1\n";
  expect(sqliteEnvironment(valid).SQLITE3_LIB_DIR).toBe("/owned/lib");
  for (const text of [
    valid + "export PATH=/foreign\n",
    valid.replace("/owned/lib", "$(foreign)"),
    valid.replace("SQLITE3_NO_PKG_CONFIG", "SQLITE3_STATIC"),
    valid.replace("export SQLITE3_STATIC=1\n", ""),
  ])
    expect(() => sqliteEnvironment(text)).toThrow(CheckError);
});

test("cohort reuse retains the complete in-container program and rejects product source changes", async () => {
  const before =
    "const IMAGE='fixed'; async function browserPhase(){return 7;} async function containerBrowser(){return 0;}";
  const hostChange = before.replace(
    "containerBrowser(){return 0;}",
    "containerBrowser(){return 1;}",
  );
  await qualifyCohortHostProgram(before, hostChange, ["tools/web-e2e/run-web-e2e.ts"]);
  const rejected = async (promise: Promise<void>) => {
    let caught: unknown;
    try {
      await promise;
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(CheckError);
  };
  await rejected(qualifyCohortHostProgram(before, hostChange, ["src/main.rs"]));
  await rejected(
    qualifyCohortHostProgram(before, hostChange.replace("return 7", "return 8"), [
      "tools/web-e2e/run-web-e2e.ts",
    ]),
  );
  await rejected(
    qualifyCohortHostProgram(before, hostChange.replace("'fixed'", "'foreign'"), [
      "tools/web-e2e/run-web-e2e.ts",
    ]),
  );
});

describe("planSpecs", () => {
  test("returns the specs of a groups.ts plan line", () => {
    expect(planSpecs('{"specs":["e2e/a-flow.spec.ts","e2e/b-flow.spec.ts"]}')).toEqual([
      "e2e/a-flow.spec.ts",
      "e2e/b-flow.spec.ts",
    ]);
  });

  const malformed: [string, string][] = [
    ["not json", "malformed shard plan line"],
    ['["e2e/a.spec.ts"]', "not an object"],
    ["null", "not an object"],
    ['{"specs":"e2e/a.spec.ts"}', "has no specs"],
    ['{"specs":[]}', "has no specs"],
    ['{"specs":[1]}', "not a string"],
    ['{"specs":["e2e/a.spec.ts\\ne2e/b.spec.ts"]}', "invalid spec path"],
    ['{"specs":["../a.spec.ts"]}', "invalid spec path"],
    ['{"specs":["e2e/a.spec.ts"],"extra":1}', "only specs"],
    ['{"specs":[NaN]}', "malformed shard plan line"],
  ];
  for (const [line, message] of malformed) {
    test(`refuses ${line}`, () => {
      refused(() => planSpecs(line), message);
    });
  }
});

describe("resolvePath", () => {
  // Expected values are pathlib.Path(...).resolve() on the same tree.
  function tree(): string {
    const root = tempDir();
    mkdirSync(join(root, "real/sub"), { recursive: true });
    symlinkSync("real", join(root, "link"));
    symlinkSync("real/sub", join(root, "deep"));
    symlinkSync("../real", join(root, "real/sub/up"));
    symlinkSync("missing-target", join(root, "dangling"));
    writeFileSync(join(root, "file"), "");
    return root;
  }

  test("follows symlinks before a later .. and keeps missing components", () => {
    const root = tree();
    const cases: [string, string][] = [
      ["link", "real"],
      ["link/sub/../x", "real/x"],
      ["deep/..", "real"],
      ["deep/../y", "real/y"],
      ["dangling/t", "missing-target/t"],
      ["nope/../real", "real"],
      ["nope/a/b", "nope/a/b"],
      ["file/x", "file/x"],
      ["real/sub/up/sub", "real/real/sub"],
      ["real/sub/up/../..", ""],
      [".//real//sub/", "real/sub"],
      ["link/", "real"],
    ];
    for (const [input, expected] of cases) {
      const want = expected ? join(root, expected) : root;
      expect(resolvePath(input, root)).toBe(want);
      expect(resolvePath(`${root}/${input}`, "/")).toBe(want);
    }
    expect(resolvePath("/")).toBe("/");
    expect(resolvePath("/..")).toBe("/");
  });

  test("refuses a symlink loop and an empty path", () => {
    const root = tempDir();
    symlinkSync("loop-b", join(root, "loop-a"));
    symlinkSync("loop-a", join(root, "loop-b"));
    refused(() => resolvePath(join(root, "loop-a/x")), "too many levels of symbolic links");
    refused(() => resolvePath(""), "cannot resolve an empty path");
  });

  test("normalizes like PurePosixPath", () => {
    expect(normalizePath("/a//b/./c/")).toBe("/a/b/c");
    expect(normalizePath("/a/../b")).toBe("/a/../b");
    expect(normalizePath("//a")).toBe("//a");
    expect(normalizePath("///a")).toBe("/a");
    expect(normalizePath("a/.")).toBe("a");
  });
});

describe("requirePathWithin", () => {
  test("compares resolved paths by whole components", () => {
    const base = tempDir();
    const parent = join(base, "parent");
    mkdirSync(join(parent, "lib"), { recursive: true });
    mkdirSync(join(base, "parent-sibling/lib"), { recursive: true });
    mkdirSync(join(base, "outside"));
    symlinkSync(join(base, "outside"), join(parent, "escape"));
    symlinkSync(parent, join(base, "parent-link"));
    symlinkSync(join(parent, "lib"), join(base, "lib-link"));
    const inside: [string, string][] = [
      [join(parent, "lib"), parent],
      [parent, parent],
      [`${parent}/`, parent],
      [join(parent, "missing/lib"), parent],
      [join(base, "lib-link"), parent],
      [join(parent, "lib"), join(base, "parent-link")],
      [join(parent, "lib"), "/"],
    ];
    for (const [child, container] of inside) requirePathWithin(child, container);
    const outside: [string, string][] = [
      [join(base, "parent-sibling/lib"), parent],
      [join(parent, "escape/lib"), parent],
      [`${parent}/../outside`, parent],
      [`${parent}/missing/../../outside`, parent],
      [base, parent],
    ];
    for (const [child, container] of outside) {
      refused(() => {
        requirePathWithin(child, container);
      }, "is not within");
    }
  });
});

describe("createSafeDiagnostics", () => {
  test("creates a private directory and publishes it", () => {
    const runnerTemp = tempDir();
    const prefix = join(runnerTemp, "fvoci-selected-diagnostics");
    const output = join(runnerTemp, "github-output");
    createSafeDiagnostics(prefix, { RUNNER_TEMP: runnerTemp, GITHUB_OUTPUT: output });
    expect(statSync(prefix).mode & 0o777).toBe(0o700);
    expect(readFileSync(output, "utf8")).toBe(`selected-safe-diagnostics=${prefix}\n`);
  });

  test("accepts the prefix as pathlib compares it and publishes the normal form", () => {
    const runnerTemp = tempDir();
    const output = join(runnerTemp, "github-output");
    createSafeDiagnostics(`${runnerTemp}//fvoci-selected-diagnostics`, {
      RUNNER_TEMP: `${runnerTemp}/`,
      GITHUB_OUTPUT: output,
    });
    expect(readFileSync(output, "utf8")).toBe(
      `selected-safe-diagnostics=${runnerTemp}/fvoci-selected-diagnostics\n`,
    );
    refused(() => {
      createSafeDiagnostics(`${runnerTemp}/x/../fvoci-selected-diagnostics`, {
        RUNNER_TEMP: runnerTemp,
      });
    }, "must be");
  });

  test("refuses another path, an occupied or symlinked destination and a missing RUNNER_TEMP", () => {
    const runnerTemp = tempDir();
    const prefix = join(runnerTemp, "fvoci-selected-diagnostics");
    refused(() => {
      createSafeDiagnostics(prefix, {});
    }, "RUNNER_TEMP is required");
    refused(() => {
      createSafeDiagnostics(`${runnerTemp}/other`, { RUNNER_TEMP: runnerTemp });
    }, "must be");
    const linked = join(tempDir(), "temp-link");
    symlinkSync(runnerTemp, linked);
    refused(() => {
      createSafeDiagnostics(join(linked, "fvoci-selected-diagnostics"), { RUNNER_TEMP: linked });
    }, "must be");
    symlinkSync(tempDir(), prefix);
    refused(() => {
      createSafeDiagnostics(prefix, { RUNNER_TEMP: runnerTemp });
    }, "cannot create");
    rmSync(prefix);
    mkdirSync(prefix, { mode: 0o700 });
    refused(() => {
      createSafeDiagnostics(prefix, { RUNNER_TEMP: runnerTemp });
    }, "cannot create");
  });
});

describe("launcher receipt", () => {
  test("keeps the json.dump layout with null for stages that did not run", () => {
    expect(launcherReceipt(["not-run", "0", "7", "0", "7"])).toBe(
      '{"actual_launcher_exit": null, "ownership_return_exit": 0, "selected_final_exit": 7, "pending_exit": 0, "config_list_exit": 7}\n',
    );
  });

  for (const value of ["-1", "256", "07", "+1", " 1", "1.0", "", "not_run"]) {
    test(`refuses stage exit ${JSON.stringify(value)}`, () => {
      refused(() => launcherReceipt([value, "0", "0", "0", "0"]), "invalid stage exit status");
    });
  }

  function privateDir(): string {
    const prefix = join(tempDir(), "safe");
    mkdirSync(prefix, { mode: 0o700 });
    chmodSync(prefix, 0o700);
    return prefix;
  }

  test("writes once, mode 0600", () => {
    const prefix = privateDir();
    writeLauncherReceipt(prefix, ["0", "0", "0", "0", "not-run"]);
    const receipt = join(prefix, "launcher-stage.json");
    expect(lstatSync(receipt).mode & 0o777).toBe(0o600);
    expect(JSON.parse(readFileSync(receipt, "utf8"))).toEqual({
      actual_launcher_exit: 0,
      ownership_return_exit: 0,
      selected_final_exit: 0,
      pending_exit: 0,
      config_list_exit: null,
    });
    refused(() => {
      writeLauncherReceipt(prefix, ["1", "1", "1", "1", "1"]);
    }, "cannot create launcher stage receipt");
    expect(readFileSync(receipt, "utf8")).toContain('"actual_launcher_exit": 0');
  });

  test("refuses an invalid status before creating the receipt", () => {
    const prefix = privateDir();
    refused(() => {
      writeLauncherReceipt(prefix, ["0", "0", "x", "0", "0"]);
    }, "invalid stage exit status");
    expect(() => lstatSync(join(prefix, "launcher-stage.json"))).toThrow();
  });

  test("refuses a symlinked, shared or non-directory prefix", () => {
    const prefix = privateDir();
    const link = join(tempDir(), "link");
    symlinkSync(prefix, link);
    refused(() => {
      writeLauncherReceipt(link, ["0", "0", "0", "0", "0"]);
    }, "must be a 0700 directory");
    chmodSync(prefix, 0o750);
    refused(() => {
      writeLauncherReceipt(prefix, ["0", "0", "0", "0", "0"]);
    }, "must be a 0700 directory");
    const file = join(tempDir(), "file");
    writeFileSync(file, "");
    refused(() => {
      writeLauncherReceipt(file, ["0", "0", "0", "0", "0"]);
    }, "must be a 0700 directory");
  });
});

describe("CLI", () => {
  function cli(args: string[], env: Record<string, string | undefined> = process.env) {
    const result = Bun.spawnSync([process.execPath, CLI, ...args], {
      env,
      stdout: "pipe",
      stderr: "pipe",
    });
    return {
      status: result.exitCode,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    };
  }

  test("prints plan specs one per line and refuses with exit 1", () => {
    expect(cli(["plan-specs", '{"specs":["e2e/a-flow.spec.ts"]}'])).toEqual({
      status: 0,
      stdout: "e2e/a-flow.spec.ts\n",
      stderr: "",
    });
    const bad = cli(["plan-specs", '{"specs":[]}']);
    expect(bad.status).toBe(1);
    expect(bad.stderr).toBe('shard plan group has no specs: {"specs":[]}\n');
  });

  test("prefixes committed API refusals and prints the tested SHA on success", () => {
    const { root, sha } = committedCheckout();
    const env = {
      PATH: process.env.PATH,
      CI: "true",
      GITHUB_ACTIONS: "true",
      GITHUB_JOB: "workspace-browser-shard",
      GITHUB_SHA: sha,
    };
    expect(cli(["committed-api", root, "0", "false", "consume"], env)).toEqual({
      status: 0,
      stdout: `committed API outputs match tested checkout ${sha}\n`,
      stderr: "",
    });
    const refusedRun = cli(["committed-api", root, "0", "false", "consume"], {
      ...env,
      CI: "false",
    });
    expect(refusedRun).toEqual({
      status: 1,
      stdout: "",
      stderr: "committed API qualification failed: requires GitHub CI\n",
    });
  });

  test("rejects unknown commands, arity and flag values with exit 2", () => {
    for (const args of [
      [],
      ["nope"],
      ["plan-specs"],
      ["committed-api", "/", "", "yes", ""],
      ["committed-api", "/", "", "true", "later"],
    ]) {
      expect(cli(args).status).toBe(2);
    }
  });
});
