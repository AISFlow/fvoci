import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { shardPlanLines } from "./groups.ts";

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "fvoci-shard-"));
  const bin = mkdtempSync(join(tmpdir(), "fvoci-shard-bin-"));
  mkdirSync(join(root, "tools/web-e2e"), { recursive: true });
  mkdirSync(join(root, "scripts"), { recursive: true });
  mkdirSync(join(root, "apps/web/e2e"), { recursive: true });
  writeFileSync(
    join(root, "apps/web/package.json"),
    '{"scripts":{"build":"echo fvoci-web-e2e-fake-bun-build >&2"}}\n',
  );
  writeFileSync(join(root, "apps/web/.gitignore"), "node_modules\n");
  symlinkSync(join(import.meta.dir, "../../node_modules"), join(root, "apps/web/node_modules"));
  for (const name of ["run.ts", "groups.ts", "labels.ts", "proc.ts", "child-env.ts"]) {
    cpSync(join(import.meta.dir, name), join(root, "tools/web-e2e", name));
  }
  writeFileSync(
    join(root, "tools/web-e2e/run-group.ts"),
    `console.log("fvoci-web-e2e-run-group " + process.argv.slice(2).join(" "));\nprocess.exit(Number(process.env.FVOCI_TEST_RUN_GROUP_EXIT ?? 0));\n`,
  );
  writeFileSync(
    join(root, "scripts/generate-api.sh"),
    "#!/usr/bin/env bash\necho fvoci-web-e2e-fake-generate-api >&2\n",
  );
  writeFileSync(
    join(root, "scripts/prepare-sqlite-ci.sh"),
    "#!/usr/bin/env bash\nset -euo pipefail\n[[ \"$1\" == --env-file && $# == 2 ]]\nprintf '%s\\n' 'export SQLITE3_LIB_DIR=/fixture/sqlite/lib' 'export SQLITE3_INCLUDE_DIR=/fixture/sqlite/include' 'export SQLITE3_STATIC=1' 'export SQLITE3_NO_PKG_CONFIG=1' >\"$2\"\n",
  );
  chmodSync(join(root, "scripts/generate-api.sh"), 0o755);
  chmodSync(join(root, "scripts/prepare-sqlite-ci.sh"), 0o755);
  writeFileSync(
    join(bin, "bun"),
    '#!/usr/bin/env bash\nif [[ "$*" == "--bun x --no-install playwright --version" ]]; then exit 0; fi\nif [[ "$*" == "--bun run build" ]]; then echo fvoci-web-e2e-fake-bun-build >&2; exit 0; fi\nexec "$FVOCI_REAL_BUN" "$@"\n',
  );
  writeFileSync(
    join(bin, "cargo"),
    '#!/usr/bin/env bash\necho "fvoci-web-e2e-fake-cargo $*" >&2\nexit 0\n',
  );
  chmodSync(join(bin, "bun"), 0o755);
  chmodSync(join(bin, "cargo"), 0o755);
  return {
    root,
    bin,
    cleanup: () => {
      rmSync(root, { recursive: true, force: true });
      rmSync(bin, { recursive: true, force: true });
    },
  };
}

function specs(root: string, count: number) {
  const dir = join(root, "apps/web/e2e");
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "workspace-flow.spec.ts"), "// fixture\n");
  writeFileSync(join(dir, "workspace-wiki-flow.spec.ts"), "// fixture\n");
  for (let index = 1; index <= count; index += 1)
    writeFileSync(join(dir, `extra-${index}-flow.spec.ts`), "// fixture\n");
}

function run(fx: ReturnType<typeof fixture>, args: string[], env: Record<string, string> = {}) {
  return spawnSync("bun", [join(fx.root, "tools/web-e2e/run.ts"), ...args], {
    cwd: fx.root,
    env: {
      ...process.env,
      PATH: `${fx.bin}:${process.env.PATH}`,
      CARGO_TARGET_DIR: join(fx.root, "target"),
      FVOCI_REAL_BUN: process.execPath,
      ...env,
    },
    encoding: "utf8",
  });
}

describe("web e2e shard entrypoint", () => {
  test("builds once and runs the planned groups in order", () => {
    const fx = fixture();
    specs(fx.root, 26);
    const result = run(fx, ["--ci-shard", "0"]);
    expect(result.status).toBe(0);
    expect(
      result.stderr.split("\n").filter((line) => line === "fvoci-web-e2e-fake-generate-api"),
    ).toHaveLength(1);
    const executed = result.stdout
      .split("\n")
      .filter((line) => line.startsWith("fvoci-web-e2e-run-group "))
      .map((line) => line.slice("fvoci-web-e2e-run-group ".length));
    const planned = shardPlanLines(join(fx.root, "apps/web/e2e"), 0, 8).map((line) =>
      line.specs.join(" "),
    );
    expect(executed).toEqual(planned);
    fx.cleanup();
  });

  test("an empty plan and a group failure do not count as success", () => {
    const fx = fixture();
    mkdirSync(join(fx.root, "apps/web/e2e"), { recursive: true });
    const empty = run(fx, ["--ci-shard", "0"]);
    expect(empty.status).not.toBe(0);
    expect(empty.stderr).not.toContain("fvoci-web-e2e-fake-generate-api");
    specs(fx.root, 26);
    const failed = run(fx, ["--ci-shard", "0"], { FVOCI_TEST_RUN_GROUP_EXIT: "1" });
    expect(failed.status).not.toBe(0);
    expect(failed.stderr).toContain("failed on group:");
    const override = run(fx, ["--ci-shard", "0"], { FVOCI_WEB_E2E_SHARD_COUNT: "16" });
    expect(override.status).not.toBe(0);
    expect(override.stderr).toContain("FVOCI_WEB_E2E_SHARD_COUNT must not override");
    expect(override.stderr).not.toContain("fvoci-web-e2e-fake-generate-api");
    fx.cleanup();
  });

  test("committed API consumer refuses dirty, symlink, and wrong job output", () => {
    const fx = fixture();
    specs(fx.root, 8);
    mkdirSync(join(fx.root, "apps/web/src/generated"), { recursive: true });
    mkdirSync(join(fx.root, "scripts/selected-backend-ci"), { recursive: true });
    writeFileSync(join(fx.root, "apps/web/openapi.json"), '{"openapi":"3.1.0"}\n');
    writeFileSync(join(fx.root, "apps/web/src/generated/api.ts"), "// committed fixture types\n");
    writeFileSync(
      join(fx.root, "scripts/selected-backend-ci/web-build-handoff.py"),
      "import os, sys\nassert sys.argv[1:] == ['consume']\nassert os.environ['FVOCI_WEB_BUILD_PHASE'] == 'consume'\nprint('fvoci-web-e2e-fake-handoff-consume')\nraise SystemExit(int(os.environ.get('FVOCI_TEST_HANDOFF_EXIT', '0')))\n",
    );
    const git = (...args: string[]) => {
      const result = spawnSync("git", ["-C", fx.root, ...args], { encoding: "utf8" });
      if (result.status !== 0) throw new Error(result.stderr);
    };
    git("init", "-q");
    git("add", "scripts", "apps");
    git(
      "-c",
      "user.name=Fixture",
      "-c",
      "user.email=fixture@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "-qm",
      "Tracked API fixture",
    );
    const sha = spawnSync("git", ["-C", fx.root, "rev-parse", "HEAD"], {
      encoding: "utf8",
    }).stdout.trim();
    const base = {
      CI: "true",
      GITHUB_ACTIONS: "true",
      GITHUB_JOB: "workspace-browser-shard",
      GITHUB_SHA: sha,
      FVOCI_SELECTED_CI_OUTPUT: join(fx.root, "browser-output"),
      FVOCI_WEB_BUILD_HANDOFF: join(fx.root, "browser-packet"),
      FVOCI_WEB_BUILD_HANDOFF_SHA256: "0".repeat(64),
    };
    const combined = (result: { stdout: string; stderr: string }) =>
      `${result.stdout}\n${result.stderr}`;
    const ok = run(fx, ["--ci-use-committed-api", "--ci-consume-browser", "--ci-shard", "0"], base);
    expect(ok.status, ok.stderr).toBe(0);
    expect(combined(ok)).not.toContain("fvoci-web-e2e-fake-generate-api");
    expect(combined(ok)).not.toContain("fvoci-web-e2e-fake-bun-build");
    expect(combined(ok)).toContain("fvoci-web-e2e-fake-handoff-consume");
    expect(ok.stderr).toMatch(/web-e2e stage=browser-handoff-consume elapsed_seconds=\d+ exit=0/);
    const wrongJob = run(
      fx,
      ["--ci-use-committed-api", "--ci-consume-browser", "--ci-shard", "0"],
      { ...base, GITHUB_JOB: "web-checks" },
    );
    expect(wrongJob.status).not.toBe(0);
    expect(wrongJob.stderr).toContain("wrong browser consumer job");
    expect(combined(wrongJob)).not.toContain("fvoci-web-e2e-fake-handoff-consume");
    writeFileSync(join(fx.root, "apps/web/src/generated/api.ts"), "// dirty fixture\n", {
      flag: "a",
    });
    const dirty = run(
      fx,
      ["--ci-use-committed-api", "--ci-consume-browser", "--ci-shard", "0"],
      base,
    );
    expect(dirty.status).not.toBe(0);
    expect(dirty.stderr).toContain("tracked checkout is dirty");
    expect(combined(dirty)).not.toContain("fvoci-web-e2e-fake-handoff-consume");
    git("restore", "apps/web/src/generated/api.ts");
    const original = readFileSync(join(fx.root, "apps/web/openapi.json"));
    const moved = join(fx.root, "copied-openapi.json");
    rmSync(join(fx.root, "apps/web/openapi.json"));
    writeFileSync(moved, original);
    symlinkSync(moved, join(fx.root, "apps/web/openapi.json"));
    const link = run(
      fx,
      ["--ci-use-committed-api", "--ci-consume-browser", "--ci-shard", "0"],
      base,
    );
    expect(link.status).not.toBe(0);
    expect(link.stderr).toContain("committed API qualification failed");
    expect(combined(link)).not.toContain("fvoci-web-e2e-fake-handoff-consume");
    fx.cleanup();
  });
});
