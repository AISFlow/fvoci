import { expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { contextFromTexts } from "./load.ts";
import type { Mapping } from "./py.ts";
import {
  WEB_COLLAB_LANES,
  verifyWebBrowserBudget,
  rawJobScalar,
  verifyWebBrowserBudgetJobs,
  verifyWebBuildHandoff,
  verifyWebBuildHandoffJobs,
  verifyCollaborationPreparationJobs,
  verifyNoRunScopedCacheKeys,
  verifyWebHostInputsJobs,
  WEB_SQLITE_JOBS,
} from "./web-browser.ts";
import { ROOT, RegistryTree, loadContext, realJobs } from "./rust2-fixture.ts";

const clone = <T>(value: T): T => structuredClone(value);
const steps = (jobs: Mapping, job: string) => (jobs[job] as Mapping)["steps"] as Mapping[];
const stepWith = (jobs: Mapping, job: string, match: (step: Mapping) => boolean) => {
  const step = steps(jobs, job).find(match);
  if (!step) throw new Error(`no matching step in ${job}`);
  return step;
};
const usesOf = (step: Mapping) => (typeof step["uses"] === "string" ? step["uses"] : "");
const byId = (id: string) => (step: Mapping) => step["id"] === id;
const isDownload = (step: Mapping) => usesOf(step).startsWith("actions/download-artifact@");
const unitStep = (jobs: Mapping) =>
  stepWith(jobs, "web-checks", (step) => step["name"] === "Web and editor unit regressions");

const requireStep = (step: Mapping) => step["name"] === "Require exact current native preparation";
const restoreStep = (step: Mapping) => step["name"] === "Restore current native preparation";
const runOf = (step: Mapping): string => {
  const run = step["run"];
  if (typeof run !== "string") throw new Error("step has no run script");
  return run;
};

test("every Web SQLite caller upgrades complete host inputs before capture without relaxing refusal", () => {
  const jobs = realJobs("web.yml");
  expect(verifyWebHostInputsJobs(jobs)).toEqual([]);
  for (const name of WEB_SQLITE_JOBS) {
    for (const [before, after] of [
      ["APT::Update::Error-Mode=any", "APT::Update::Error-Mode=persistent"],
      ["DEBIAN_FRONTEND=noninteractive", "DEBIAN_FRONTEND=dialog"],
      ['Always-Include-Phased-Updates "true"', 'Always-Include-Phased-Updates "false"'],
      ["--with-new-pkgs", ""],
      ["--no-remove", ""],
      [" linux-libc-dev", ""],
      [" libcrypt-dev", ""],
      ["libclang-18-dev=1:18.1.8-20ubuntu8", "libclang-18-dev"],
      ["dpkg-query -W >", "dpkg-query -W gcc >"],
      ["Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg", "Trusted: yes"],
    ] as const) {
      const bad = clone(jobs);
      const prep = stepWith(bad, name, byId("sqlite"));
      expect(runOf(prep)).toContain(before);
      prep["run"] = runOf(prep).replace(before, after);
      expect(verifyWebHostInputsJobs(bad).join("\n")).toContain(
        "complete unmasked host preparation",
      );
    }
    const masked = clone(jobs);
    stepWith(masked, name, byId("sqlite"))["continue-on-error"] = true;
    expect(verifyWebHostInputsJobs(masked).join("\n")).toContain(
      "complete unmasked host preparation",
    );
    const reordered = clone(jobs);
    const prep = stepWith(reordered, name, byId("sqlite"));
    const capture = 'dpkg-query -W > "$RUNNER_TEMP/fvoci-sqlite/build-packages.txt"\n';
    prep["run"] = runOf(prep)
      .replace(capture, "")
      .replace(
        "sudo env DEBIAN_FRONTEND=noninteractive apt-get upgrade",
        capture + "sudo env DEBIAN_FRONTEND=noninteractive apt-get upgrade",
      );
    expect(verifyWebHostInputsJobs(reordered).join("\n")).toContain(
      "complete unmasked host preparation",
    );
  }
});

test("SQLite diagnostics publish only the exact hash file after original mandatory steps", () => {
  const jobs = realJobs("web.yml");
  for (const name of WEB_SQLITE_JOBS) {
    for (const edit of [
      (step: Mapping) => {
        step["if"] = "always()";
      },
      (step: Mapping) => {
        step["continue-on-error"] = true;
      },
      (step: Mapping) => {
        (step["with"] as Mapping)["path"] = "${{ runner.temp }}/fvoci-sqlite/consumer-inputs.json";
      },
      (step: Mapping) => {
        (step["with"] as Mapping)["path"] = "${{ runner.temp }}/**";
      },
      (step: Mapping) => {
        step["uses"] = "actions/upload-artifact@main";
      },
    ]) {
      const bad = clone(jobs);
      edit(stepWith(bad, name, (step) => step["name"] === "Retain SQLite input hashes"));
      expect(verifyWebHostInputsJobs(bad).join("\n")).toContain("safe hash diagnostic");
    }
    const bad = clone(jobs),
      list = steps(bad, name);
    list.splice(list.findIndex(byId("sqlite")), 0, list.pop()!);
    expect(verifyWebHostInputsJobs(bad).join("\n")).toContain("safe hash diagnostic");
    const changed = clone(jobs);
    const record = stepWith(changed, name, byId("sqlite_diagnostics"));
    record["run"] = runOf(record).replace("const limit = 512", "const limit = Infinity");
    expect(verifyWebHostInputsJobs(changed).join("\n")).toContain("bounded hash-only diagnostic");
  }
});

test("actual workflow diagnostic hashes canaries, bounds rows, and cannot replace the original producer refusal", () => {
  const jobs = realJobs("web.yml");
  const run = runOf(stepWith(jobs, "collaboration-test-build", byId("sqlite_diagnostics")));
  const match =
    /^bun --no-env-file - <<'SQLITE_INPUT_HASHES'\n([\s\S]+)SQLITE_INPUT_HASHES\n$/.exec(run);
  expect(match).not.toBeNull();
  const code = match![1]!;
  const canary = "PRIVATE_INPUT_CANARY";
  const hash = (value: string) => createHash("sha256").update(value).digest("hex");
  const packages =
    Array.from({ length: 600 }, (_, n) => `canary-package-${n}\t1.0-${canary}`).join("\n") + "\n";
  const input = {
    inputs: { private: canary },
    exports: { private: canary },
    wrapper_sha256: "a".repeat(64),
    os_release: canary,
    architecture: canary,
    packages,
    libclang_path: canary,
    libclang_sha256: "b".repeat(64),
    build_environment_sha256: "c".repeat(64),
    rustc: canary,
    cache_identity: "d".repeat(64),
    unknown_secret: canary,
  };
  const base = mkdtempSync(join(tmpdir(), "web-input-hashes-"));
  try {
    const invoke = (name: string, contents: string | null, occupied = false) => {
      const parent = join(base, name);
      mkdirSync(join(parent, "fvoci-sqlite"), { recursive: true });
      if (contents !== null)
        writeFileSync(join(parent, "fvoci-sqlite/consumer-inputs.json"), contents);
      const output = join(parent, "github-output");
      writeFileSync(output, "");
      if (occupied) {
        mkdirSync(join(parent, "fvoci-sqlite-input-hashes"));
        writeFileSync(join(parent, "fvoci-sqlite-input-hashes/hashes.json"), canary);
      }
      const child = Bun.spawnSync([process.execPath, "--no-env-file", "-"], {
        stdin: Buffer.from(code),
        env: { PATH: "/usr/bin:/bin", RUNNER_TEMP: parent, GITHUB_OUTPUT: output },
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(child.exitCode).toBe(0);
      expect(child.stdout.toString() + child.stderr.toString()).not.toContain(canary);
      const text = readFileSync(join(parent, "fvoci-sqlite-input-hashes/hashes.json"), "utf8");
      return {
        text,
        output: readFileSync(output, "utf8"),
        stderr: child.stderr.toString(),
        parent,
      };
    };
    const good = invoke("good", JSON.stringify(input));
    expect(good.text).not.toContain(canary);
    const diagnostic = JSON.parse(good.text);
    expect(diagnostic.status).toBe("available");
    expect(diagnostic.identity_sha256).toBe(input.cache_identity);
    expect(diagnostic.fields.os_release).toBe(hash(JSON.stringify(canary)));
    expect(diagnostic.packages_sha256).toBe(hash(packages));
    expect(diagnostic.package_count).toBe(600);
    expect(diagnostic.package_entries).toHaveLength(512);
    expect(diagnostic.packages_truncated).toBe(true);
    expect(
      diagnostic.package_entries.every(
        (row: string[]) => row.length === 2 && row.every((value) => /^[0-9a-f]{64}$/.test(value)),
      ),
    ).toBe(true);
    expect(good.output).toBe("written=true\n");
    expect(statSync(join(good.parent, "fvoci-sqlite-input-hashes/hashes.json")).mode & 0o777).toBe(
      0o600,
    );
    const changed = invoke(
      "changed",
      JSON.stringify({ ...input, packages: packages + "last-package\t9.0\n" }),
    );
    expect(JSON.parse(changed.text).packages_sha256).not.toBe(diagnostic.packages_sha256);
    for (const [index, contents] of [
      null,
      canary,
      JSON.stringify({ ...input, cache_identity: [input.cache_identity] }),
      JSON.stringify({ ...input, packages: "" }),
      JSON.stringify({ ...input, packages: "duplicate\t1\nduplicate\t1\n" }),
      JSON.stringify({ ...input, packages: canary }),
      " ".repeat(4 * 1024 * 1024 + 1),
    ].entries()) {
      const unavailable = invoke(`missing-${index}`, contents);
      expect(JSON.parse(unavailable.text)).toEqual({ schema: 1, status: "unavailable" });
      expect(unavailable.output).toBe("written=true\n");
    }
    const occupied = invoke("occupied", JSON.stringify(input), true);
    expect(occupied.text).toBe(canary);
    expect(occupied.output).toBe(""); // An occupied destination can never be uploaded.
    expect(occupied.stderr).toContain("MISSING");
    const original = runOf(stepWith(jobs, "collaboration-test-build", requireStep));
    const identity = `X64-1.99.0-${"a".repeat(40)}-1-${"b".repeat(64)}`;
    expect(
      Bun.spawnSync(["bash", "-c", original], {
        env: {
          PATH: "/usr/bin:/bin",
          PRODUCER_RESULT: "success",
          PRODUCER_ARTIFACT: "123",
          PRODUCER_DIGEST: "c".repeat(64),
          PRODUCER_IDENTITY: identity,
          CURRENT_IDENTITY: identity.replace(/b$/, "c"),
        },
        stdout: "pipe",
        stderr: "pipe",
      }).exitCode,
    ).toBe(1);
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("collaboration preparation hands exact current-run targets over by artifact and compiles completely", () => {
  const jobs = realJobs("web.yml");
  expect(verifyCollaborationPreparationJobs(jobs)).toEqual([]);
  const fails = (bad: Mapping, message: string) => {
    expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(message);
  };
  for (const name of ["collaboration-main-build", "collaboration-test-build"]) {
    const bad = clone(jobs);
    Reflect.deleteProperty(bad, name);
    fails(bad, "must exist");
  }
  for (const name of ["collaboration-main-build", "collaboration-test-build"]) {
    for (const [key, value] of [
      ["name", "web-native-shared"],
      ["path", "target"],
      ["compression-level", 6],
      ["if-no-files-found", "warn"],
      ["retention-days", 7],
    ] as const) {
      const bad = clone(jobs);
      const save = stepWith(
        bad,
        name,
        (step) => step["name"] === "Save current native preparation",
      );
      (save["with"] as Mapping)[key] = value;
      fails(bad, "save only the packed current-run target as a run-scoped artifact");
    }
    for (const output of ["artifact_id", "target_sha256", "native_identity"]) {
      const bad = clone(jobs);
      ((bad[name] as Mapping)["outputs"] as Mapping)[output] = "${{ github.run_id }}";
      fails(bad, "outputs the artifact ID, digest and arch/SHA/attempt/SQLite identity");
    }
    const bad = clone(jobs);
    stepWith(bad, name, byId("native"))["run"] = "true\n";
    fails(bad, "pack the compiled target with its modes, mtimes and digest");
  }
  for (const name of ["collaboration-test-build", "collaboration-build"]) {
    for (const [key, value] of [
      ["artifact-ids", "${{ needs.foreign.outputs.artifact_id }}"],
      ["name", "web-native-main-1"],
      ["run-id", "1"],
      ["merge-multiple", false],
    ] as const) {
      const bad = clone(jobs);
      (stepWith(bad, name, isDownload)["with"] as Mapping)[key] = value;
      fails(bad, "exact current-run producer artifact ID");
    }
    const masked = clone(jobs);
    stepWith(masked, name, isDownload)["continue-on-error"] = true;
    fails(masked, "exact current-run producer artifact ID");
    for (const key of ["PRODUCER_RESULT", "PRODUCER_IDENTITY", "CURRENT_IDENTITY"]) {
      const bad = clone(jobs);
      (stepWith(bad, name, requireStep)["env"] as Mapping)[key] = "${{ github.run_id }}";
      fails(bad, "producer must match this run's arch/SHA/attempt/SQLite identity");
    }
    const unverified = clone(jobs);
    stepWith(unverified, name, restoreStep)["run"] = 'zstd -dc -- "$1" | tar -xf -\n';
    fails(unverified, "digest-verified current-run artifact restore before compilation");
  }
  for (const name of ["collaboration-main-build", "collaboration-test-build"]) {
    const bad = clone(jobs);
    stepWith(bad, name, (step) => step["name"] === "Compile native preparation")["run"] = "true\n";
    fails(bad, "complete unmasked compiler");
  }
});

test("the producer check admits only this run's attempt, SHA and SQLite identity", () => {
  const jobs = realJobs("web.yml");
  const identity = `X64-1.99.0-${"a".repeat(40)}-1-${"b".repeat(64)}`;
  const good = {
    PRODUCER_RESULT: "success",
    PRODUCER_ARTIFACT: "4242",
    PRODUCER_DIGEST: "c".repeat(64),
    PRODUCER_IDENTITY: identity,
    CURRENT_IDENTITY: identity,
  };
  const noSqlite = identity.replace(/[0-9a-f]{64}$/, "");
  const cases: [Record<string, string>, number][] = [
    [good, 0],
    [{ ...good, PRODUCER_RESULT: "skipped" }, 1],
    [{ ...good, PRODUCER_ARTIFACT: "" }, 1],
    [{ ...good, PRODUCER_DIGEST: "" }, 1],
    [{ ...good, PRODUCER_IDENTITY: identity.replace("-1-", "-2-") }, 1],
    [{ ...good, PRODUCER_IDENTITY: identity.replace(/b$/, "c") }, 1],
    [{ ...good, PRODUCER_IDENTITY: identity.replace("a", "d") }, 1],
    [{ ...good, PRODUCER_IDENTITY: noSqlite, CURRENT_IDENTITY: noSqlite }, 1],
  ];
  for (const name of ["collaboration-test-build", "collaboration-build"]) {
    const run = runOf(stepWith(jobs, name, requireStep));
    for (const [env, code] of cases) {
      const result = Bun.spawnSync(["bash", "-c", run], {
        env: { PATH: "/usr/bin:/bin", ...env },
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(result.exitCode, `${name} ${JSON.stringify(env)}`).toBe(code);
    }
  }
});

// The artifact zip drops file modes; Cargo needs the build scripts' and test
// binaries' modes and the outputs' mtimes back exactly as compiled.
test("a packed target restores modes and mtimes only under the producer digest", () => {
  const jobs = realJobs("web.yml");
  const base = mkdtempSync(join(tmpdir(), "web-native-handoff-"));
  try {
    for (const [producer, consumer] of [
      ["collaboration-main-build", "collaboration-test-build"],
      ["collaboration-test-build", "collaboration-build"],
    ] as const) {
      const temp = join(base, `${producer}-temp`);
      const source = join(base, producer);
      const executable = "target/debug/build/x-1/build-script-build";
      const depInfo = "target/debug/deps/x-1.d";
      mkdirSync(temp);
      mkdirSync(join(source, "target/debug/build/x-1"), { recursive: true });
      mkdirSync(join(source, "target/debug/deps"), { recursive: true });
      writeFileSync(join(source, executable), "#!/bin/sh\n");
      chmodSync(join(source, executable), 0o755);
      writeFileSync(join(source, depInfo), "x-1.d: src/lib.rs\n");
      utimesSync(join(source, depInfo), 1_700_000_000.25, 1_700_000_000.25);
      const output = join(temp, "github-output");
      writeFileSync(output, "");
      const env = { PATH: "/usr/bin:/bin", RUNNER_TEMP: temp, GITHUB_OUTPUT: output };
      const pack = Bun.spawnSync(["bash", "-c", runOf(stepWith(jobs, producer, byId("native")))], {
        cwd: source,
        env,
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(pack.exitCode, pack.stderr.toString()).toBe(0);
      const digest = /^target_sha256=([0-9a-f]{64})$/m.exec(readFileSync(output, "utf8"))?.[1];
      if (digest === undefined) throw new Error("pack wrote no digest");
      const restore = runOf(stepWith(jobs, consumer, restoreStep));
      // The consumer downloads into the same RUNNER_TEMP path the producer packed.
      for (const [value, code] of [
        ["0".repeat(64), 1],
        ["", 1],
        [digest, 0],
      ] as const) {
        const work = join(base, `${consumer}-${String(code)}-${String(value.length)}`);
        mkdirSync(work);
        const result = Bun.spawnSync(["bash", "-c", restore], {
          cwd: work,
          env: { ...env, PRODUCER_DIGEST: value },
          stdout: "pipe",
          stderr: "pipe",
        });
        expect(result.exitCode).toBe(code);
        expect(existsSync(join(work, "target"))).toBe(code === 0);
      }
      const restored = join(base, `${consumer}-0-64`);
      expect(statSync(join(restored, executable)).mode & 0o777).toBe(0o755);
      expect(statSync(join(restored, depInfo), { bigint: true }).mtimeNs).toBe(
        statSync(join(source, depInfo), { bigint: true }).mtimeNs,
      );
      expect(readFileSync(join(restored, executable), "utf8")).toBe("#!/bin/sh\n");
    }
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("no web job hands outputs over through a run-scoped cache key", () => {
  const jobs = realJobs("web.yml");
  expect(verifyNoRunScopedCacheKeys(jobs)).toEqual([]);
  for (const key of [
    "v2-x-${{ github.run_id }}",
    "v2-x-${{ github.run_attempt }}-${{ github.sha }}",
  ]) {
    const bad = clone(jobs);
    steps(bad, "collaboration-test-build").push({
      uses: "actions/cache/save@fixture",
      with: { path: "target", key },
    });
    expect(verifyNoRunScopedCacheKeys(bad)).toEqual([
      "web: collaboration-test-build must hand off run-scoped outputs by artifact, not by a cache key",
    ]);
  }
});

test("real web.yml passes through the context entry points", () => {
  const ctx = loadContext(ROOT);
  expect(verifyWebBrowserBudget(ctx)).toEqual([]);
  expect(verifyWebBuildHandoff(ctx)).toEqual([]);
  expect(verifyWebBuildHandoff({ root: ROOT, workflows: {} })).toEqual([]);
  expect(verifyWebBrowserBudget({ root: ROOT, workflows: { "web.yml": { jobs: {} } } })).toEqual(
    [],
  );
});

test("normal browser job budget is measured and fail closed", () => {
  const jobs = realJobs("web.yml");
  expect(verifyWebBrowserBudgetJobs(jobs, "20")).toEqual([]);
  expect((jobs["workspace-browser-shard"] as Mapping)["strategy"]).toEqual({
    "fail-fast": false,
    matrix: { shard: [0, 1, 2, 3, 4, 5, 6, 7] },
  });
  for (const job of [
    "web-static",
    "web-native-checks",
    "collaboration-build",
    ...WEB_COLLAB_LANES.map((lane) => lane.name),
  ]) {
    expect((jobs[job] as Mapping)["timeout-minutes"], job).toBe(15);
  }
  // Parsed values; the raw-scalar form (20.0, 020) is covered below.
  for (const value of [null, 0, 15, 16, 19, 21, 30, "20", 20.5, true]) {
    const bad = clone(jobs);
    (bad["workspace-browser-shard"] as Mapping)["timeout-minutes"] = value;
    expect(verifyWebBrowserBudgetJobs(bad, "20"), String(value)).toEqual([
      "web: normal browser shard requires the measured 20 minute job budget",
    ]);
  }
  for (const value of [null, [], "20"]) {
    const bad = clone(jobs);
    bad["workspace-browser-shard"] = value;
    expect(verifyWebBrowserBudgetJobs(bad, "20")).toEqual([
      "web: normal browser shard must be a mapping",
    ]);
  }
  const missing = clone(jobs);
  delete missing["workspace-browser-shard"];
  expect(verifyWebBrowserBudgetJobs(missing, "20")).toEqual([
    "web: normal browser shard must be a mapping",
  ]);
});

test("web current build handoff positive and fail closed", () => {
  const jobs = realJobs("web.yml");
  expect(verifyWebBuildHandoffJobs(jobs)).toEqual([]);
  const mutations: [string, (bad: Mapping) => void, string][] = [
    [
      "lane needs",
      (bad) => void ((bad["collaboration-install-on"] as Mapping)["needs"] = "ci-plan"),
      "consumer needs successful registered producer",
    ],
    [
      "producer budget",
      (bad) => void ((bad["collaboration-build"] as Mapping)["timeout-minutes"] = 16),
      "fixed runner/budget",
    ],
    [
      "prepare masked",
      (bad) =>
        void (stepWith(bad, "collaboration-build", byId("prepare"))["continue-on-error"] = true),
      "unconditional qualified producer",
    ],
    [
      "publish masked",
      (bad) =>
        void (stepWith(bad, "collaboration-build", byId("publish"))["continue-on-error"] = true),
      "publish only successful complete packet",
    ],
    [
      "browser masked",
      (bad) =>
        void (stepWith(bad, "collaboration-install-on", byId("browser"))["continue-on-error"] =
          true),
      "mandatory full original runtime",
    ],
    [
      "foreign run id",
      (bad) =>
        void ((stepWith(bad, "collaboration-postgres-on", isDownload)["with"] as Mapping)[
          "run-id"
        ] = "foreign"),
      "current-run exact artifact ID",
    ],
    [
      "digest output",
      (bad) =>
        void (((bad["collaboration-build"] as Mapping)["outputs"] as Mapping)["handoff_sha256"] =
          ""),
      "producer artifact identity",
    ],
    [
      "browser command",
      (bad) =>
        void (stepWith(bad, "collaboration-sqlite-off", byId("browser"))["run"] =
          "bash scripts/run-web-e2e.sh --ci-use-committed-api --with-selected-backends"),
      "mandatory full original runtime",
    ],
    [
      "target cache",
      (bad) =>
        void steps(bad, "collaboration-postgres-off").push({
          uses: "actions/cache@anything",
          with: { path: "target" },
        }),
      "consumer cannot borrow target cache",
    ],
    [
      "foreign download",
      (bad) =>
        void steps(bad, "collaboration-postgres-on").push({
          name: "Foreign download",
          uses: "actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333",
          with: {
            "artifact-ids": "${{ needs.collaboration-build.outputs.artifact_id }}",
            "merge-multiple": true,
            path: "${{ runner.temp }}/foreign",
            "run-id": "999999",
            "github-token": "${{ secrets.GITHUB_TOKEN }}",
          },
        }),
      "current-run exact artifact ID",
    ],
    [
      "missing receipt download",
      (bad) => {
        const job = bad["collaboration-sqlite-on"] as Mapping;
        job["steps"] = steps(bad, "collaboration-sqlite-on").filter(
          (step) =>
            (step["with"] as Mapping | undefined)?.["path"] !==
            "${{ runner.temp }}/fvoci-closed-install",
        );
      },
      "current-run exact artifact ID",
    ],
    [
      "unpinned download",
      (bad) =>
        void (stepWith(bad, "collaboration-install-on", isDownload)["uses"] =
          "actions/download-artifact@v4"),
      "current-run exact artifact ID",
    ],
    [
      "checkout credentials",
      (bad) =>
        void (stepWith(bad, "collaboration-build", (s) =>
          usesOf(s).startsWith("actions/checkout@"),
        )["with"] = {}),
      "default exact checkout",
    ],
    [
      "selection",
      (bad) => void ((bad["collaboration-sqlite-on"] as Mapping)["if"] = "always()"),
      "selection only by registered plan",
    ],
    [
      "authority",
      (bad) =>
        void ((bad["collaboration-build"] as Mapping)["permissions"] = { contents: "write" }),
      "no masked/alternate authority",
    ],
    [
      "non-mapping step",
      (bad) => void steps(bad, "collaboration-build").push("oops" as unknown as Mapping),
      "job collaboration-build steps must be a list of mappings",
    ],
    [
      "non-mapping job",
      (bad) => void (bad["collaboration-sqlite-off"] = null),
      "job collaboration-sqlite-off must be a mapping",
    ],
  ];
  for (const [name, mutate, needle] of mutations) {
    const bad = clone(jobs);
    mutate(bad);
    const errors = verifyWebBuildHandoffJobs(bad);
    expect(errors.join("\n"), name).toContain("web: current build handoff " + needle);
  }
});

test("selected registration commands cannot be missing or masked", () => {
  const jobs = realJobs("web.yml");
  for (const command of [
    "(cd apps/web && bun test e2e-pending/collab-playwright.config.test.ts --timeout 60000)",
  ]) {
    const bad = clone(jobs);
    const step = unitStep(bad);
    expect(step["run"] as string).toContain(command + "\n");
    step["run"] = (step["run"] as string).replace(command + "\n", "");
    expect(verifyWebBuildHandoffJobs(bad)).toEqual([
      "web: current build handoff mandatory complete web/editor and selected registration fixtures",
    ]);
  }
  for (const [key, value] of [
    ["if", "false"],
    ["continue-on-error", true],
  ] as const) {
    const bad = clone(jobs);
    unitStep(bad)[key] = value;
    expect(verifyWebBuildHandoffJobs(bad).length, key).toBe(1);
  }
});

const BUDGET_ERROR = "web: normal browser shard requires the measured 20 minute job budget";

function budgetWithRaw(raw: string): string[] {
  using tree = new RegistryTree([]);
  const rel = ".github/workflows/web.yml";
  const text = tree.read(rel);
  const start = text.indexOf("  workspace-browser-shard:\n");
  const tail = text.slice(start);
  expect(tail).toContain("\n    timeout-minutes: 20\n");
  tree.write(
    rel,
    text.slice(0, start) +
      tail.replace("\n    timeout-minutes: 20\n", `\n    timeout-minutes: ${raw}\n`),
  );
  return verifyWebBrowserBudget(loadContext(tree.root));
}

test("browser budget raw scalar must be a plain decimal integer", () => {
  expect(budgetWithRaw("20")).toEqual([]);
  expect(budgetWithRaw("20 # measured")).toEqual([]);
  // Bun.YAML reads each of these as the number 20; PyYAML does not.
  for (const raw of ["20.0", "020", "0x14", "+20", "2_0", "20.", "2e1"]) {
    expect(budgetWithRaw(raw), raw).toEqual([BUDGET_ERROR]);
  }
});

// Applies `edit` to the workspace-browser-shard job text only; `append` also
// adds a copy of the job, with that budget line set to 20.0, at the file end.
function budgetWithSource(edit: (job: string) => string, append = ""): string[] {
  using tree = new RegistryTree([]);
  const rel = ".github/workflows/web.yml";
  const text = tree.read(rel);
  const start = text.indexOf("  workspace-browser-shard:\n");
  const end = text.slice(start + 1).search(/\n {2}[^ #\n]/) + start + 2;
  const job = text.slice(start, end);
  const changed =
    text.slice(0, start) +
    edit(job) +
    text.slice(end) +
    (append === "" ? "" : job.replace(append, "\n    timeout-minutes: 20.0\n"));
  expect(changed).not.toBe(text);
  tree.write(rel, changed);
  const ctx = loadContext(tree.root);
  return [...verifyWebBrowserBudget(ctx), ...verifyWebBuildHandoff(ctx)];
}

test("duplicate spellings of the budget path fail closed", () => {
  // Both loaders keep the last duplicate (Python then refuses 20.0) while a
  // first-match raw lookup would read the earlier 20.
  const line = "\n    timeout-minutes: 20\n";
  for (const second of [
    '"timeout-minutes": 20.0',
    "'timeout-minutes': 20.0",
    '"timeout\\u002dminutes": 20.0',
  ]) {
    const errors = budgetWithSource((text) => text.replace(line, `${line}    ${second}\n`));
    expect(errors, second).toEqual([BUDGET_ERROR]);
  }
  const duplicateJob = budgetWithSource(
    (job) => job + job.replace(line, "\n    timeout-minutes: 20.0\n"),
  );
  expect(duplicateJob).toEqual([BUDGET_ERROR]);
  // The reviewer's form: the copy with 20.0 appended at the end of jobs.
  expect(budgetWithSource((job) => job, line)).toEqual([BUDGET_ERROR]);
  // A quoted single spelling is not the plain form the raw check accepts.
  expect(budgetWithSource((text) => text.replace(line, '\n    "timeout-minutes": 20\n'))).toEqual([
    BUDGET_ERROR,
  ]);
  // Unrelated duplicates elsewhere in the job stay outside this check.
  expect(
    budgetWithSource((text) => text.replace(line, `${line}    "name": dup\n    'name': dup\n`)),
  ).toEqual([]);
});

test("raw block scalar refuses unclassified key lines on the path", () => {
  const job = (body: string) => `jobs:\n  j:\n${body}`;
  expect(rawJobScalar(job("    t: 5\n"), "j", "t")).toBe("5");
  for (const body of [
    "    <<: *base\n    t: 5\n",
    "    ? t\n    : 5\n",
    "    &a t: 5\n",
    "    \tt: 5\n",
  ]) {
    expect(rawJobScalar(job(body), "j", "t"), JSON.stringify(body)).toBeNull();
  }
  expect(rawJobScalar('"jobs":\n  j:\n    t: 5\njobs:\n  j:\n    t: 6\n', "j", "t")).toBeNull();
  expect(rawJobScalar("jobs:\n  'j':\n    t: 5\n  j:\n    t: 6\n", "j", "t")).toBeNull();
});

test("raw job scalar lookup is exact and fails closed", () => {
  const source = [
    "on: push",
    "jobs:",
    "  a:",
    "    timeout-minutes: 7 # c",
    "    steps:",
    "      - timeout-minutes: 9",
    "  b:",
    "    run: |",
    "      timeout-minutes: 3",
    "  c:",
    "    timeout-minutes: 1",
    "    timeout-minutes: 2",
    "",
  ].join("\n");
  expect(rawJobScalar(source, "a", "timeout-minutes")).toBe("7");
  expect(rawJobScalar(source, "b", "timeout-minutes")).toBeNull();
  expect(rawJobScalar(source, "c", "timeout-minutes")).toBeNull();
  expect(rawJobScalar(source, "missing", "timeout-minutes")).toBeNull();
  expect(rawJobScalar("jobs: {a: {timeout-minutes: 7}}\n", "a", "timeout-minutes")).toBeNull();
  expect(verifyWebBrowserBudgetJobs(realJobs("web.yml"), null)).toEqual([BUDGET_ERROR]);
});

test("the budget scalar comes from the context text, not the file on disk", () => {
  const source = readFileSync(join(ROOT, ".github/workflows/web.yml"), "utf8");
  const marker = "\n    timeout-minutes: 20\n";
  const start = source.indexOf("  workspace-browser-shard:\n");
  expect(source.indexOf(marker, start)).toBeGreaterThan(start);
  const at = source.indexOf(marker, start);
  const text =
    source.slice(0, at) + "\n    timeout-minutes: 20.0\n" + source.slice(at + marker.length);
  expect(verifyWebBrowserBudget(contextFromTexts(ROOT, { "web.yml": text }))).toEqual([
    BUDGET_ERROR,
  ]);
  expect(verifyWebBrowserBudget(contextFromTexts(ROOT, { "web.yml": source }))).toEqual([]);
});

test("collaboration normalizes source times only after an exact current-run artifact restore", () => {
  const jobs = realJobs("web.yml");
  const normalizerName = "Normalize tracked source mtimes for native cache reuse";
  expect(
    steps(jobs, "collaboration-main-build").some((step) => step["name"] === normalizerName),
  ).toBe(false);
  for (const name of ["collaboration-test-build", "collaboration-build"]) {
    const list = steps(jobs, name);
    const at = list.findIndex((step) => step["name"] === normalizerName);
    expect(at, name).toBeGreaterThan(-1);
    const restoreAt = list.findIndex(
      (step) => step["name"] === "Restore current native preparation",
    );
    expect(at, name).toBeGreaterThan(restoreAt);
    expect(at, name).toBeLessThan(
      list.findIndex(
        (step) => step["name"] === "Compile native preparation" || step["id"] === "prepare",
      ),
    );
    for (const edit of ["true\n", "git ls-files -z | xargs -0 touch --\n"]) {
      const bad = clone(jobs);
      const normalizer = steps(bad, name)[at];
      if (!normalizer) throw new Error("missing source mtime normalizer");
      normalizer["run"] = edit;
      expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
        "deterministic tracked source mtimes",
      );
    }
    for (const before of [0, restoreAt]) {
      const bad = clone(jobs);
      const reordered = steps(bad, name);
      const [normalizer] = reordered.splice(at, 1);
      if (!normalizer) throw new Error("missing source mtime normalizer");
      reordered.splice(before, 0, normalizer);
      expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
        "after exact current-run artifact restore",
      );
    }
  }
  const bad = clone(jobs);
  const normalizer = stepWith(
    jobs,
    "collaboration-test-build",
    (step) => step["name"] === normalizerName,
  );
  steps(bad, "collaboration-main-build").splice(1, 0, clone(normalizer));
  expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
    "long-term seed must retain checkout source times",
  );
});

test("normalized collaboration jobs reject long-term native and helper fallback caches", () => {
  const jobs = realJobs("web.yml");
  for (const name of ["collaboration-test-build", "collaboration-build"]) {
    expect(
      steps(jobs, name).some(
        (step) => (step["with"] as Mapping | undefined)?.["path"] === "crates/collab-engine/target",
      ),
    ).toBe(false);
    for (const path of [
      "target",
      "crates/collab-engine/target",
      "target\ncrates/collab-engine/target\n",
    ]) {
      for (const fallback of [false, true]) {
        for (const before of [true, false]) {
          const bad = clone(jobs);
          const restore: Mapping = {
            name: "Restore old helper",
            id: "old_helper",
            uses: "actions/cache/restore@55cc8345863c7cc4c66a329aec7e433d2d1c52a9",
            with: {
              path,
              key: "old-source",
              ...(fallback ? { "restore-keys": "old-" } : {}),
            },
          };
          const list = steps(bad, name);
          list.splice(
            before
              ? 1
              : list.findIndex(
                  (step) =>
                    step["id"] === "prepare" || step["name"] === "Compile native preparation",
                ),
            0,
            restore,
          );
          expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
            "no long-term native cache in normalized jobs",
          );
        }
      }
    }
  }
});
