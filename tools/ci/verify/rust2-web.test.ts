import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { contextFromTexts } from "./load.ts";
import { isMapping, type Mapping } from "./py.ts";
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
  verifyWebContainerHandoffJobs,
  WEB_SQLITE_JOBS,
} from "./web-browser.ts";
import { ROOT, RegistryTree, loadContext, realJobs } from "./rust2-fixture.ts";
import { SQLITE_PACKAGES, verifySqlitePrefixCache } from "./rust-sqlite.ts";

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

const runOf = (step: Mapping): string => {
  const run = step["run"];
  if (typeof run !== "string") throw new Error("step has no run script");
  return run;
};

test("every Web SQLite caller installs required packages before capture without unrelated upgrades", () => {
  const jobs = realJobs("web.yml");
  expect(verifyWebHostInputsJobs(jobs)).toEqual([]);
  for (const name of WEB_SQLITE_JOBS) {
    for (const [before, after] of [
      ["APT::Update::Error-Mode=any", "APT::Update::Error-Mode=persistent"],
      ["DEBIAN_FRONTEND=noninteractive", "DEBIAN_FRONTEND=dialog"],
      ['Always-Include-Phased-Updates "true"', 'Always-Include-Phased-Updates "false"'],
      [
        "apt-get install",
        "apt-get upgrade -y --with-new-pkgs --no-remove\nsudo env DEBIAN_FRONTEND=noninteractive apt-get install",
      ],
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
        "sudo env DEBIAN_FRONTEND=noninteractive apt-get install",
        capture + "sudo env DEBIAN_FRONTEND=noninteractive apt-get install",
      );
    expect(verifyWebHostInputsJobs(reordered).join("\n")).toContain(
      "complete unmasked host preparation",
    );
  }
});

test("Rust native handoff callers align host inputs before existing qualified SQLite capture", () => {
  const jobs = realJobs("rust.yml");
  expect(verifySqlitePrefixCache(jobs)).toEqual([]);
  for (const name of ["postgres-build", "postgres", "collaboration"]) {
    for (const [before, after] of [
      ["APT::Update::Error-Mode=any", "APT::Update::Error-Mode=persistent"],
      ["DEBIAN_FRONTEND=noninteractive", "DEBIAN_FRONTEND=dialog"],
      ['Always-Include-Phased-Updates "true"', 'Always-Include-Phased-Updates "false"'],
      [
        "apt-get install",
        "apt-get upgrade -y --with-new-pkgs --no-remove\nsudo env DEBIAN_FRONTEND=noninteractive apt-get install",
      ],
      ["--no-remove", ""],
      [" linux-libc-dev", ""],
      [" libcrypt-dev", ""],
      ["libclang-18-dev=1:18.1.8-20ubuntu8", "libclang-18-dev"],
    ] as const) {
      const bad = clone(jobs);
      const prep = stepWith(bad, name, byId("sqlite"));
      expect(runOf(prep)).toContain(before);
      prep["run"] = runOf(prep).replace(before, after);
      expect(verifySqlitePrefixCache(bad).join("\n")).toContain(
        "host packages must be aligned before SQLite capture",
      );
    }
    const reordered = clone(jobs);
    const prep = stepWith(reordered, name, byId("sqlite"));
    prep["run"] = runOf(prep)
      .replace(SQLITE_PACKAGES + "\n", "")
      .replace(
        "sudo env DEBIAN_FRONTEND=noninteractive apt-get install",
        SQLITE_PACKAGES + "\n" + "sudo env DEBIAN_FRONTEND=noninteractive apt-get install",
      );
    expect(verifySqlitePrefixCache(reordered).join("\n")).toContain(
      "host packages must be aligned before SQLite capture",
    );
    const masked = clone(jobs);
    stepWith(masked, name, byId("sqlite"))["continue-on-error"] = true;
    expect(verifySqlitePrefixCache(masked).join("\n")).toContain("exact preflight/restore/verify");
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
    const last = list.pop();
    if (last === undefined) throw new Error("diagnostic upload step missing");
    list.splice(list.findIndex(byId("sqlite")), 0, last);
    expect(verifyWebHostInputsJobs(bad).join("\n")).toContain("safe hash diagnostic");
    const changed = clone(jobs);
    const record = stepWith(changed, name, byId("sqlite_diagnostics"));
    record["run"] = runOf(record).replace("const limit = 512", "const limit = Infinity");
    expect(verifyWebHostInputsJobs(changed).join("\n")).toContain("bounded hash-only diagnostic");
  }
});

test("actual workflow diagnostic hashes canaries, bounds rows, and cannot replace the original producer refusal", () => {
  const jobs = realJobs("web.yml");
  const run = runOf(stepWith(jobs, "collaboration-build", byId("sqlite_diagnostics")));
  const match =
    /^bun --no-env-file - <<'SQLITE_INPUT_HASHES'\n([\s\S]+)SQLITE_INPUT_HASHES\n$/.exec(run);
  expect(match).not.toBeNull();
  const [, code] = match ?? [];
  if (code === undefined) throw new Error("workflow diagnostic code missing");
  const canary = "PRIVATE_INPUT_CANARY";
  const hash = (value: string) => createHash("sha256").update(value).digest("hex");
  const packages =
    Array.from({ length: 600 }, (_, n) => `canary-package-${String(n)}\t1.0-${canary}`).join("\n") +
    "\n";
  const input = {
    inputs: { private: canary },
    exports: { private: canary },
    wrapper_sha256: "a".repeat(64),
    os_release: canary,
    architecture: canary,
    libclang_files: { private: canary },
    libclang_path: canary,
    libclang_sha256: "b".repeat(64),
    build_environment_sha256: "c".repeat(64),
    rustc: canary,
    cache_identity: "d".repeat(64),
    unknown_secret: canary,
  };
  const base = mkdtempSync(join(tmpdir(), "web-input-hashes-"));
  try {
    const invoke = (
      name: string,
      contents: string | null,
      occupied = false,
      packageText: string | null = packages,
    ) => {
      const parent = join(base, name);
      mkdirSync(join(parent, "fvoci-sqlite"), { recursive: true });
      if (contents !== null)
        writeFileSync(join(parent, "fvoci-sqlite/consumer-inputs.json"), contents);
      if (packageText !== null)
        writeFileSync(join(parent, "fvoci-sqlite/build-packages.txt"), packageText);
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
    const parseDiagnostic = (text: string): Mapping => {
      const value: unknown = JSON.parse(text);
      if (!isMapping(value)) throw new Error("diagnostic must be an object");
      return value;
    };
    const diagnostic = parseDiagnostic(good.text);
    const fields = diagnostic.fields;
    const entries = diagnostic.package_entries;
    if (!isMapping(fields) || !Array.isArray(entries))
      throw new Error("diagnostic fields or package entries missing");
    expect(diagnostic.status).toBe("available");
    expect(diagnostic.identity_sha256).toBe(input.cache_identity);
    expect(fields.os_release).toBe(hash(JSON.stringify(canary)));
    expect(diagnostic.packages_sha256).toBe(hash(packages));
    expect(diagnostic.package_count).toBe(600);
    expect(diagnostic.package_entries).toHaveLength(512);
    expect(diagnostic.packages_truncated).toBe(true);
    expect(
      entries.every(
        (row: unknown) =>
          Array.isArray(row) &&
          row.length === 2 &&
          row.every((value: unknown) => typeof value === "string" && /^[0-9a-f]{64}$/.test(value)),
      ),
    ).toBe(true);
    expect(good.output).toBe("written=true\n");
    expect(statSync(join(good.parent, "fvoci-sqlite-input-hashes/hashes.json")).mode & 0o777).toBe(
      0o600,
    );
    const changed = invoke(
      "changed",
      JSON.stringify(input),
      false,
      packages + "last-package\t9.0\n",
    );
    expect(parseDiagnostic(changed.text).packages_sha256).not.toBe(diagnostic.packages_sha256);
    for (const [index, contents] of [
      null,
      canary,
      JSON.stringify({ ...input, cache_identity: [input.cache_identity] }),
      JSON.stringify({ ...input, libclang_files: null }),
      " ".repeat(4 * 1024 * 1024 + 1),
    ].entries()) {
      const unavailable = invoke(`missing-${String(index)}`, contents);
      expect(parseDiagnostic(unavailable.text)).toEqual({ schema: 1, status: "unavailable" });
      expect(unavailable.output).toBe("written=true\n");
    }
    for (const [index, packageText] of [
      null,
      "",
      "duplicate\t1\nduplicate\t1\n",
      canary,
      " ".repeat(4 * 1024 * 1024 + 1),
    ].entries()) {
      const unavailable = invoke(
        `packages-${String(index)}`,
        JSON.stringify(input),
        false,
        packageText,
      );
      expect(parseDiagnostic(unavailable.text)).toEqual({ schema: 1, status: "unavailable" });
    }
    const occupied = invoke("occupied", JSON.stringify(input), true);
    expect(occupied.text).toBe(canary);
    expect(occupied.output).toBe(""); // An occupied destination can never be uploaded.
    expect(occupied.stderr).toContain("MISSING");
    const original = runOf(
      stepWith(
        jobs,
        "collaboration-install-on",
        (step) => step["name"] === "Require a successful exact producer artifact",
      ),
    );
    expect(
      Bun.spawnSync(["bash", "-c", original], {
        env: {
          PATH: "/usr/bin:/bin",
          PRODUCER_RESULT: "success",
          PRODUCER_ARTIFACT: "123",
          PRODUCER_DIGEST: "",
        },
        stdout: "pipe",
        stderr: "pipe",
      }).exitCode,
    ).toBe(1);
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("one collaboration producer rejects obsolete stages and keeps complete qualification", () => {
  const jobs = realJobs("web.yml");
  expect(verifyCollaborationPreparationJobs(jobs)).toEqual([]);
  for (const name of ["collaboration-main-build", "collaboration-test-build"]) {
    const bad = clone(jobs);
    bad[name] = clone(jobs["collaboration-build"]);
    expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
      "obsolete preparation job must be absent",
    );
  }
  for (const needs of [[], ["ci-plan", "collaboration-test-build"], ["ci-plan", "foreign"]]) {
    const bad = clone(jobs);
    (bad["collaboration-build"] as Mapping)["needs"] = needs;
    expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
      "single producer needs only ci-plan",
    );
  }
  for (const edit of [
    "true\n",
    "bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-prepare-browser\n",
  ]) {
    const bad = clone(jobs);
    stepWith(bad, "collaboration-build", byId("prepare"))["run"] = edit;
    expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
      "complete unmasked compiler and packet qualification",
    );
  }
  for (const key of ["GITHUB_RUN_ATTEMPT", "GITHUB_SHA", "CARGO_TARGET_DIR"]) {
    const bad = clone(jobs);
    (stepWith(bad, "collaboration-build", byId("prepare"))["env"] as Mapping)[key] = "foreign";
    expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
      "complete unmasked compiler and packet qualification",
    );
  }
  const borrowed = clone(jobs);
  steps(borrowed, "collaboration-build").push({
    uses: "actions/download-artifact@fixture",
    with: { name: "old-target" },
  });
  expect(verifyCollaborationPreparationJobs(borrowed).join("\n")).toContain(
    "cannot borrow an intermediate target",
  );
});

test("no web job hands outputs over through a run-scoped cache key", () => {
  const jobs = realJobs("web.yml");
  expect(verifyNoRunScopedCacheKeys(jobs)).toEqual([]);
  for (const key of [
    "v2-x-${{ github.run_id }}",
    "v2-x-${{ github.run_attempt }}-${{ github.sha }}",
  ]) {
    const bad = clone(jobs);
    steps(bad, "collaboration-build").push({
      uses: "actions/cache/save@fixture",
      with: { path: "target", key },
    });
    expect(verifyNoRunScopedCacheKeys(bad)).toEqual([
      "web: collaboration-build must hand off run-scoped outputs by artifact, not by a cache key",
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

test("ordinary image route requires all eight shards and qualified current-run artifact", () => {
  const jobs = realJobs("web.yml");
  expect(verifyWebContainerHandoffJobs(jobs)).toEqual([]);
  const mutations: ((bad: Mapping) => void)[] = [
    (bad) => {
      delete bad["workspace-browser-container-build"];
    },
    (bad) => {
      (bad["workspace-browser-container-build"] as Mapping)["timeout-minutes"] = 21;
    },
    (bad) => {
      stepWith(bad, "workspace-browser-shard", byId("container_browser"))["run"] = "true";
    },
    ...["workspace-browser-container-build", "workspace-browser-shard"].map(
      (job) => (bad: Mapping) => {
        const step = stepWith(
          bad,
          job,
          byId(job === "workspace-browser-shard" ? "container_browser" : "prepare"),
        );
        step.run = runOf(step) + ' --image-pin "latest" --sdk-sha256 ""';
      },
    ),
    ...["workspace-browser-container-build", "workspace-browser-shard"].map(
      (job) => (bad: Mapping) => {
        const step = stepWith(
          bad,
          job,
          byId(job === "workspace-browser-shard" ? "container_browser" : "prepare"),
        );
        delete (step.env as Mapping).TMPDIR;
      },
    ),
    (bad) => {
      stepWith(bad, "workspace-browser-shard", byId("container_browser"))["if"] = "false";
    },
    (bad) => {
      (bad["workspace-browser-shard"] as Mapping).strategy = {
        matrix: { shard: [0, 1, 2, 3, 4, 6, 7] },
      };
    },
    (bad) => {
      const dl = stepWith(
        bad,
        "workspace-browser-shard",
        (step) =>
          usesOf(step).startsWith("actions/download-artifact") &&
          String((step["with"] as Mapping)["artifact-ids"]).includes("container-build"),
      );
      (dl["with"] as Mapping)["run-id"] = "foreign";
    },
  ];
  for (const mutate of mutations) {
    const bad = clone(jobs);
    mutate(bad);
    expect(verifyWebContainerHandoffJobs(bad).length).toBeGreaterThan(0);
  }
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

test("single collaboration producer keeps seed source times and rejects helper fallback caches", () => {
  const jobs = realJobs("web.yml");
  const changed = clone(jobs);
  steps(changed, "collaboration-build").push({ run: "git ls-files -z | xargs -0 touch --\n" });
  expect(verifyCollaborationPreparationJobs(changed).join("\n")).toContain(
    "retain checkout source times",
  );
  for (const path of ["crates/collab-engine/target", "target\ncrates/collab-engine/target\n"]) {
    const bad = clone(jobs);
    steps(bad, "collaboration-build").push({
      uses: "actions/cache/restore@fixture",
      with: { path, key: "old-source" },
    });
    expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
      "no helper fallback cache",
    );
  }
  for (const field of ["key", "restore-keys"]) {
    const bad = clone(jobs);
    const seed = stepWith(bad, "collaboration-build", byId("cache_collaboration_main_seed"));
    (seed["with"] as Mapping)[field] = "foreign";
    expect(verifyCollaborationPreparationJobs(bad).join("\n")).toContain(
      "only identity-bound root seed",
    );
  }
});
