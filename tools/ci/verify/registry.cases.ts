// Workflow mutations shared by the registry tests. Each case edits one real
// workflow text and names a message the verifier must report for it.

export type Edit = (text: string) => string;
export type MutationCase = { name: string; file: string; edit: Edit; needle: string };

export function replaceOnce(text: string, old: string, replacement: string): string {
  const at = text.indexOf(old);
  if (at < 0) throw new Error(`mutation marker not found: ${JSON.stringify(old)}`);
  return text.slice(0, at) + replacement + text.slice(at + old.length);
}

const swap =
  (old: string, replacement: string): Edit =>
  (text) =>
    replaceOnce(text, old, replacement);

function within(start: string, end: string | null, edit: Edit): Edit {
  return (text) => {
    const from = text.indexOf(start);
    if (from < 0) throw new Error(`section marker not found: ${start}`);
    const to = end === null ? text.length : text.indexOf(end, from + start.length);
    if (to < 0) throw new Error(`section end not found: ${String(end)}`);
    return text.slice(0, from) + edit(text.slice(from, to)) + text.slice(to);
  };
}

const GATED = ["rust", "web", "install", "documents", "collab-engine"] as const;
const PHASES =
  "options: [connection, crud, transactions, migration, inventory, reset, persistence, restore, ui-ack, ui-baseline]";
const TARGET_INIT = `          printf 'CARGO_TARGET_DIR=%s/turso-target\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n`;
const UNIT =
  "      - name: Credential-free frozen diagnostic unit (exactly one test)\n" +
  "        run: python3 scripts/selected-backend-ci/turso-test-guard.py --diagnostic-unit\n";
const UNIT_NEEDLE = "credential-free exact frozen diagnostic unit before secret consumption";
const CONSUME = "        run: python3 scripts/selected-backend-ci/turso-test-guard.py --consume\n";
const WEB_GATE =
  '          python3 scripts/ci_selection.py gate --workflow web --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n';
const POSTGRES_MATRIX_LINE =
  "      matrix: ${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}\n";

function caseList(): MutationCase[] {
  const cases: MutationCase[] = [];
  const add = (name: string, file: string, edit: Edit, needle: string): void => {
    cases.push({ name, file, edit, needle });
  };

  // Gate wiring (RegistryMutationCliTest).
  add(
    "pr-path-filter",
    "web.yml",
    swap("  pull_request:\n", "  pull_request:\n    paths: ['apps/web/**']\n"),
    "pull_request must be unfiltered",
  );
  for (const override of ["ref: attacker-head", "repository: attacker/repo", "fetch-depth: 1"]) {
    add(
      `checkout-${override}`,
      "web.yml",
      swap("          fetch-depth: 0", "          " + override),
      "ci-plan must checkout the event merge",
    );
  }
  add(
    "runner-sha-override",
    "web.yml",
    swap(
      "          GITHUB_EVENT_NAME:",
      "          GITHUB_SHA: attacker-head\n          GITHUB_EVENT_NAME:",
    ),
    "must not override trusted GITHUB_SHA",
  );
  add(
    "new-job",
    "web.yml",
    swap(
      "  web-ci-gate:",
      "\n  new-suite:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_new_suite == 'true'\n    runs-on: ubuntu-26.04\n    steps:\n      - run: echo new\n  web-ci-gate:",
    ),
    "unregistered job id new-suite",
  );
  add(
    "gate-suffixed-job",
    "web.yml",
    swap(
      "  web-ci-gate:",
      "\n  sneaky-ci-gate:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_sneaky_ci_gate == 'true'\n    runs-on: ubuntu-26.04\n    steps:\n      - run: echo sneaky\n  web-ci-gate:",
    ),
    "unregistered job id sneaky-ci-gate",
  );
  const install: [string, string, string][] = [
    ["        default: false\n", "        default: true\n", "input run_upgrade_smoke_arm must be"],
    ["        type: boolean\n", "        type: string\n", "input run_upgrade_smoke_arm must be"],
    [
      "      run_upgrade_smoke_arm:\n",
      "      upgrade_old:\n        type: string\n      run_upgrade_smoke_arm:\n",
      "workflow_dispatch inputs must be exactly",
    ],
    [
      "  upgrade-smoke-arm64:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n    runs-on: ubuntu-26.04-arm\n",
      "  upgrade-smoke-arm64:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n    runs-on: ubuntu-26.04\n",
      "upgrade-smoke-arm64 runs-on must be ubuntu-26.04-arm",
    ],
    [
      "    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n",
      "    if: github.event_name == 'workflow_dispatch'\n",
      "upgrade-smoke-arm64 if must be",
    ],
    [
      "    needs: [ci-plan, install-smoke, backup-restore-smoke, upgrade-smoke-arm64]\n",
      "    needs: [ci-plan, install-smoke, backup-restore-smoke]\n",
      "install-ci-gate needs must be",
    ],
    [
      "      select_upgrade_smoke_arm64: ${{ steps.plan.outputs.select_upgrade_smoke_arm64 }}\n",
      "",
      "missing selector output select_upgrade_smoke_arm64",
    ],
  ];
  install.forEach(([old, replacement, needle], index) => {
    add(`install-opt-in-${String(index)}`, "install.yml", swap(old, replacement), needle);
  });
  add(
    "opt-in-on-other-workflow",
    "web.yml",
    swap(
      "  workflow_dispatch:\n",
      "  workflow_dispatch:\n    inputs:\n      run_upgrade_smoke_arm:\n        type: boolean\n        default: false\n",
    ),
    "web: workflow_dispatch inputs must be exactly []",
  );
  add(
    "missing-selector-output",
    "web.yml",
    swap("      select_web_checks: ${{ steps.plan.outputs.select_web_checks }}\n", ""),
    "missing selector output select_web_checks",
  );
  add(
    "gate-needs-mismatch",
    "web.yml",
    within("  web-ci-gate:", null, (gate) =>
      gate.replace(/ {4}needs:\n {6}\[[^\]]*\]/, "    needs: [ci-plan, web-checks]"),
    ),
    "needs must be ci-plan and every registered job",
  );
  add(
    "swapped-needs-json",
    "rust.yml",
    swap("NEEDS_JSON: ${{ toJSON(needs) }}", "NEEDS_JSON: ${{ toJSON(needs.postgres) }}"),
    "env must be exactly",
  );
  add(
    "forged-needs-json",
    "rust.yml",
    swap("NEEDS_JSON: ${{ toJSON(needs) }}", 'NEEDS_JSON: \'{"fast":{"result":"success"}}\''),
    "env must be exactly",
  );
  add(
    "swapped-tested-sha",
    "rust.yml",
    swap("TESTED_SHA: ${{ github.sha }}", "TESTED_SHA: ${{ needs.fast.result }}"),
    "env must be exactly",
  );
  add(
    "extra-gate-env",
    "rust.yml",
    swap(
      "          TESTED_SHA: ${{ github.sha }}\n",
      "          TESTED_SHA: ${{ github.sha }}\n          JOB_FAST: ${{ needs.postgres.result }}\n",
    ),
    "env must be exactly",
  );
  add(
    "decoy-echo-gate",
    "web.yml",
    swap(WEB_GATE, "          echo " + WEB_GATE.trimStart()),
    "canonical gate invocation",
  );
  add(
    "commented-gate",
    "web.yml",
    swap(WEB_GATE, "          # " + WEB_GATE.trimStart() + "          true\n"),
    "canonical gate invocation",
  );
  add(
    "rust-missing-selector-wrapper",
    "rust.yml",
    swap("          bash scripts/test-ci-selection.sh\n", ""),
    "must run scripts/test-ci-selection.sh",
  );
  add(
    "web-duplicate-selector-wrapper",
    "web.yml",
    swap(
      "          python3 scripts/ci_selection.py plan \\\n",
      "          bash scripts/test-ci-selection.sh\n          python3 scripts/ci_selection.py plan \\\n",
    ),
    "must not duplicate scripts/test-ci-selection.sh",
  );
  for (const workflow of GATED) {
    const file = `${workflow}.yml`;
    add(
      `${workflow}-event-gate-name`,
      file,
      swap(`    name: ${workflow}-ci-gate\n`, "    name: ${{ github.event_name }}-ci-gate\n"),
      `${workflow}-ci-gate name must stay ${workflow}-ci-gate`,
    );
    add(
      `${workflow}-drop-merge-group`,
      file,
      swap("  merge_group:\n    types: [checks_requested]\n", ""),
      "merge_group trigger is required",
    );
    add(
      `${workflow}-filter-merge-group`,
      file,
      swap("types: [checks_requested]", "types: [destroyed]"),
      "merge_group must request checks_requested",
    );
  }

  // Runner labels and cache qualification.
  for (const file of [
    ...GATED.map((workflow) => `${workflow}.yml`),
    "release.yml",
    "turso-test.yml",
  ]) {
    for (const label of ["ubuntu-24.04", "ubuntu-22.04", "ubuntu-latest"]) {
      add(
        `${file}-runner-${label}`,
        file,
        swap("runs-on: ubuntu-26.04", "runs-on: " + label),
        "requires explicit Ubuntu 26.04 runners",
      );
    }
  }
  add(
    "old-os-cache-prefix",
    "collab-engine.yml",
    swap(
      "restore-keys: v1-collab-engine-ubuntu-26.04-",
      "restore-keys: v1-collab-engine-ubuntu-24.04-",
    ),
    "cache restore-keys must bind Ubuntu 26.04",
  );
  for (const family of ["restore", "save"]) {
    add(
      `separate-cache-family-${family}`,
      "rust.yml",
      within("      - name: Restore Cargo downloads\n", "      - name:", (step) =>
        step
          .replace("uses: actions/cache@", `uses: actions/cache/${family}@`)
          .replace(/key: v2-cargo-server-[^\n]*/, "key: unqualified-downloads"),
      ),
      "cache key must bind Ubuntu 26.04, architecture and toolchain",
    );
  }

  // Release and image token scopes.
  add(
    "release-pull-request",
    "release.yml",
    swap("  workflow_dispatch:", "  pull_request:\n  workflow_dispatch:"),
    "release.yml: triggers must be exactly push (tags) and workflow_dispatch",
  );
  add(
    "release-verify-writes",
    "release.yml",
    swap(
      "      contents: read\n      checks: read\n",
      "      contents: write\n      checks: read\n",
    ),
    "release.yml: verify may not write ['contents']",
  );
  add(
    "release-per-tag-concurrency",
    "release.yml",
    swap("  group: release-ghcr-fvoci\n", "  group: release-${{ github.ref_name }}\n"),
    "release.yml: concurrency must be one fixed group with cancel-in-progress: false",
  );
  add(
    "release-publish-writes-contents",
    "release.yml",
    within(
      "  publish:\n    needs: [verify, index, smoke]\n",
      null,
      swap("      packages: write\n", "      contents: write\n      packages: write\n"),
    ),
    "release.yml: publish may not write ['contents']",
  );
  add(
    "release-branch-push",
    "release.yml",
    swap("    tags:", "    branches: [main]\n    tags:"),
    "release.yml: push must list only v0.* tags",
  );
  for (const [job, scope] of [
    ["build", "packages"],
    ["push", "issues"],
    ["push", "contents"],
    ["push-manifest", "contents"],
    ["push-manifest", "issues"],
  ] as const) {
    add(
      `ci-base-${job}-${scope}`,
      "ci-base-image.yml",
      (text) => {
        const jobAt = text.indexOf(`  ${job}:\n`, text.indexOf("jobs:\n"));
        if (job === "build") {
          const at = jobAt + `  ${job}:\n`.length;
          return text.slice(0, at) + `    permissions:\n      ${scope}: write\n` + text.slice(at);
        }
        const at = text.indexOf("    permissions:\n", jobAt) + "    permissions:\n".length;
        let rest = text.slice(at);
        if (scope === "contents") rest = rest.replace("      contents: read\n", "");
        return text.slice(0, at) + `      ${scope}: write\n` + rest;
      },
      `ci-base-image.yml: ${job} may not write ['${scope}']`,
    );
  }

  // Turso manual workflow.
  const turso: [string, string, string][] = [
    ["  workflow_dispatch:\n", "  pull_request_target:\n", "manual dispatch only"],
    ["      destructive:\n", "      checkout_sha:\n", "fixed phase inputs"],
    ["default: connection", "default: migration", "fixed phase inputs"],
    ["default: false", "default: true", "fixed phase inputs"],
    [
      "Connection, inventory and ui-baseline read-only; migration, reset and ui-ack require both destructive gates; others NOT IMPLEMENTED",
      "All phases implemented",
      "fixed phase inputs",
    ],
    ["  contents: read\n", "  contents: write\n", "contents read only"],
    ["  cancel-in-progress: false\n", "  cancel-in-progress: true\n", "fixed database concurrency"],
    [
      "github.repository == 'AISFlow/fvoci'",
      "github.repository == 'attacker/fvoci'",
      "trusted admission",
    ],
    [
      "ref: ${{ github.sha }}",
      "ref: ${{ inputs.checkout_sha }}",
      "pre-Environment admission steps",
    ],
    ["persist-credentials: false", "persist-credentials: true", "pre-Environment admission steps"],
    ["    needs: admission\n", "    needs: []\n", "runtime needs successful trusted admission"],
    ["    environment: fvoci-turso-test\n", "    environment: production\n", "fixed Environment"],
    [" --admit\n", " --consume\n", "pre-Environment admission steps"],
    ["--no-run --message-format=json", "--message-format=json", "fixed fresh compilation"],
    [
      "      CARGO_INCREMENTAL: 0\n",
      "      TOKEN: ${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}\n",
      "credential-free compiler environment",
    ],
    [" --consume\n", " --consume || true\n", "only one sanitized runtime step"],
    [
      "branches: [fvoci/v060-turso-verified-connection]",
      "branches: ['*']",
      "fixed credential-free bootstrap",
    ],
    [
      "    if: github.event_name == 'workflow_dispatch'",
      "    if: github.event_name == 'push'",
      "runtime needs successful trusted admission",
    ],
    [
      "      CARGO_INCREMENTAL: 0\n",
      "      CARGO_INCREMENTAL: 0\n      CARGO_TARGET_DIR: ${{ runner.temp }}/turso-target\n",
      "credential-free compiler environment",
    ],
    [TARGET_INIT, "", "maintained pinned compiler/native preparation"],
    [
      TARGET_INIT,
      TARGET_INIT.replace("$RUNNER_TEMP", "/foreign"),
      "maintained pinned compiler/native preparation",
    ],
    [
      TARGET_INIT + "          rustup toolchain install 1.98.1 --profile minimal\n",
      "          rustup toolchain install 1.98.1 --profile minimal\n" + TARGET_INIT,
      "maintained pinned compiler/native preparation",
    ],
  ];
  turso.forEach(([old, replacement, needle], index) => {
    add(`turso-boundary-${String(index)}`, "turso-test.yml", swap(old, replacement), needle);
  });

  const bunCache = `          printf 'BUN_INSTALL_CACHE_DIR=%s/turso-bun-cache\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n`;
  const browserCache = `          printf 'PLAYWRIGHT_BROWSERS_PATH=%s/turso-browsers\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n`;
  const ui: [string, string][] = [
    ["    needs: admission\n", "    needs: []\n"],
    ["github.event.inputs.phase == 'ui-ack'", "github.event.inputs.phase != 'ui-ack'"],
    ["github.repository == 'AISFlow/fvoci'", "github.repository == 'attacker/fvoci'"],
    ["github.ref == 'refs/heads/main'", "startsWith(github.ref, 'refs/heads/')"],
    ["    environment: fvoci-turso-test\n", "    environment: production\n"],
    ["    timeout-minutes: 40\n", "    timeout-minutes: 90\n"],
    ["      FVOCI_BUILD_SHA: ${{ github.sha }}\n", "      FVOCI_BUILD_SHA: stale9202\n"],
    ["      CARGO_BUILD_JOBS: 2\n", "      CARGO_BUILD_JOBS: 12\n"],
    ["      CARGO_INCREMENTAL: 0\n", "      TOKEN: ${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}\n"],
    [
      "      PYTHONDONTWRITEBYTECODE: '1'\n",
      "      BUN_INSTALL_CACHE_DIR: ${{ runner.temp }}/turso-bun-cache\n      PYTHONDONTWRITEBYTECODE: '1'\n",
    ],
    [
      "      PYTHONDONTWRITEBYTECODE: '1'\n",
      "      PLAYWRIGHT_BROWSERS_PATH: ${{ runner.temp }}/turso-browsers\n      PYTHONDONTWRITEBYTECODE: '1'\n",
    ],
    [bunCache, ""],
    [browserCache, ""],
    [bunCache, bunCache.replace("$RUNNER_TEMP", "/foreign")],
    [browserCache, browserCache.replace("$GITHUB_ENV", "$GITHUB_OUTPUT")],
    ["          ref: ${{ github.sha }}\n", "          ref: main\n"],
    ["          persist-credentials: false\n", "          persist-credentials: true\n"],
    ["          cargo fetch --locked\n", "          cargo fetch\n"],
    ["          cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml\n", ""],
    [
      "          cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml\n",
      "          cargo fetch --manifest-path crates/collab-engine/Cargo.toml\n",
    ],
    [
      "          cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml\n",
      "          cargo fetch --locked --manifest-path Cargo.toml\n",
    ],
    ["          bun-version: 1.4.2\n", "          bun-version: latest\n"],
    ["          bun install --frozen-lockfile\n", "          bun install\n"],
    ["          python3 scripts/selected-backend-ci/turso-ui.py --record-before\n", ""],
    [
      "          bun --bun run --cwd apps/web build\n",
      "          cp -r /stale9202/dist apps/web/dist\n",
    ],
    ["          bun --bun run --cwd apps/web build\n", "          bun run --cwd apps/web build\n"],
    ["--features api-schema,db-tests --jobs 2", "--features db-tests --jobs 2"],
    ["--features worker --jobs 2", "--jobs 2"],
    ["          python3 scripts/selected-backend-ci/turso-ui.py --freeze\n", ""],
    [
      "      - name: Actual current primary UI baseline or guarded ON restart OFF consumer\n",
      "      - name: Actual current primary UI baseline or guarded ON restart OFF consumer\n        if: false\n",
    ],
    ["${{ vars.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE }}", "true"],
    ["${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}", "${{ secrets.PRODUCTION_TOKEN }}"],
    [CONSUME, "        run: python3 scripts/selected-backend-ci/turso-ui.py --actor\n"],
  ];
  ui.forEach(([old, replacement], index) => {
    add(
      `turso-ui-${String(index)}`,
      "turso-test.yml",
      within("  turso-ui:\n", null, swap(old, replacement)),
      "fixed private Turso UI job and current source baseline consumer",
    );
  });
  add(
    "turso-ui-extra-step",
    "turso-test.yml",
    (text) => text + "    extra-step: {uses: actions/upload-artifact@v4}\n",
    "fixed private Turso UI job and current source baseline consumer",
  );
  const inputs: [string, string][] = [
    ["      ui_source_sha:\n", "      arbitrary_source:\n"],
    ["      ui_baseline_sha256:\n", "      arbitrary_dataset:\n"],
    ["      ui_target_sha256:\n", "      arbitrary_target:\n"],
    [PHASES, PHASES.replace(", ui-baseline", "")],
    [PHASES, PHASES.replace(", inventory", "")],
    [PHASES, PHASES.replace("inventory", "unapproved-phase")],
    [PHASES, PHASES.replace("inventory", "inventory, unapproved-phase")],
    [PHASES, PHASES.replace("inventory", "inventory, inventory")],
    [PHASES, PHASES.replace("migration, inventory", "inventory, migration")],
    [PHASES, PHASES.replace(", reset", "")],
    [PHASES, PHASES.replace("reset", "unapproved-phase")],
    [PHASES, PHASES.replace("reset", "reset, reset")],
    [PHASES, PHASES.replace("reset", "reset, unapproved-phase")],
  ];
  inputs.forEach(([old, replacement], index) => {
    add(
      `turso-inputs-${String(index)}`,
      "turso-test.yml",
      swap(old, replacement),
      "turso-test.yml: fixed phase inputs and non-destructive default",
    );
  });
  const unit: [string, string][] = [
    ["", "fixed credential-free build then single consuming step"],
    [UNIT + UNIT, "fixed credential-free build then single consuming step"],
    [UNIT.replace("--diagnostic-unit", "--consume"), UNIT_NEEDLE],
    [UNIT.replace("--diagnostic-unit", "--diagnostic-unit --ignored"), UNIT_NEEDLE],
    [UNIT.replace("--diagnostic-unit", "--diagnostic-unit || true"), UNIT_NEEDLE],
    [UNIT.replace("frozen diagnostic unit", "arbitrary unit"), UNIT_NEEDLE],
    [
      UNIT.replace(
        "        run:",
        "        env:\n          TOKEN: ${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}\n        run:",
      ),
      UNIT_NEEDLE,
    ],
    [
      UNIT.replace("        run:", "        env:\n          EXTRA: value\n        run:"),
      UNIT_NEEDLE,
    ],
    [UNIT.replace("        run:", "        if: false\n        run:"), UNIT_NEEDLE],
  ];
  unit.forEach(([replacement, needle], index) => {
    add(`turso-unit-${String(index)}`, "turso-test.yml", swap(UNIT, replacement), needle);
  });
  add(
    "turso-unit-before-compile",
    "turso-test.yml",
    (text) => {
      const marker = "      - name: Credential-free current library test compilation\n";
      return replaceOnce(replaceOnce(text, UNIT, ""), marker, UNIT + marker);
    },
    UNIT_NEEDLE,
  );
  add(
    "turso-unit-after-secret",
    "turso-test.yml",
    (text) => replaceOnce(replaceOnce(text, UNIT, ""), CONSUME, CONSUME + UNIT),
    UNIT_NEEDLE,
  );
  add(
    "turso-target-init-moved",
    "turso-test.yml",
    (text) =>
      replaceOnce(
        replaceOnce(text, TARGET_INIT, ""),
        "          cargo test --locked",
        TARGET_INIT + "          cargo test --locked",
      ),
    "maintained pinned compiler/native preparation",
  );

  // Unfiltered gate triggers, SHA pins, permission pins, check names, postgres matrix.
  add(
    "turso-top-level-defaults",
    "turso-test.yml",
    swap("\njobs:\n", "\ndefaults:\n  run:\n    shell: bash -c 'env; bash {0}'\njobs:\n"),
    "turso-test.yml: no top-level defaults, run-name or other workflow keys",
  );
  add(
    "rust-push-paths",
    "rust.yml",
    swap(
      "  push:\n    branches: [main]\n",
      "  push:\n    branches: [main]\n    paths: ['src/**']\n",
    ),
    "rust.yml: push must not use paths",
  );
  add(
    "documents-push-paths-ignore",
    "documents.yml",
    swap(
      "  push:\n    branches: [main]\n",
      "  push:\n    branches: [main]\n    paths-ignore: ['docs/**']\n",
    ),
    "documents.yml: push must not use paths-ignore",
  );
  add(
    "unpinned-uses",
    "documents.yml",
    swap(
      "uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
      "uses: actions/checkout@v4",
    ),
    "must pin a full commit SHA",
  );
  add(
    "short-sha-uses",
    "release.yml",
    swap(
      "uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
      "uses: actions/checkout@11d5960",
    ),
    "must pin a full commit SHA",
  );
  add(
    "local-uses",
    "install.yml",
    swap(
      "uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
      "uses: ./.github/actions/checkout",
    ),
    "must pin a full commit SHA",
  );
  add(
    "top-permissions-widened",
    "web.yml",
    swap(
      "permissions:\n  contents: read\n",
      "permissions:\n  contents: read\n  pull-requests: read\n",
    ),
    "web.yml: top-level permissions changed from the pinned value",
  );
  add(
    "release-verify-permission-dropped",
    "release.yml",
    swap("      contents: read\n      checks: read\n", "      contents: read\n"),
    "release.yml: verify permissions changed from the pinned value",
  );
  add(
    "required-check-renamed",
    "documents.yml",
    swap("  documents-ci-gate:\n", "  documents-gate:\n"),
    "documents.yml: required check documents-ci-gate missing",
  );
  const flow =
    '[{"runner":"ubuntu-26.04","pg_major":"18","shard":"a","tests":"--test db_integration"}]';
  for (const [name, line] of [
    ["static-include", `      matrix: {"include": ${flow}, "exclude": []}\n`],
    ["plain-static-include", `      matrix: {"include": ${flow}}\n`],
    ["exclude-output", "      matrix: ${{ fromJSON(needs.ci-plan.outputs.postgres_exclude) }}\n"],
    [
      "array-fallback",
      "      matrix: ${{ fromJSON(needs.ci-plan.outputs.postgres_matrix || '[]') }}\n",
    ],
  ] as const) {
    add(
      `postgres-matrix-${name}`,
      "rust.yml",
      swap(POSTGRES_MATRIX_LINE, line),
      "rust: postgres strategy must be exactly",
    );
  }
  return cases;
}

export const MUTATION_CASES: readonly MutationCase[] = caseList();
