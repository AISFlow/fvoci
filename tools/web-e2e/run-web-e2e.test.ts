// run-web-e2e.ts checks on temporary checkouts and directories; the shard
// fixture (scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh) drives the
// same commands through the real wrapper.
import { afterEach, describe, expect, spyOn, test } from "bun:test";
import * as fs from "node:fs";
import {
  chmodSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
  watch,
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
  qualifyNativeChecks,
  selectedImageBudget,
  selectedAfterPending,
  selectedPermissionDiagnostics,
  requirePathWithin,
  resolvePath,
  writeLauncherReceipt,
  type CommittedApiRequest,
} from "./run-web-e2e.ts";
import { nativeArchitecture, nativeLinux } from "../selected-backend-ci/types.ts";
import { checkedOutput } from "../selected-backend-ci/runtime.ts";
import { registeredModules } from "../selected-backend-ci/admission.ts";
import type { LocalGrant } from "../selected-backend-ci/types.ts";

const CLI = join(import.meta.dir, "run-web-e2e.ts");
const scratch: string[] = [];

test("selected permission projection withholds private entries and admits only unique fixed failures", () => {
  const location = "selected permissions failure at tools/selected-backend-ci/runtime.ts:390",
    descendant = "selected permissions descendant prefix=output symlink=1 owner_match=1";
  expect(
    selectedPermissionDiagnostics(`PRIVATE_CREDENTIAL_CANARY\n${descendant}\n${location}\n`),
  ).toEqual([descendant, location]);
  expect(
    selectedPermissionDiagnostics(
      "selected permissions descendant prefix=sqlite symlink=0 owner_match=0\n",
    ),
  ).toEqual(["selected permissions descendant prefix=sqlite symlink=0 owner_match=0"]);
  for (const text of [
    descendant + " PRIVATE_CREDENTIAL_CANARY",
    descendant.replace("output", "/private/PRIVATE_CREDENTIAL_CANARY"),
    descendant.replace("symlink=1", "symlink=0"),
    descendant.replace("owner_match=1", "owner_match=2"),
    descendant + "\r",
    "\u001b[31m" + descendant,
    descendant + "\n" + descendant,
    descendant + "\n" + "x".repeat(16384),
    location.replace("tools/selected-backend-ci/runtime", "/private/runtime"),
    location + "\n" + location,
  ])
    expect(selectedPermissionDiagnostics(text)).toEqual([]);
});

test("checked child receives the current environment after browser cache selection", () => {
  const key = "FVOCI_CHILD_ENV_FIXTURE",
    previous = process.env[key];
  try {
    process.env[key] = "selected-private-browser-copy";
    expect(
      checkedOutput(
        [
          process.execPath,
          "--eval",
          'console.log(process.env.FVOCI_CHILD_ENV_FIXTURE ?? "absent")',
        ],
        process.cwd(),
      ),
    ).toBe("selected-private-browser-copy");
  } finally {
    if (previous === undefined) Reflect.deleteProperty(process.env, key);
    else process.env[key] = previous;
  }
});

function tempDir(): string {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "fvoci-run-web-e2e.")));
  scratch.push(directory);
  return directory;
}

afterEach(() => {
  for (const directory of scratch.splice(0)) rmSync(directory, { recursive: true, force: true });
});

test("selected image operations consume one original entry deadline", () => {
  expect(selectedImageBudget("901000", 1000)).toBe(900000);
  expect(selectedImageBudget("901000", 900999)).toBe(1);
  for (const deadline of [undefined, "NaN", "901000.5", "1000", "999", "901001"])
    expect(() => selectedImageBudget(deadline, 1000)).toThrow(/budget/);
});

test("pending failure survives a later selected admission exception while retirement is still attempted", async () => {
  const failure = new Error("modeled selected admission failure");
  let attempts = 0;
  const admission = () => {
    attempts++;
    return Promise.reject(failure);
  };
  expect(await selectedAfterPending(19, admission)).toBe(19);
  expect(attempts).toBe(1);
  let original: unknown;
  try {
    await selectedAfterPending(0, admission);
  } catch (error) {
    original = error;
  }
  expect(original).toBe(failure);
  expect(attempts).toBe(2);
});

for (const mode of [
  "normal",
  "pending-fail",
  "consume-fail",
  "prepare-fail",
  "preexisting",
  "expired",
])
  test("local selected caller with real other-UID grant and modeled image: " + mode, async () => {
    const { root } = committedCheckout(),
      base = tempDir(),
      bin = join(base, "bin"),
      temp = join(base, "cohort"),
      output = join(temp, "selected"),
      calls = join(base, "calls"),
      actorUid = (process.getuid?.() ?? 0) + 1,
      actorGid = (process.getgid?.() ?? 0) + 1;
    mkdirSync(bin);
    mkdirSync(temp, { mode: 0o700 });
    mkdirSync(output, { mode: 0o700 });
    const original = join(import.meta.dir, "../..");
    for (const name of registeredModules) {
      mkdirSync(join(root, name, ".."), { recursive: true });
      fs.copyFileSync(join(original, name), join(root, name));
    }
    for (const path of ["crates/collab-engine", "apps/web"])
      mkdirSync(join(root, path), { recursive: true });
    const bun = join(bin, "bun");
    fs.copyFileSync(process.execPath, bun);
    chmodSync(bun, 0o755);
    for (const name of ["start-test-postgres.sh", "start-test-meili.sh"])
      writeFileSync(join(root, "scripts", name), '#!/bin/bash\nexec "$@"\n', { mode: 0o755 });
    writeFileSync(
      join(root, "scripts/run-selected-backend-e2e.ts"),
      `import fs from "node:fs";
const mode=process.argv[2];fs.appendFileSync(process.env.FIXTURE_CALLS!,JSON.stringify({host:mode,uid:process.getuid?.(),gid:process.getgid?.(),lane:process.argv.includes("--lane")})+"\\n");
if(mode==="permissions")console.log((process.getgroups?.()??[]).sort((a,b)=>a-b).join(","));`,
    );
    writeFileSync(join(bin, "stat"), '#!/bin/sh\nprintf "986\\n"\n', { mode: 0o755 });
    const sha = checkedOutput(
      ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@invalid", "add", "."],
      root,
    );
    expect(sha).toBe("");
    checkedOutput(
      [
        "git",
        "-c",
        "user.name=fixture",
        "-c",
        "user.email=fixture@invalid",
        "commit",
        "-qm",
        "local fixture",
        "--no-verify",
        "--no-gpg-sign",
      ],
      root,
    );
    const source = checkedOutput(["git", "rev-parse", "HEAD"], root),
      tree = checkedOutput(["git", "rev-parse", "HEAD^{tree}"], root);
    const allocation = join(temp, "allocation.json");
    const grant: LocalGrant = {
      schema: 1,
      status: "GRANTED",
      executionMode: "orca-local",
      exclusiveLocalBatch: true,
      currentDispatchConfirmed: true,
      owner: "orca:fixture",
      runId: "run_ab12",
      dispatchId: "ctx_cd34",
      taskId: "task_ef56",
      workerTerminal: "fixture-worker",
      rootTerminal: "fixture-root",
      worktree: root,
      uid: actorUid,
      gid: actorGid,
      source,
      tree,
      outputRoot: output,
      allowedModes: [
        "prepare",
        "consume",
        "pending",
        "permissions",
        "config-list",
        "run",
        "owner-return",
      ],
      expiresUtc:
        mode === "expired" ? "2000-01-01T00:00:00Z" : new Date(Date.now() + 3600000).toISOString(),
      registrationHashes: Object.fromEntries(
        registeredModules.map((name) => [
          name,
          new Bun.CryptoHasher("sha256").update(readFileSync(join(root, name))).digest("hex"),
        ]),
      ),
      stageCommands: {},
    };
    const grantBytes = JSON.stringify(grant),
      grantSha = new Bun.CryptoHasher("sha256").update(grantBytes).digest("hex");
    writeFileSync(allocation, grantBytes, { mode: 0o600 });
    writeFileSync(calls, "");
    if (mode === "preexisting") {
      mkdirSync(join(root, "target"));
      writeFileSync(join(root, "target/keep"), "existing cache");
    }
    writeFileSync(
      join(bin, "docker"),
      "#!" +
        bun +
        "\n" +
        String.raw`
const fs=require("node:fs"),a=process.argv.slice(2),e=process.env;
if(a[0]==="info")console.log("linux amd64");
else if(a[0]==="image"&&a[1]==="inspect")console.log(a[3]==="{{json .Config.Env}}"?JSON.stringify(["PATH=/usr/local/bin:/usr/bin:/bin"]):"sha256:"+"b".repeat(64)+" linux amd64 "+e.FIXTURE_SOURCE+" "+JSON.stringify([e.FIXTURE_PIN]));
else if(a[0]==="inspect")console.log("false "+(e.FIXTURE_MODE===e.FIXTURE_LAST_PHASE+"-fail"?19:0)+" false");
else if(a[0]==="rm"){}
else if(a[0]==="cp"){fs.mkdirSync(a[2]+"/chromium-1248/chrome-linux64",{recursive:true});fs.writeFileSync(a[2]+"/chromium-1248/chrome-linux64/deb.deps","owned");}
else if(a[0]==="run"||a[0]==="create"){
const variables=Object.fromEntries(fs.readFileSync(a[a.indexOf("--env-file")+1],"utf8").trim().split("\n").map(line=>{const at=line.indexOf("=");return[line.slice(0,at),line.slice(at+1)];})),phase=a.at(-1),out=variables.FVOCI_SELECTED_CI_OUTPUT;
fs.appendFileSync(e.FIXTURE_CALLS,JSON.stringify({image:phase,op:a[0],uid:a[a.indexOf("--user")+1],grant: variables.FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256===e.FIXTURE_GRANT_SHA,readonly:a.includes("type=bind,source="+e.FIXTURE_GRANT+",target="+e.FIXTURE_GRANT+",readonly"),github:Object.keys(variables).some(key=>key==="CI"||key.startsWith("GITHUB_")),socket:a.some(v=>v.includes("docker.sock")),caps:a.includes("--cap-drop=ALL")&&a.includes("--security-opt=no-new-privileges")})+"\n");
if(a[0]==="run"){
 if(phase==="prepare"){fs.writeFileSync(variables.FVOCI_WEB_BUILD_HANDOFF+"/handoff.json","{}");fs.writeFileSync(out+"/producer-owned","generated");fs.writeFileSync(e.FIXTURE_ROOT+"/target/producer-owned","generated");}
 if(phase==="consume"&&(fs.existsSync(out+"/producer-owned")||fs.existsSync(e.FIXTURE_ROOT+"/target/producer-owned")))process.exit(42);
 if(e.FIXTURE_MODE===phase+"-fail")process.exit(19);
}
}else process.exit(42);
`,
    );
    chmodSync(join(bin, "docker"), 0o755);
    const preload = join(base, "disk.ts");
    writeFileSync(
      preload,
      'import {mock} from "bun:test";import * as fs from "node:fs";const original=fs.statfsSync;mock.module("node:fs",()=>({...fs,statfsSync:(...args:Parameters<typeof original>)=>({...original(...args),bavail:30_000_000,bsize:4096})}));',
    );
    const restore = () => {
      expect(
        Bun.spawnSync(
          [
            "sudo",
            "-n",
            "chown",
            "-R",
            `${String(process.getuid?.())}:${String(process.getgid?.())}`,
            base,
            root,
          ],
          { stdout: "pipe", stderr: "pipe" },
        ).exitCode,
      ).toBe(0);
    };
    expect(
      Bun.spawnSync(
        ["sudo", "-n", "chown", "-R", `${String(actorUid)}:${String(actorGid)}`, base, root],
        { stdout: "pipe", stderr: "pipe" },
      ).exitCode,
    ).toBe(0);
    let exit = -1;
    try {
      const child = Bun.spawn(
        [
          "sudo",
          "-n",
          "setpriv",
          `--reuid=${String(actorUid)}`,
          `--regid=${String(actorGid)}`,
          "--clear-groups",
          "--",
          bun,
          "--no-env-file",
          "--preload",
          preload,
          join(root, "tools/web-e2e/run-web-e2e.ts"),
          "entry",
          root,
          "--with-selected-backends",
        ],
        {
          cwd: root,
          stdout: "pipe",
          stderr: "pipe",
          env: {
            PATH: bin + ":/usr/bin:/bin",
            HOME: base,
            TMPDIR: base,
            DOCKER_HOST: "unix:///fixture/socket",
            FVOCI_E2E_PENDING: "1",
            FVOCI_SELECTED_EXECUTION_MODE: "orca-local",
            FVOCI_SELECTED_LOCAL_ALLOCATION: allocation,
            FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256: grantSha,
            FVOCI_CI_OWNER: grant.owner,
            FVOCI_ROOT_RUN_OWNER: grant.owner,
            FVOCI_CI_SELECTED_RUNS: join(output, "runtime"),
            FVOCI_LOCAL_RUN_ID: grant.runId,
            FVOCI_LOCAL_DISPATCH_ID: grant.dispatchId,
            FVOCI_LOCAL_TASK_ID: grant.taskId,
            ORCA_TERMINAL_HANDLE: grant.workerTerminal,
            FVOCI_LOCAL_ROOT_TERMINAL: grant.rootTerminal,
            FIXTURE_MODE: mode,
            FIXTURE_CALLS: calls,
            FIXTURE_SOURCE: source,
            FIXTURE_PIN: CI_WEB_IMAGE,
            FIXTURE_GRANT: allocation,
            FIXTURE_GRANT_SHA: grantSha,
            FIXTURE_ROOT: root,
          },
        },
      );
      const result = await Promise.all([
        child.exited,
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
      ]);
      exit = result[0];
      expect(result[1] + result[2]).not.toContain("unregistered module");
    } finally {
      restore();
    }
    expect(exit).toBe(mode === "normal" ? 0 : mode.endsWith("-fail") ? 19 : 1);
    expect(readFileSync(allocation, "utf8")).toBe(grantBytes);
    const rows = readFileSync(calls, "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line) as Record<string, unknown>);
    const work = rows.filter((row) => row.op === "run");
    expect(work.map((row) => row.image)).toEqual(
      mode === "expired" || mode === "preexisting"
        ? []
        : mode === "prepare-fail"
          ? ["prepare"]
          : mode === "consume-fail"
            ? ["prepare", "consume"]
            : ["prepare", "consume", "pending"],
    );
    for (const row of rows.filter((row) => row.image)) {
      expect(row).toMatchObject({
        uid: `${String(actorUid)}:${String(actorGid)}`,
        grant: true,
        readonly: true,
        github: false,
        socket: false,
        caps: true,
      });
    }
    const host = rows.filter((row) => row.host);
    expect(host.map((row) => row.host)).toEqual(
      mode === "normal" || mode === "pending-fail"
        ? ["permissions", "config-list", "run", "owner-return"]
        : [],
    );
    for (const row of host)
      expect(row).toMatchObject({ uid: actorUid, gid: actorGid, lane: false });
    if (mode === "preexisting")
      expect(readFileSync(join(root, "target/keep"), "utf8")).toBe("existing cache");
    if (fs.existsSync(join(output, "tmp")))
      expect(readdirSync(join(output, "tmp")).filter((name) => name.endsWith(".env"))).toEqual([]);
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

test("containers admit every complete ordinary shard, never a partial selection", () => {
  for (const value of ["0", "1", "2", "3", "4", "5", "6", "7"])
    expect(containerShard(value)).toBe(value);
  for (const value of [undefined, "", "-1", "6 --grep=skip", "8"])
    expect(() => containerShard(value)).toThrow(CheckError);
});

test("modeled service inspection binds owned identities and revalidates all fields before cleanup, even on body failure", async () => {
  const root = tempDir(),
    bin = join(root, "bin"),
    state = join(root, "state"),
    calls = join(root, "calls");
  mkdirSync(bin);
  writeFileSync(
    join(bin, "docker"),
    `#!${process.execPath}
import { readFileSync, writeFileSync, appendFileSync } from "node:fs";
const args = process.argv.slice(2), path = process.env.FIXTURE_STATE;
const state = JSON.parse(readFileSync(path, "utf8"));
appendFileSync(process.env.FIXTURE_CALLS, JSON.stringify(args) + "\\n");
const save = () => writeFileSync(path, JSON.stringify(state));
if (args[0] === "run") {
  const name = args[args.indexOf("--name") + 1];
  const id = (state.mode === "collision" ? "c" : name.includes("-pg-") ? "a" : "b").repeat(64);
  state.services[id] = { id, name, image: args[args.indexOf("--env-file") + 2], label: state.mode === "collision" ? "WITHHELD_SERVICE_CANARY" : args[args.indexOf("--label") + 1].split("=")[1] };
  save();
  if (state.mode === "partial" || state.mode === "collision") process.exit(23);
  console.log(id);
} else if (args[0] === "exec") {
  if (args.includes("psql")) console.log("180006");
} else if (args[0] === "inspect") {
  const format = args[args.indexOf("--format") + 1], key = args.at(-1);
  const service = state.services[key] ?? Object.values(state.services).find(s => s.name === key);
  if (!service) process.exit(3);
  if (format.includes(".Config.Env")) process.exit(90);
  if (format === "{{.Id}}") console.log(service.id);
  else if (format.includes(".State.Running")) console.log("false " + (service.name.includes("-pg-") ? "0" : "143") + " false");
  else if (!format.includes(".State.StartedAt")) console.log(JSON.stringify([service.id, "/" + service.name, service.image, {"fvoci.test-run": service.label}]));
  else {
    const fields = [service.id, "/" + service.name, service.image, "2026-10-17T00:00:00Z", 123, {"fvoci.test-run": service.label}, []];
    if (!state.body && state.mode === "missing") process.exit(3);
    if (!state.body && state.mode === "foreign") fields[5] = {"fvoci.test-run": "WITHHELD_SERVICE_CANARY"};
    if (state.body && state.mode.startsWith("change-")) {
      const at = Number(state.mode.slice(7));
      fields[at] = at === 4 ? 456 : at === 5 ? {"fvoci.test-run": service.label, extra: true} : at === 6 ? [{Source: "changed"}] : fields[at] + "changed";
    }
    console.log(JSON.stringify(fields) + "\\n" + fields.slice(2).map(v => JSON.stringify(v)).join("|"));
  }
} else if (args[0] === "stop" && state.mode === "cleanup") process.exit(3);
else if (args[0] === "rm") { delete state.services[args.at(-1)]; save(); }
`,
    { mode: 0o755 },
  );
  const driver = `
import { browserServices } from ${JSON.stringify(CLI)};
import {readFileSync,writeFileSync} from "node:fs";
const path = process.env.FIXTURE_STATE;
try {
 const result = await browserServices(${JSON.stringify(root)}, ${JSON.stringify(root)}, "owned-fixture-network", "linux/amd64", async env => {
  const ledger = JSON.parse(env.FVOCI_E2E_SERVICE_WITNESS);
  if (ledger.length !== 2 || ledger.some((e,i) => e.name !== ["FVOCI_TEST_PG_CONTAINER","FVOCI_TEST_MEILI_CONTAINER"][i] || e.container !== env[e.name] || e.inspected.split("|").length !== 5)) throw Error("invalid relay");
  const state = JSON.parse(readFileSync(path,"utf8")); state.body = true; writeFileSync(path,JSON.stringify(state));
  if (state.throwBody) throw Error("WITHHELD_SERVICE_CANARY");
  return state.bodyStatus;
 });
 console.log("RESULT=" + result);
} catch (error) {
 if (["partial","collision"].includes(JSON.parse(readFileSync(path,"utf8")).mode))
  console.log("ORIGINAL_CREATE_FAILURE=" + Number(error.message === "container browser prerequisite failed"));
 console.log("RESULT=11");
}
`;
  for (const [mode, bodyStatus, throwBody, expected, bodyRan] of [
    ["valid", 0, false, 0, true],
    ["missing", 0, false, 11, false],
    ["foreign", 0, false, 11, false],
    ["partial", 0, false, 11, false],
    ["collision", 0, false, 11, false],
    ...Array.from({ length: 7 }, (_, at) => ["change-" + String(at), 0, false, 1, true]),
    ["valid", 17, false, 17, true],
    ["change-3", 17, false, 17, true],
    ["valid", 0, true, 11, true],
    ["cleanup", 0, false, 1, true],
  ] as [string, number, boolean, number, boolean][]) {
    writeFileSync(
      state,
      JSON.stringify({ mode, bodyStatus, throwBody, body: false, services: {} }),
    );
    writeFileSync(calls, "");
    const child = Bun.spawn([process.execPath, "--no-env-file", "-e", driver], {
      cwd: root,
      env: {
        ...process.env,
        PATH: bin + ":" + (process.env.PATH ?? ""),
        FIXTURE_STATE: state,
        FIXTURE_CALLS: calls,
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    const [exit, out, err] = await Promise.all([
      child.exited,
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
    ]);
    expect(exit, mode).toBe(0);
    expect(out, mode).toBe(
      (["partial", "collision"].includes(mode) ? "ORIGINAL_CREATE_FAILURE=1\n" : "") +
        "RESULT=" +
        String(expected) +
        "\n",
    );
    expect(out + err).not.toContain("WITHHELD_SERVICE_CANARY");
    const final = JSON.parse(readFileSync(state, "utf8")) as {
      body: boolean;
      services: Record<string, unknown>;
    };
    expect(final.body, mode).toBe(bodyRan);
    if (mode === "collision") expect(Object.keys(final.services), mode).toEqual(["c".repeat(64)]);
    else expect(final.services, mode).toEqual({});
    const commands = readFileSync(calls, "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line) as string[]);
    expect(commands.filter((args) => args[0] === "stop").length, mode).toBe(
      mode === "collision" ? 0 : mode === "partial" ? 1 : 2,
    );
    expect(
      commands.some(
        (args) => ["stop", "rm"].includes(args[0] ?? "") && args.includes("c".repeat(64)),
      ),
    ).toBe(false);
    expect(commands.at(-1), mode).toEqual(["network", "rm", "owned-fixture-network"]);
    expect(
      readdirSync(root).some((name) => name.endsWith(".env")),
      mode,
    ).toBe(false);
    if (mode === "valid" && bodyRan) expect(err).toContain("service_witness_unchanged=PASS");
    if (mode.startsWith("change-")) expect(err).toContain("service witness revalidation failed");
  }
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

test("maintained default runs all ordinary shards in Docker and preserves explicit host selections", () => {
  expect(webRequest([], {}).route).toBe("docker");
  for (const shard of ["0", "1", "2", "3", "4", "5", "6", "7"])
    expect(webRequest(["--ci-shard", shard], {}).route).toBe("docker");
  const args = ["e2e/v050-task-timer.spec.ts", "--grep", "exact original filter", "--workers=1"];
  expect(webRequest(args, {}).specs).toEqual(args);
  expect(webRequest(args, {}).route).toBe("host");
  expect(webRequest([], { FVOCI_E2E_PENDING: "1" }).route).toBe("host");
  expect(webRequest(["--host"], {}).route).toBe("host");
  for (const shard of ["0", "2", "3"]) {
    expect(webRequest(["--ci-shard", shard], { GITHUB_ACTIONS: "true" }).route).toBe("host");
    expect(webRequest(["--ci-shard", shard], { FVOCI_E2E_PENDING: "1" }).route).toBe("host");
    expect(webRequest(["--host", "--ci-shard", shard], {}).route).toBe("host");
  }
  for (const args of [
    ["--ci-shard", "8"],
    ["--ci-shard", "2", "x.spec.ts"],
    ["--host", "--local-image-sdk"],
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
      expect(
        containerRequest(
          ["/checkout", "--native-checks"],
          { ...ci, GITHUB_JOB: "web-native-checks" },
          architecture,
        ).sdk,
      ).toEqual(local.sdk);
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
            ...Object.fromEntries(
              Object.entries(process.env).filter(
                ([key]) => key !== "CI" && !key.startsWith("GITHUB_"),
              ),
            ),
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
  "native-checks",
  "native-fail",
  "native-remove-fail",
  "native-cancel",
  ...(process.platform === "linux" && nativeArchitecture(process.arch) === "x86_64"
    ? [
        "selected-pending",
        "selected-fail",
        "selected-cancel",
        "selected-remove-fail",
        "selected-prepare-fail",
        "selected-copy-fail",
        "selected-copy-cancel",
        "selected-copy-remove-fail",
      ]
    : []),
  ...(nativeArchitecture(process.arch) === "x86_64" ? ["amd64", "x64"] : ["arm64"]),
  "inputs-fail",
  "consume-fail",
  "remove-fail",
  "daemon-mismatch",
  "image-mismatch",
  "foreign-digest",
  "remote-endpoint",
])
  test("isolated CI Docker topology: " + mode, async () => {
    const native = mode.startsWith("native-");
    const selected = mode.startsWith("selected-"),
      selectedPrepare = [
        "selected-prepare-fail",
        "selected-copy-fail",
        "selected-copy-cancel",
        "selected-copy-remove-fail",
      ].includes(mode);
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
      mode === "published-default" || native || selected
        ? CI_WEB_IMAGE
        : "ghcr.io/aisflow/fvoci-ci-web@sha256:" + "a".repeat(64);
    const docker = String.raw`
const fs=require("node:fs"),a=process.argv.slice(2),file=process.env.FIXTURE_STATE,s=JSON.parse(fs.readFileSync(file,"utf8"));
const save=()=>fs.writeFileSync(file,JSON.stringify(s));
const name=a[a.indexOf("--name")+1];
const row={op:a.slice(0,2),name:a.includes("--name")?name:undefined};
if(a[0]==="run"||a[0]==="create") {const phase=a.at(-1)==="prepare"&&name.startsWith("fvoci-selected-tool-")?"selected-prepare":a.at(-1)==="pending"?"selected-pending":a.at(-2)==="inputs"?"inputs":a.at(-2)==="consume"?"consume":a.at(-1)==="native-checks"?"native-checks":"service";row.phase=phase;row.safe=a.includes("--cpus=2")&&a.includes("--memory=16g")&&a.includes("--pids-limit=1024")&&a.includes("--cap-drop=ALL")&&a.includes("--security-opt=no-new-privileges")&&!a.some(x=>x.includes("docker.sock")||x==="--privileged"||x==="--pid=host");row.user=a[a.indexOf("--user")+1];row.network=a.includes("--network")?a[a.indexOf("--network")+1]:null;const env=fs.readFileSync(a[a.indexOf("--env-file")+1],"utf8");row.hasActionToken=env.includes("PRIVATE_ACTION_CANARY");row.wholeTemp=a.some(x=>x==="type=bind,source="+process.env.RUNNER_TEMP+",target="+process.env.RUNNER_TEMP);row.hasDatabase=env.includes("TEST_DATABASE_URL=");row.hasMeili=env.includes("MEILI_MASTER_KEY=");if(phase!=="service"){row.sdk=env.match(/^FVOCI_WEB_CONTAINER_SDK_SHA256=([a-f0-9]+)$/m)?.[1];row.imagePinned=a.includes(process.env.FIXTURE_PIN);}if(phase==="consume"){const ledger=JSON.parse(env.match(/^FVOCI_E2E_SERVICE_WITNESS=(.+)$/m)?.[1]??"null");row.serviceWitness=Array.isArray(ledger)&&ledger.length===2&&ledger.every(e=>env.includes(e.name+"="+e.container+"\n")&&typeof e.inspected==="string");}}
if((a[0]==="run"||a[0]==="create")&&name.startsWith("fvoci-selected-tool-")){const mount=a.find(x=>x.startsWith("type=bind,source=")&&x.endsWith(",target=/tmp"));row.nodeTemp=mount?.slice("type=bind,source=".length).split(",target=")[0];const facts=fs.lstatSync(row.nodeTemp);row.privateNodeTemp=row.nodeTemp.startsWith("/var/tmp/fvoci-selected-tool.")&&!facts.isSymbolicLink()&&facts.uid===process.getuid()&&(facts.mode&0o777)===0o700;row.privateBunCache=fs.readFileSync(a[a.indexOf("--env-file")+1],"utf8").split("\n").includes("BUN_INSTALL_CACHE_DIR=/tmp/fvoci-bun-install-cache");if(a[0]==="run"){const shim=row.nodeTemp+"/bun-node-fixture";fs.mkdirSync(shim);for(const executable of["node","bun"])fs.symlinkSync(process.execPath,shim+"/"+executable);const cache=row.nodeTemp+"/fvoci-bun-install-cache";fs.mkdirSync(cache);fs.mkdirSync(cache+"/package-bytes");fs.symlinkSync("package-bytes",cache+"/1.0.0@@@1");}}
if(a[0]==="pull"||a[0]==="run")row.platform=a[a.indexOf("--platform")+1];
fs.appendFileSync(process.env.FIXTURE_CALLS,JSON.stringify(row)+"\n");
if(a[0]==="pull"){}
else if(a[0]==="context"&&a[1]==="inspect")console.log(JSON.stringify(process.env.FIXTURE_MODE==="remote-endpoint"?"tcp://PRIVATE_ENDPOINT_CANARY":"unix:///fixture/docker.sock"));
else if(a[0]==="info")console.log("linux "+(process.env.FIXTURE_MODE==="daemon-mismatch"?process.env.FIXTURE_OTHER_ARCH:["amd64","x64","arm64"].includes(process.env.FIXTURE_MODE)?process.env.FIXTURE_MODE:process.env.FIXTURE_ARCH));
else if(a[0]==="image"&&a[1]==="inspect"&&a[3]==="{{json .Config.Env}}")console.log(JSON.stringify(["PATH=/usr/local/bin:/usr/bin:/bin"]));
else if(a[0]==="image"&&a[1]==="inspect")console.log("sha256:"+"b".repeat(64)+" linux "+(process.env.FIXTURE_MODE==="image-mismatch"?process.env.FIXTURE_OTHER_ARCH:process.env.FIXTURE_ARCH)+" "+process.env.FIXTURE_SHA+" "+JSON.stringify([process.env.FIXTURE_MODE==="foreign-digest"?"foreign":process.env.FIXTURE_PIN]));
else if(a[0]==="network"){}
else if(a[0]==="create"){s[name]={running:false,created:true};save();console.log("d".repeat(64));}
else if(a[0]==="cp"){if(process.env.FIXTURE_MODE==="selected-copy-fail"){console.error("PRIVATE_CONSOLE_CANARY");process.exit(23);}if(process.env.FIXTURE_MODE==="selected-copy-cancel"){fs.writeFileSync(process.env.FIXTURE_READY,"READY");await new Promise(resolve=>process.once("SIGTERM",resolve));process.exit(0);}if(!a[1].endsWith(":/opt/playwright/."))process.exit(42);const dest=a[2];fs.mkdirSync(dest+"/chromium-1248/chrome-linux64",{recursive:true});fs.writeFileSync(dest+"/chromium-1248/chrome-linux64/deb.deps","PRIVATE_DEPENDENCY_CANARY",{mode:0o600});}
else if(a[0]==="run") {s[name]=a.includes("-d");if(row.phase==="service"){s[name]={running:true,id:(name.includes("-pg-")?"a":"b").repeat(64),image:a[a.indexOf("--env-file")+2],label:a[a.indexOf("--label")+1].split("=")[1]};console.log(s[name].id);}save();if(row.phase==="selected-prepare"&&process.env.FIXTURE_MODE==="selected-prepare-fail"){console.error("PRIVATE_CONSOLE_CANARY".repeat(2000));console.error("web-e2e stage=selected-dependencies started at=2026-10-17T04:15:45Z");console.error("web-e2e stage=PRIVATE_CONSOLE_CANARY elapsed_seconds=0.001 exit=1");console.error("web-e2e stage=selected-dependencies elapsed_seconds=0.001 exit=1 PRIVATE_CONSOLE_CANARY");console.error("web-e2e stage=selected-dependencies finished at=2026-10-17T04:16:01Z elapsed_seconds=15.928 exit=19");process.exit(19);}if(row.phase==="selected-pending"&&process.env.FIXTURE_MODE==="selected-fail")process.exit(19);if(row.phase==="selected-pending"&&process.env.FIXTURE_MODE==="selected-cancel"){s[name]=true;save();fs.writeFileSync(process.env.FIXTURE_READY,"READY");await new Promise(resolve=>process.once("SIGTERM",resolve));}if(row.phase==="native-checks"&&process.env.FIXTURE_MODE==="native-cancel"){s[name]=true;save();await new Promise(resolve=>{process.once("SIGTERM",resolve);console.log("NATIVE_READY");});process.exit(0);}if(row.phase==="native-checks"&&process.env.FIXTURE_MODE==="native-fail")process.exit(19);if(row.phase==="inputs"&&process.env.FIXTURE_MODE==="inputs-fail")process.exit(17);if(row.phase==="consume"&&["consume-fail","remove-fail"].includes(process.env.FIXTURE_MODE)){console.error("PRIVATE_CONSOLE_CANARY".repeat(2000));console.error("web-e2e stage=sqlite-sdk-admit elapsed_seconds=0.010 exit=0");console.error("web-e2e stage=browser-handoff-consume started");console.error("web-e2e stage=PRIVATE_CONSOLE_CANARY elapsed_seconds=0.001 exit=1");console.error("web-e2e stage=browser-group elapsed_seconds=0.001 exit=1 PRIVATE_CONSOLE_CANARY");if(process.env.FIXTURE_MODE==="consume-fail"){console.error("web build handoff check_line=705");console.error("web build handoff refused: PRIVATE_CONSOLE_CANARY");console.error("web-e2e stage=browser-handoff-consume elapsed_seconds=0.020 exit=19");process.exit(19);}console.error("web-e2e stage=browser-handoff-consume elapsed_seconds=0.020 exit=0");}}
else if(a[0]==="exec"){if(a.includes("SHOW server_version_num"))console.log("180006");}
else if(a[0]==="inspect"){const key=Object.keys(s).find(k=>s[k]?.id===a.at(-1))??a.at(-1),v=s[key];if(a[2].startsWith("[")){const fields=[v.id,"/"+key,v.image,"2026-10-17T00:00:00Z",123,{"fvoci.test-run":v.label},[]];console.log(JSON.stringify(fields)+"\n"+fields.slice(2).map(x=>JSON.stringify(x)).join("|"));}else if(a[2]==="{{.Id}}")console.log(v.id);else console.log((typeof v==="object"?v.running:v?true:false)+" "+(((!v?.created&&key.startsWith("fvoci-selected-tool-")&&["selected-fail","selected-prepare-fail"].includes(process.env.FIXTURE_MODE))||key.endsWith("consume2")&&process.env.FIXTURE_MODE==="consume-fail"||key.endsWith("native-checks")&&process.env.FIXTURE_MODE==="native-fail")?"19":(typeof v==="object"?!v.running:!v)&&key.includes("meili")?"143":"0")+" false");}
else if(a[0]==="stop"){const key=Object.keys(s).find(k=>s[k]?.id===a.at(-1))??a.at(-1);if(typeof s[key]==="object")s[key].running=false;else s[key]=false;save();}
else if(a[0]==="rm"){if((a.at(-1).startsWith("fvoci-selected-tool-")&&["selected-remove-fail","selected-copy-remove-fail"].includes(process.env.FIXTURE_MODE)||a.at(-1).endsWith("consume2")&&process.env.FIXTURE_MODE==="remove-fail"||a.at(-1).endsWith("native-checks")&&process.env.FIXTURE_MODE==="native-remove-fail")){console.error("PRIVATE_REMOVE_CANARY");process.exit(23);}const key=Object.keys(s).find(k=>s[k]?.id===a.at(-1))??a.at(-1);delete s[key];save();}
else process.exit(42);
`;
    writeFileSync(join(bin, "docker"), "#!" + process.execPath + "\n" + docker, { mode: 0o755 });
    const selectedOutput = join(bin, "fvoci-selected-current");
    if (selected) {
      writeFileSync(output, "", { mode: 0o600 });
      for (const path of [
        selectedOutput,
        ...(selectedPrepare
          ? []
          : [
              join(selectedOutput, "tmp"),
              join(selectedOutput, "home"),
              join(bin, "fvoci-sqlite"),
              join(bin, "fvoci-image-cargo"),
            ]),
      ])
        mkdirSync(path, { mode: 0o700 });
    }
    let ready = false,
      rejectReady: (() => void) | undefined;
    let watcher: ReturnType<typeof watch> | undefined;
    const readyEvent = ["selected-cancel", "selected-copy-cancel"].includes(mode)
      ? new Promise<void>((resolve, reject) => {
          rejectReady = () => {
            if (!ready) reject(new Error("selected fixture exited before readiness"));
          };
          watcher = watch(bin, (_event, name) => {
            if (name !== "ready") return;
            try {
              if (readFileSync(join(bin, "ready"), "utf8") === "READY") {
                ready = true;
                watcher?.close();
                resolve();
              }
            } catch {
              /* creation event precedes completed write */
            }
          });
          watcher.unref();
        })
      : undefined;
    const diskPreload = join(bin, "disk-preload.ts");
    writeFileSync(
      diskPreload,
      String.raw`import {mock} from "bun:test";import * as fs from "node:fs";const original=fs.statfsSync;mock.module("node:fs",()=>({...fs,statfsSync:(...args:Parameters<typeof original>)=>({...original(...args),bavail:30_000_000,bsize:4096})}));`,
    );
    const child = Bun.spawn(
      [
        process.execPath,
        "--no-env-file",
        "--preload",
        diskPreload,
        CLI,
        selectedPrepare ? "entry" : selected ? "selected-image-pending" : "container-browser",
        root,
        ...(selectedPrepare
          ? ["--ci-use-committed-api", "--ci-prepare-selected"]
          : selected
            ? []
            : native
              ? ["--native-checks"]
              : [
                  "--consume",
                  "2",
                  "--handoff",
                  packet,
                  "--handoff-sha256",
                  new Bun.CryptoHasher("sha256").update("{}").digest("hex"),
                ]),
        ...(mode === "published-default" || native || selected
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
          GITHUB_JOB: selectedPrepare
            ? "collaboration-build"
            : native
              ? "web-native-checks"
              : "workspace-browser-shard",
          HOME: selectedPrepare ? bin : process.env.HOME,
          FVOCI_E2E_PENDING: selectedPrepare ? "1" : process.env.FVOCI_E2E_PENDING,
          GITHUB_SHA: sha,
          GITHUB_REPOSITORY: "fixture/repo",
          GITHUB_RUN_ID: "12",
          GITHUB_RUN_ATTEMPT: "1",
          GITHUB_OUTPUT: output,
          RUNNER_TEMP: bin,
          FVOCI_SELECTED_CI_OUTPUT: selected ? selectedOutput : undefined,
          FVOCI_SELECTED_CI_SQLITE_PARENT: selected ? join(bin, "fvoci-sqlite") : undefined,
          FVOCI_WEB_CONTAINER_IMAGE: selected ? pin : undefined,
          FVOCI_WEB_CONTAINER_SDK_SHA256: selected
            ? "dfd2e7b84d19f6e8ca03b80ded497d69606df8685ed6f1a929b6e8ecaba35892"
            : undefined,
          FVOCI_WEB_CONTAINER_IMAGE_SOURCE: selected ? sha : undefined,
          FVOCI_WEB_BUILD_HANDOFF: selectedPrepare
            ? join(bin, "fvoci-web-build-handoff")
            : undefined,
          FVOCI_SELECTED_IMAGE_DEADLINE: String(Date.now() + 900000),
          BUN_INSTALL_CACHE_DIR: "PRIVATE_BUN_CACHE_CANARY",
          ACTIONS_RUNTIME_TOKEN: "PRIVATE_ACTION_CANARY",
          FIXTURE_READY: join(bin, "ready"),
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
    if (readyEvent)
      void child.exited.then(
        () => rejectReady?.(),
        () => rejectReady?.(),
      );
    const stdoutResult = async () => {
      if (readyEvent) {
        await readyEvent;
        child.kill("SIGTERM");
        return new Response(child.stdout).text();
      }
      if (mode !== "native-cancel") return new Response(child.stdout).text();
      const reader = child.stdout.getReader(),
        decoder = new TextDecoder();
      let output = "";
      try {
        while (!output.includes("NATIVE_READY\n")) {
          const chunk = await reader.read();
          if (chunk.done) throw new Error("native fixture exited before ready");
          output += decoder.decode(chunk.value);
        }
        child.kill("SIGTERM");
        for (;;) {
          const chunk = await reader.read();
          if (chunk.done) break;
          output += decoder.decode(chunk.value);
        }
        return output;
      } finally {
        if (child.exitCode === null && !output.includes("NATIVE_READY\n")) child.kill("SIGTERM");
      }
    };
    const [code, stdout, stderr] = await Promise.all([
      child.exited,
      stdoutResult(),
      new Response(child.stderr).text(),
    ]);
    watcher?.close();
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
            serviceWitness?: boolean;
            safe?: boolean;
            hasActionToken?: boolean;
            wholeTemp?: boolean;
            privateNodeTemp?: boolean;
            privateBunCache?: boolean;
            nodeTemp?: string;
            user?: string;
          },
      );
    if (selected)
      for (const row of rows) if (row.privateNodeTemp && row.nodeTemp) scratch.push(row.nodeTemp);
    const refused = [
      "daemon-mismatch",
      "image-mismatch",
      "foreign-digest",
      "remote-endpoint",
    ].includes(mode);
    expect(code, stderr).toBe(
      mode === "native-cancel" || mode === "selected-cancel" || mode === "selected-copy-cancel"
        ? 143
        : refused ||
            mode === "remove-fail" ||
            mode === "native-remove-fail" ||
            mode === "selected-remove-fail" ||
            mode === "selected-copy-remove-fail"
          ? 1
          : mode === "selected-copy-fail"
            ? 23
            : mode === "inputs-fail"
              ? 17
              : mode === "consume-fail" ||
                  mode === "native-fail" ||
                  mode === "selected-fail" ||
                  mode === "selected-prepare-fail"
                ? 19
                : 0,
    );
    expect(stdout + stderr).not.toContain("PRIVATE_DEPENDENCY_CANARY");
    expect(stdout + stderr).not.toContain("PRIVATE_ENDPOINT_CANARY");
    expect(stdout + stderr).not.toContain("PRIVATE_CONSOLE_CANARY");
    expect(stdout + stderr).not.toContain("PRIVATE_REMOVE_CANARY");
    if (mode === "consume-fail" || mode === "remove-fail") {
      expect(stderr).toContain("web-e2e stage=sqlite-sdk-admit elapsed_seconds=0.010 exit=0");
      expect(stderr).toContain("web-e2e stage=browser-handoff-consume started");
      expect(stderr).toContain(
        mode === "consume-fail"
          ? "container-browser failure child_exit=19 inspect_exit=0 running=0 container_exit=19 oom=0 remove_exit=0"
          : "container-browser failure child_exit=0 inspect_exit=0 running=0 container_exit=0 oom=0 remove_exit=23",
      );
      if (mode === "consume-fail") expect(stderr).toContain("web build handoff check_line=705");
    }
    if (selected) {
      if (mode === "selected-prepare-fail") {
        expect(stderr).toContain(
          "web-e2e stage=selected-dependencies started at=2026-10-17T04:15:45Z",
        );
        expect(stderr).toContain(
          "web-e2e stage=selected-dependencies finished at=2026-10-17T04:16:01Z elapsed_seconds=15.928 exit=19",
        );
        const log = join(selectedOutput, "prepare.private.log");
        expect(readFileSync(log, "utf8")).toContain("PRIVATE_CONSOLE_CANARY");
        expect(statSync(log).mode & 0o777).toBe(0o600);
        expect(rows.filter((row) => row.op[0] === "create")).toHaveLength(1);
        expect(rows.filter((row) => row.op[0] === "cp")).toHaveLength(1);
        expect(rows.filter((row) => row.op[0] === "rm")).toHaveLength(2);
      }
      const runs = rows.filter((row) => row.op[0] === "run");
      const copyFailure = mode.startsWith("selected-copy-");
      expect(runs).toHaveLength(copyFailure ? 0 : 1);
      for (const row of rows.filter((row) => row.op[0] === "create" || row.op[0] === "run")) {
        expect(row.safe).toBe(true);
        expect(row.hasActionToken).toBe(false);
        expect(row.wholeTemp).toBe(false);
        expect(row.privateNodeTemp).toBe(true);
        expect(row.privateBunCache).toBe(true);
        expect(fs.existsSync(row.nodeTemp as string)).toBe(
          ["selected-remove-fail", "selected-copy-remove-fail"].includes(mode),
        );
        if (row.op[0] === "run")
          expect(fs.existsSync(join(row.nodeTemp as string, "fvoci-bun-install-cache"))).toBe(
            mode === "selected-remove-fail",
          );
      }
      expect(
        readdirSync(join(selectedOutput, "tmp")).some((entry) => entry.startsWith("bun-node")),
      ).toBe(false);
      expect(rows.some((row) => row.op[0] === "rm")).toBe(true);
      if (mode === "selected-copy-remove-fail")
        expect(rows.filter((row) => row.op[0] === "rm")).toHaveLength(1);
      expect(
        readdirSync(join(selectedOutput, "tmp")).filter((name) => name.endsWith(".env")),
      ).toEqual([]);
      if (!["selected-remove-fail", "selected-copy-remove-fail"].includes(mode))
        expect(JSON.parse(readFileSync(state, "utf8"))).toEqual({});
      return;
    }
    expect(rows.filter((row) => row.op[0] === "pull")).toHaveLength(
      refused ? (["daemon-mismatch", "remote-endpoint"].includes(mode) ? 0 : 1) : native ? 1 : 3,
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
    if (native) {
      const work = rows.filter((row) => row.op[0] === "run");
      expect(work).toHaveLength(1);
      expect(work[0]?.phase).toBe("native-checks");
      expect(work[0]?.network).toBeNull();
      expect(work[0]?.hasDatabase).toBe(false);
      expect(work[0]?.hasMeili).toBe(false);
      expect(work[0]?.imagePinned).toBe(true);
      expect(work[0]?.safe).toBe(true);
      expect(work[0]?.user).toBe(`${String(process.getuid?.())}:${String(process.getgid?.())}`);
      if (mode === "native-cancel") expect(rows.some((row) => row.op[0] === "stop")).toBe(true);
      expect(rows.some((row) => row.op[0] === "network")).toBe(false);
      expect(
        Object.keys(JSON.parse(readFileSync(state, "utf8")) as Record<string, unknown>),
      ).toHaveLength(mode === "native-remove-fail" ? 1 : 0);
      return;
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
      expect(rows[consume]?.serviceWitness).toBe(true);
    } else {
      expect(network).toBe(-1);
      expect(consume).toBe(-1);
    }
    const remaining: unknown = JSON.parse(readFileSync(state, "utf8"));
    if (mode === "remove-fail") {
      expect(Object.keys(remaining as object)).toHaveLength(1);
      expect(Object.entries(remaining as object)[0]).toEqual([
        expect.stringMatching(/-consume2$/),
        false,
      ]);
    } else expect(remaining).toEqual({});
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

test("native source admission rejects stale source, image, profile and wrong job", () => {
  const facts = fs.statfsSync("."),
    empty = fs.statfsSync(".");
  facts.bavail = 30_000_000;
  facts.bsize = 4096;
  empty.bavail = 0;
  const disk = spyOn(fs, "statfsSync").mockReturnValue(facts);
  try {
    const { root, sha } = committedCheckout();
    const sdk = containerRequest([root, "--native-checks"], {}).sdk;
    const env = {
      FVOCI_WEB_CONTAINER_LOCAL: "1",
      FVOCI_WEB_CONTAINER_JOB: "web-native-checks",
      FVOCI_WEB_CONTAINER_SOURCE: sha,
      FVOCI_WEB_CONTAINER_IMAGE: sdk.image,
      FVOCI_WEB_CONTAINER_SDK_SHA256: sdk.sdk,
      FVOCI_WEB_CONTAINER_LOCAL_IMAGE: "0",
      CARGO_BUILD_JOBS: "2",
      CARGO_INCREMENTAL: "0",
      CARGO_PROFILE_DEV_DEBUG: "0",
      CARGO_PROFILE_TEST_DEBUG: "0",
      CARGO_TARGET_DIR: join(root, "target"),
    };
    expect(() => {
      qualifyNativeChecks(root, env);
    }).not.toThrow();
    disk.mockReturnValue(empty);
    expect(() => {
      qualifyNativeChecks(root, env);
    }).toThrow(/20GB disk floor/);
    disk.mockReturnValue(facts);
    for (const changed of [
      { FVOCI_WEB_CONTAINER_SOURCE: "a".repeat(40) },
      { FVOCI_WEB_CONTAINER_IMAGE: "foreign" },
      { FVOCI_WEB_CONTAINER_SDK_SHA256: "b".repeat(64) },
      { CARGO_PROFILE_TEST_DEBUG: "2" },
      { CI: "true", GITHUB_ACTIONS: "true", GITHUB_JOB: "workspace-browser-shard" },
    ])
      expect(() => {
        qualifyNativeChecks(root, { ...env, ...changed });
      }).toThrow(CheckError);
    writeFileSync(join(root, "apps/web/openapi.json"), "dirty");
    expect(() => {
      qualifyNativeChecks(root, env);
    }).toThrow(/clean source/);
    for (const args of [
      ["--handoff", "/packet"],
      ["--reuse-cohort", "/packet"],
      ["--image-pin", sdk.image, "--sdk-sha256", sdk.sdk],
    ])
      expect(() => containerRequest([root, "--native-checks", ...args], {})).toThrow(
        /without packet or reuse overrides/,
      );
  } finally {
    disk.mockRestore();
  }
});
