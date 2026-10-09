import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { test } from "bun:test";
import {
  assertRunnableList,
  assertShardCoverage,
  assertTimerCoverage,
  listTests,
  repositoryRoot,
  timerRuns,
} from "./groups";
import { fixtureTest, groupFixture, pairFiles } from "./fixtures";

const require = createRequire(import.meta.url);
const cli = resolve(dirname(require.resolve("playwright/package.json")), "cli.js");
const reporter = resolve(import.meta.dir, "acceptance-reporter.ts");

function withFixture(
  files: Record<string, string>,
  body: (fixture: ReturnType<typeof groupFixture>) => void,
): void {
  const fixture = groupFixture(files);
  try {
    body(fixture);
  } finally {
    fixture.cleanup();
  }
}
function shards(
  options: Parameters<typeof listTests>[0],
  total: number,
): ReturnType<typeof listTests>[] {
  return Array.from({ length: total }, (_, index) =>
    listTests({ ...options, shard: String(index + 1) + "/" + String(total) }),
  );
}

// Original names are retained for the traceability map; assertions exercise the
// installed standard CLI rather than reproducing the historical LPT scheduler.
test("test_observed_costs_reduce_concentrated_makespan → standard shard coverage", () => {
  withFixture(
    pairFiles(Array.from({ length: 20 }, (_, i) => "new-" + String(i) + ".spec.ts")),
    ({ options }) => {
      const full = listTests(options);
      assertShardCoverage(full, shards(options, 8));
      assert.equal(full.length, 22);
    },
  );
});
test("test_longest_first_and_deterministic_ties → standard shard determinism", () => {
  withFixture(pairFiles(["alpha.spec.ts", "beta.spec.ts"]), ({ options }) => {
    assert.deepEqual(shards(options, 2), shards(options, 2));
  });
});
test("test_unknown_specs_and_workspace_pair_covered_once", () => {
  withFixture(
    pairFiles([
      "v050-task-timer.spec.ts",
      ...Array.from({ length: 20 }, (_, i) => "new-" + String(i) + ".spec.ts"),
    ]),
    ({ options }) => {
      const full = listTests(options);
      const selected = shards(options, 8);
      assertShardCoverage(full, selected);
      const files = selected.flat().map((test) => test.file);
      assert.equal(files.length, 23);
      for (const file of [
        "workspace-flow.spec.ts",
        "workspace-wiki-flow.spec.ts",
        "v050-task-timer.spec.ts",
      ])
        assert.equal(files.filter((path) => path === file).length, 1);
    },
  );
});
test("test_assignment_missing_duplicate_and_split_groups_fail_closed", () => {
  withFixture(pairFiles(["alpha.spec.ts"]), ({ options }) => {
    const full = listTests(options);
    const first = full[0];
    assert(first);
    assert.throws(() => {
      assertShardCoverage(full, [full.slice(1)]);
    }, /missing/);
    assert.throws(() => {
      assertShardCoverage(full, [[...full, first]]);
    }, /duplicate/);
    assert.throws(() => {
      assertShardCoverage(full, [[{ ...first, id: "foreign" }, ...full.slice(1)]]);
    }, /foreign/);
    assert.throws(() => {
      assertShardCoverage(full, [[{ ...first, file: "foreign.spec.ts" }, ...full.slice(1)]]);
    }, /altered/);
  });
});
test("test_duplicate_and_malformed_discovery_fail_closed", () => {
  withFixture(pairFiles(), ({ options }) => {
    const full = listTests(options);
    assert.throws(() => {
      assertRunnableList([...full, ...full]);
    }, /Duplicate/);
    const first = full[0];
    assert(first);
    assert.throws(() => {
      assertShardCoverage(full, [[{ ...first, file: "../escape.spec.ts" }, ...full.slice(1)]]);
    }, /altered/);
  });
});
test("test_missing_wiki_and_invalid_shards_fail_closed → standard shard range", () => {
  withFixture(pairFiles(), ({ options }) => {
    for (const shard of ["1/0", "1/-1", "0/1", "2/1", "broken"])
      assert.throws(() => listTests({ ...options, shard }), /discovery failed/);
  });
});
test("test_workspace_pair_required → dependency setup precedes independently selected wiki", () => {
  // A fixture proves the standard dependency boundary. The product's actual
  // workspace setup must be extracted in B before the pair guard is retired.
  withFixture(
    {
      "workspace.setup.ts":
        'import { test } from "@playwright/test"; import { writeFileSync } from "node:fs"; test("workspace setup", () => { writeFileSync(process.env.FVOCI_GROUP_FIXTURE_DIR + "/ready", "owned"); });',
      "workspace-wiki-flow.spec.ts":
        'import { test, expect } from "@playwright/test"; import { readFileSync } from "node:fs"; test("wiki", () => { expect(readFileSync(process.env.FVOCI_GROUP_FIXTURE_DIR + "/ready", "utf8")).toBe("owned"); });',
    },
    ({ directory, options }) => {
      const config = resolve(directory, "dependency.config.ts");
      writeFileSync(
        config,
        'import { defineConfig } from "@playwright/test"; export default defineConfig({ testDir: ".", outputDir: "./.playwright-output", workers: 1, retries: 0, projects: [{ name: "setup", testMatch: "workspace.setup.ts" }, { name: "wiki", testMatch: "workspace-wiki-flow.spec.ts", dependencies: ["setup"] }] });',
      );
      const result = spawnSync(
        process.execPath,
        ["--bun", cli, "test", "--config", config, "--project=wiki", "--reporter=" + reporter],
        {
          cwd: repositoryRoot,
          env: { ...process.env, ...options.env },
          encoding: "utf8",
        },
      );
      assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.match(result.stdout, /selected=2 completed=2/);
      assert.equal(readFileSync(resolve(directory, "ready"), "utf8"), "owned");
    },
  );
});
test("test_new_spec_auto_included", () => {
  withFixture(pairFiles(), ({ directory, options }) => {
    const before = listTests(options);
    writeFileSync(resolve(directory, "brand-new-flow.spec.ts"), fixtureTest);
    const after = listTests(options);
    assert.equal(after.length, before.length + 1);
    assert(after.some((test) => test.file === "brand-new-flow.spec.ts"));
  });
});
test("test_nested_spec_fails_closed → recursive discovery retains nested spec", () => {
  withFixture(pairFiles(["nested/hidden-flow.spec.ts"]), ({ options }) => {
    assert(listTests(options).some((test) => test.file === "nested/hidden-flow.spec.ts"));
  });
});
test("test_unsupported_test_suffix → standard testMatch includes test.ts", () => {
  withFixture(pairFiles(["collab-wire.test.ts"]), ({ options }) => {
    assert(listTests(options).some((test) => test.file === "collab-wire.test.ts"));
  });
});
test("test_empty_shard_fails_verify", () => {
  withFixture(pairFiles(), ({ options }) => {
    assert.throws(() => listTests({ ...options, shard: "8/8" }), /no tests/);
  });
});
test("test_shard_jsonl_pair_on_one_line → no JSONL execution interface", () => {
  withFixture(pairFiles(), ({ options }) => {
    const full = listTests(options);
    assert.deepEqual(
      full.map((test) => test.file),
      ["workspace-flow.spec.ts", "workspace-wiki-flow.spec.ts"],
    );
    assertShardCoverage(full, [full]);
  });
});
test("test_unsafe_basename_rejected → argv preserves spaced paths", () => {
  withFixture(pairFiles(["bad spec.spec.ts"]), ({ options }) => {
    const selected = listTests({ ...options, selection: ["bad spec.spec.ts"] });
    assert.equal(selected.length, 1);
    assert.equal(selected[0]?.file, "bad spec.spec.ts");
  });
});
test("test_representative_module_suffixes_fail_closed → standard discovery includes modules", () => {
  withFixture(
    pairFiles([
      "extra-flow.spec.mts",
      "extra-flow.test.mjs",
      "extra-flow.spec.cts",
      "extra-flow.test.cjs",
    ]),
    ({ options }) => {
      assert.equal(listTests(options).length, 6);
    },
  );
});
test("test_all_default_discoverable_suffixes_fail_closed → every standard suffix discovered", () => {
  const files = pairFiles();
  for (const kind of ["spec", "test"])
    for (const prefix of ["", "c", "m"])
      for (const language of ["j", "t"])
        for (const x of ["", "x"])
          files["extra-flow." + kind + "." + prefix + language + "s" + x] = fixtureTest;
  withFixture(files, ({ options }) => {
    const full = listTests(options);
    assert.equal(full.length, 26);
    assert.deepEqual(full.map((test) => test.file).sort(), Object.keys(files).sort());
    assertShardCoverage(full, shards(options, 8));
  });
});
test("test_new_spec_ts_does_not_hide_sibling_mts", () => {
  withFixture(pairFiles(["brand-new-flow.spec.ts", "brand-new-flow.spec.mts"]), ({ options }) => {
    assert.equal(
      listTests(options).filter((test) => test.file.startsWith("brand-new-flow.")).length,
      2,
    );
  });
});
test("timer four fresh-run selections retain 9/7/1/5 and exact coverage", () => {
  const titles = [
    ...Array.from({ length: 9 }, (_, i) => "ordinary " + String(i)),
    "real browser offline start",
    "a native committed pause",
    "a planner A-B-A",
    "an estimate A-B-A",
    "a genuine new session retires",
    "transient browser 429",
    "one ordinary task restore",
    "native same-database restart",
    "ordinary research plan persists",
    "task widget retires",
    "a late task-widget R1",
    "owner releases opaque legacy reservations",
    "a late legacy release",
  ];
  withFixture(
    {
      "v050-task-timer.spec.ts":
        'import { test } from "@playwright/test";\n' +
        titles.map((title) => "test(" + JSON.stringify(title) + ", () => {});").join("\n"),
    },
    ({ options }) => {
      const full = listTests(options);
      const runs = timerRuns.map((run) =>
        listTests({ ...options, selection: ["v050-task-timer.spec.ts", ...run.args] }),
      );
      assertTimerCoverage(full, runs);
      assert.throws(() => {
        assertTimerCoverage(full, runs.slice(1));
      }, /four fresh/);
      assert.throws(() => {
        assertTimerCoverage(full, [runs[0] ?? [], runs[1] ?? [], runs[2] ?? [], runs[0] ?? []]);
      }, /four fresh/);
    },
  );
});
test("runtime reporter rejects selected failure, skip, setup failure and zero tests", () => {
  for (const [body, expected] of [
    ["() => {}", 0],
    ['() => { throw new Error("private-error-token"); }', 1],
    ["() => { test.skip(); }", 1],
  ] as const) {
    withFixture(
      {
        "selected.spec.ts":
          'import { test } from "@playwright/test"; test("private-title-token", ' + body + ");",
      },
      ({ options }) => {
        const result = spawnSync(
          process.execPath,
          ["--bun", cli, "test", "--config", options.config, "--reporter=" + reporter],
          { cwd: repositoryRoot, env: { ...process.env, ...options.env }, encoding: "utf8" },
        );
        assert.equal(result.status, expected, result.stdout + result.stderr);
        assert(!result.stdout.includes("private-title-token"));
        assert(!result.stdout.includes("private-error-token"));
        assert(!result.stdout.includes(repositoryRoot));
      },
    );
  }
  withFixture(
    {
      "selected.spec.ts":
        'import { test } from "@playwright/test"; test.beforeAll(() => { throw new Error("private-setup"); }); test("first", () => {}); test("second", () => {});',
    },
    ({ options }) => {
      const result = spawnSync(
        process.execPath,
        ["--bun", cli, "test", "--config", options.config, "--reporter=" + reporter],
        { cwd: repositoryRoot, env: { ...process.env, ...options.env }, encoding: "utf8" },
      );
      assert.equal(result.status, 1);
      assert.match(result.stdout, /status=failed/);
    },
  );
  withFixture({}, ({ options }) => {
    const result = spawnSync(
      process.execPath,
      ["--bun", cli, "test", "--config", options.config, "--reporter=" + reporter],
      { cwd: repositoryRoot, env: { ...process.env, ...options.env }, encoding: "utf8" },
    );
    assert.equal(result.status, 1);
  });
});
