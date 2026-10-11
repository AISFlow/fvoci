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
