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

  // Release steps run the tools/release Bun entries, never the Python originals.
  const RELEASE_API = "bun tools/release/release-api.ts ";
  const PROVENANCE = 'bun tooling/tools/release/provenance.ts --dist "$RUNNER_TEMP/dist"';
  const RELEASE_SETUP_BUN =
    / {6}- uses: oven-sh\/setup-bun@[^\n]*\n {8}with:\n {10}bun-version-file: [^\n]*\n/;
  const releaseRuntime: [string, Edit, string][] = [
    [
      "push-index-python",
      swap(RELEASE_API + "push-index", "python3 scripts/release-api.py push-index"),
      "release.yml: index runs Python",
    ],
    [
      "describe-python",
      swap(RELEASE_API + "describe", "python3 scripts/release-api.py describe"),
      "release.yml: index must run 'bun tools/release/release-api.ts describe",
    ],
    [
      "tag-python",
      swap(RELEASE_API + "tag", "python3 scripts/release-api.py tag"),
      "release.yml: publish runs Python",
    ],
    [
      "floating-dropped",
      swap('--tag "$MINOR" --floating\n', '--tag "$MINOR"\n'),
      "release.yml: publish must run",
    ],
    [
      "provenance-python",
      swap(PROVENANCE, 'python3 tooling/scripts/release-provenance.py --dist "$RUNNER_TEMP/dist"'),
      "release.yml: dist runs Python",
    ],
    [
      "provenance-from-tag-tree",
      swap(PROVENANCE, 'bun tools/release/provenance.ts --dist "$RUNNER_TEMP/dist"'),
      "release.yml: dist must run 'bun tooling/tools/release/provenance.ts",
    ],
    [
      "check-ci-python",
      swap(
        'run: bash scripts/release-check-ci.sh "${{ steps.source.outputs.sha }}"',
        'run: python3 scripts/release-check-ci.py "${{ steps.source.outputs.sha }}"',
      ),
      "release.yml: verify runs Python",
    ],
    [
      "bun-version-unpinned",
      swap("          bun-version-file: .bun-version\n", "          bun-version: latest\n"),
      "release.yml: verify must set up Bun",
    ],
  ];
  releaseRuntime.push(
    [
      "dist-bun-from-tag-tree",
      swap("bun-version-file: tooling/.bun-version\n", "bun-version-file: .bun-version\n"),
      "release.yml: dist must set up Bun (oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6, bun-version-file tooling/.bun-version)",
    ],
    [
      "setup-python",
      within(
        "\n  publish:\n",
        null,
        swap(
          "      - name: Tag the smoked index",
          "      - uses: actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065\n      - name: Tag the smoked index",
        ),
      ),
      "release.yml: publish runs Python",
    ],
    [
      "python-shell",
      swap(
        "      - name: Tag the smoked index 0.y.z (immutable)\n",
        "      - name: Tag the smoked index 0.y.z (immutable)\n        shell: python\n",
      ),
      "release.yml: publish runs Python",
    ],
  );
  for (const job of ["index", "publish"]) {
    add(
      `release-${job}-tooling-from-tag`,
      "release.yml",
      within(
        `\n  ${job}:\n`,
        null,
        swap(
          "          ref: ${{ github.sha }}\n",
          "          ref: ${{ needs.verify.outputs.sha }}\n",
        ),
      ),
      `release.yml: ${job} must check out only the workflow ref`,
    );
  }
  for (const [name, edit, needle] of releaseRuntime)
    add(`release-${name}`, "release.yml", edit, needle);
  for (const job of ["verify", "index", "dist", "smoke", "publish", "release"]) {
    add(
      `release-${job}-without-bun`,
      "release.yml",
      within(`\n  ${job}:\n`, null, (text) => text.replace(RELEASE_SETUP_BUN, "")),
      `release.yml: ${job} must set up Bun`,
    );
  }
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
    const scope = job ? JOBS[job] : null;
    add(
      `turso-bun-${name}`,
      "turso-test.yml",
      scope ? within(scope[0], scope[1], edit) : edit,
      needle,
    );
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
  // The documents workflow's Bun native-dependency check (tools/ci/verify/documents.ts).
  const inNative = (edit: Edit) => within("  native-extraction:\n", "  documents-ci-gate:\n", edit);
  const depsLine =
    '          bun ../../tools/ci/document-client-deps.ts "$RUNNER_TEMP/extract-client-metadata.json"\n';
  const metadataLine =
    '          cargo metadata --locked --offline --format-version 1 > "$RUNNER_TEMP/extract-client-metadata.json"\n';
  const thinName = "      - name: Thin process client without native parser dependencies\n";
  const thinDir = "        working-directory: crates/document-extract-client\n";
  const rhwpRestore = "      - name: Restore pinned public rhwp source\n";
  const aptLine =
    "          sudo apt-get install -y --no-install-recommends gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8\n";
  const rustup =
    "      - run: rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy\n";
  const thinNeedle =
    "documents.yml: native-extraction must check the process client's cargo metadata";
  const pythonNeedle = "documents.yml: native-extraction must not run or install Python";
  const bunNeedle =
    "documents.yml: native-extraction must install pinned Bun from .bun-version once";
  const heredoc =
    "          python3 - \"$RUNNER_TEMP/extract-client-metadata.json\" <<'PYTHON'\n" +
    "          import json, sys\n" +
    '          names = {p["name"] for p in json.load(open(sys.argv[1]))["packages"]}\n' +
    '          forbidden = names & {"rhwp", "cfb", "zip", "flate2", "skia-safe", "skia-bindings"}\n' +
    '          assert not forbidden, f"native dependencies leaked into process client: {forbidden}"\n' +
    "          PYTHON\n";
  for (const [name, edit, needle] of [
    ["python-heredoc-restored", swap(depsLine, heredoc), pythonNeedle],
    ["python-heredoc-restored-thin", swap(depsLine, heredoc), thinNeedle],
    ["python-heredoc-beside-bun", swap(depsLine, depsLine + heredoc), pythonNeedle],
    [
      "apt-python3-restored",
      swap(
        aptLine,
        aptLine.replace("no-install-recommends gcc", "no-install-recommends python3 gcc"),
      ),
      pythonNeedle,
    ],
    [
      "setup-python-added",
      swap(rustup, `      - uses: actions/setup-python@${"2".repeat(40)}\n` + rustup),
      pythonNeedle,
    ],
    [
      "job-shell-python",
      swap(
        "        working-directory: crates/document-extract\n    steps:\n",
        "        working-directory: crates/document-extract\n        shell: python3 {0}\n    steps:\n",
      ),
      pythonNeedle,
    ],
    [
      "job-container-python",
      swap(
        "        working-directory: crates/document-extract\n    steps:\n",
        "        working-directory: crates/document-extract\n    container: python:3.13\n    steps:\n",
      ),
      pythonNeedle,
    ],
    ["deps-check-dropped", swap(depsLine, ""), thinNeedle],
    [
      "deps-check-before-metadata",
      swap(metadataLine + depsLine, depsLine + metadataLine),
      thinNeedle,
    ],
    [
      "deps-check-masked",
      swap(depsLine, depsLine.replace('.json"\n', '.json" || true\n')),
      thinNeedle,
    ],
    [
      "deps-check-other-path",
      swap(depsLine, depsLine.replace("../../tools/ci/", "tools/ci/")),
      thinNeedle,
    ],
    ["thin-client-renamed", swap(thinName, thinName.replace("Thin", "Lean")), thinNeedle],
    [
      "thin-client-workdir",
      swap(thinDir, "        working-directory: crates/document-extract\n"),
      thinNeedle,
    ],
    [
      "thin-client-continue-on-error",
      swap(thinDir, thinDir + "        continue-on-error: true\n"),
      thinNeedle,
    ],
    ["thin-client-condition", swap(thinDir, thinDir + "        if: false\n"), thinNeedle],
    ["setup-bun-dropped", inNative(swap(SETUP_BUN, "")), bunNeedle],
    ["setup-bun-twice", inNative(swap(SETUP_BUN, SETUP_BUN + SETUP_BUN)), bunNeedle],
    ["setup-bun-other-pin", inNative(swap(SETUP_BUN_PIN, OTHER_PIN)), bunNeedle],
    [
      "setup-bun-after-check",
      inNative((text) => swap(rhwpRestore, SETUP_BUN + rhwpRestore)(swap(SETUP_BUN, "")(text))),
      bunNeedle,
    ],
    [
      "setup-bun-before-checkout",
      inNative((text) => swap(CHECKOUT, SETUP_BUN + CHECKOUT)(swap(SETUP_BUN, "")(text))),
      bunNeedle,
    ],
  ] as const) {
    add(`documents-${name}`, "documents.yml", edit, needle);
  }
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
  // rust.yml runs the prebuilt xtask and Bun; each former python3 caller is rejected.
  const XTASK = "xtask/target/debug/xtask";
  const OLD = "python3 scripts/ci_selection.py";
  const NOT_PYTHON = "must run xtask or Bun, not Python";
  const FAST_SCRIPT_STEP = "      - name: Script unit tests without a database\n";
  const FAST_SELECTED =
    "          bun test ./tools/selected-backend-ci/lane-controls.test.ts" +
    " ./tools/selected-backend-ci/drivers/binding.test.ts ./tools/selected-backend-ci/drivers/common.test.ts" +
    " ./tools/selected-backend-ci/drivers/restart.test.ts ./tools/selected-backend-ci/drivers/postgres.test.ts" +
    " ./tools/selected-backend-ci/drivers/install.test.ts\n";
  const FAST_PINNED = `rust: fast "Script unit tests without a database" must run exactly the pinned script tests`;
  for (const [name, edit, needle] of [
    [
      "build",
      swap(`${XTASK} rust-binaries build `, `${OLD} rust-binaries build `),
      "binary producer must build the complete registered cohort",
    ],
    [
      "pack",
      swap(`${XTASK} rust-binaries pack `, `${OLD} rust-binaries pack `),
      "binary producer must seal the cohort with the prebuilt xtask",
    ],
    [
      "unpack-postgres",
      swap(
        `${XTASK} rust-binaries unpack --cohort postgres `,
        `${OLD} rust-binaries unpack --cohort postgres `,
      ),
      "binary consumers must validate all inputs and hashes unconditionally",
    ],
    [
      "unpack-helper",
      swap(
        `${XTASK} rust-binaries unpack --cohort helper `,
        `${OLD} rust-binaries unpack --cohort helper `,
      ),
      "binary consumers must validate all inputs and hashes unconditionally",
    ],
    [
      "run",
      swap(`run: ${XTASK} rust-binaries run `, `run: ${OLD} rust-binaries run `),
      "PostgreSQL integration step must execute validated db-tests binaries",
    ],
    [
      "s3",
      swap(
        `start-test-minio.sh ${XTASK} rust-binaries run `,
        `start-test-minio.sh ${OLD} rust-binaries run `,
      ),
      "S3 integration step must invoke start-test-minio.sh",
    ],
    [
      "schema",
      swap(
        `          ${XTASK} schema-baseline\n`,
        "          python3 - <<'PYSCHEMA'\n          PYSCHEMA\n",
      ),
      "schema baseline requires exact configured extraction",
    ],
    [
      "install",
      swap(`sudo ${XTASK} selected-install `, "sudo python3 - "),
      "selected install step must keep exact",
    ],
    [
      "library",
      swap(
        "          xtask/target/debug/xtask selected-library --target-dir target/db-lib\n",
        "          python3 - <<'PYLIB'\n          PYLIB\n",
      ),
      "selected library step must keep exact unconditional command",
    ],
    [
      "encryption",
      swap(
        "      - run: bun tools/oracle/encryption-keys.ts self-test\n",
        "      - run: python3 scripts/encryption_keys.py self-test\n",
      ),
      NOT_PYTHON,
    ],
    [
      "rustup-metadata",
      swap(
        "          bun test scripts/schema-baseline/compare-catalogs.test.ts\n",
        "          bun test scripts/schema-baseline/compare-catalogs.test.ts\n          python3 scripts/fixtures/rustup-ci/test_metadata.py\n",
      ),
      NOT_PYTHON,
    ],
    [
      "fast-apt",
      within(
        "  fast:\n",
        "  native-arm64:\n",
        swap("--no-install-recommends gcc ", "--no-install-recommends python3 gcc "),
      ),
      "rust: fast " + NOT_PYTHON,
    ],
    [
      "job-default-shell",
      swap(
        "  native-arm64:\n    needs: ci-plan\n",
        "  native-arm64:\n    needs: ci-plan\n    defaults:\n      run:\n        shell: python {0}\n",
      ),
      "rust: native-arm64 " + NOT_PYTHON,
    ],
    [
      "setup-python",
      within(
        "  collaboration:\n",
        "  rust-ci-gate:\n",
        swap(
          "      - run: cargo fetch --locked\n",
          "      - uses: actions/setup-python@v5\n      - run: cargo fetch --locked\n",
        ),
      ),
      "rust: collaboration " + NOT_PYTHON,
    ],
    [
      "xtask-before-prepare",
      within(
        "  collaboration:\n",
        "  rust-ci-gate:\n",
        swap(
          "      - name: Prepare pinned SQLite root build inputs\n",
          "      - run: xtask/target/debug/xtask --help\n      - name: Prepare pinned SQLite root build inputs\n",
        ),
      ),
      "rust: collaboration SQLite prefix verification must precede",
    ],
    [
      "postgres-apt",
      within(
        "  postgres:\n",
        "  collaboration:\n",
        swap("--no-install-recommends gcc ", "--no-install-recommends python3 gcc "),
      ),
      "rust: postgres " + NOT_PYTHON,
    ],
    [
      "shell",
      swap(
        "        run: bash scripts/run-rust-collaboration-ci-tests.sh\n",
        "        shell: python\n        run: bash scripts/run-rust-collaboration-ci-tests.sh\n",
      ),
      "rust: collaboration " + NOT_PYTHON,
    ],
    [
      "dpkg-identity",
      within(
        "  postgres-build:\n",
        "  postgres:\n",
        swap("libclang-18-dev curl >", "libclang-18-dev python3 curl >"),
      ),
      "postgres-build SQLite prefix cache must retain exact",
    ],
    [
      "no-xtask-clippy",
      swap(
        "      - run: cargo clippy --locked --manifest-path xtask/Cargo.toml --all-targets -- -D warnings\n",
        "",
      ),
      '--all-targets -- -D warnings" once after the xtask tests',
    ],
    [
      "no-encryption-self-test",
      swap("      - run: bun tools/oracle/encryption-keys.ts self-test\n", ""),
      'fast must run "bun tools/oracle/encryption-keys.ts self-test" once',
    ],
    [
      "fast-selected-python-back",
      swap(FAST_SELECTED, "          python3 scripts/selected-backend-ci/test_restart_ledger.py\n"),
      "rust: fast " + NOT_PYTHON,
    ],
    [
      "fast-selected-python-added",
      swap(
        FAST_SELECTED,
        FAST_SELECTED +
          "          python3 scripts/selected-backend-ci/test_current_binding_engine_features.py\n",
      ),
      "rust: fast " + NOT_PYTHON,
    ],
    [
      "fast-selected-test-dropped",
      swap(" ./tools/selected-backend-ci/drivers/restart.test.ts", ""),
      FAST_PINNED,
    ],
    [
      "fast-selected-tests-masked",
      swap(FAST_SCRIPT_STEP, FAST_SCRIPT_STEP + "        continue-on-error: true\n"),
      FAST_PINNED,
    ],
  ] as const) {
    add(`rust-python-${name}`, "rust.yml", edit, needle);
  }
  // web.yml: Rustup metadata through the prebuilt xtask helper, and no python3.
  const inJob =
    (job: string, edit: Edit): Edit =>
    (text) => {
      const header = `\n  ${job}:\n`;
      const from = text.indexOf(header);
      if (from < 0) throw new Error(`job marker not found: ${job}`);
      const next = text.slice(from + header.length).search(/\n {2}[a-z][a-z0-9-]*:\n/);
      const to = next < 0 ? text.length : from + header.length + next;
      return text.slice(0, from) + edit(text.slice(from, to)) + text.slice(to);
    };
  // Swaps the step starting at `marker` with the step after it.
  const swapWithNext =
    (marker: string): Edit =>
    (section) => {
      const a = section.indexOf(marker);
      if (a < 0) throw new Error(`step marker not found: ${marker}`);
      const b = section.indexOf("\n      - ", a + 1) + 1;
      const c = section.indexOf("\n      - ", b + 1) + 1;
      if (b <= 0 || c <= 0) throw new Error(`step end not found: ${marker}`);
      return section.slice(0, a) + section.slice(b, c) + section.slice(a, b) + section.slice(c);
    };
  // Repeats the step starting at `marker` right after itself.
  const duplicateStep =
    (marker: string): Edit =>
    (section) => {
      const a = section.indexOf(marker);
      const b = section.indexOf("\n      - ", a + 1) + 1;
      if (a < 0 || b <= 0) throw new Error(`step marker not found: ${marker}`);
      return section.slice(0, b) + section.slice(a, b) + section.slice(b);
    };
  const META =
    "      - name: Prepare owned pinned Rustup component metadata before input capture\n";
  const META_BUILD =
    "          env -u CARGO_BUILD_TARGET -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR \\\n" +
    "            -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \\\n" +
    "            RUSTUP_AUTO_INSTALL=0 \\\n" +
    "            cargo build --quiet --locked --manifest-path xtask/Cargo.toml --target-dir xtask/target\n" +
    '          xtask/target/debug/xtask prepare-rustup-ci-metadata --output "$RUNNER_TEMP/fvoci-rustup-ci-metadata"\n';
  const META_PYTHON =
    'python3 -B scripts/prepare-rustup-ci-metadata.py --output "$RUNNER_TEMP/fvoci-rustup-ci-metadata"';
  const SQLITE_STEP = "      - name: Prepare pinned SQLite root build inputs\n";
  const APT_NO_PYTHON =
    "sudo apt-get install -y --no-install-recommends gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8\n";
  const APT_PYTHON = APT_NO_PYTHON.replace("recommends gcc", "recommends python3 gcc");
  const helper = (job: string) => `web: ${job} Rustup metadata must build and run the locked xtask`;
  const order = (job: string) => `web: ${job} Rustup metadata must follow the toolchain install`;
  const once = (job: string) => `web: ${job} requires exactly one Rustup metadata step`;
  const elsewhere = (job: string) =>
    `web: ${job} may prepare Rustup metadata only in its pinned step`;
  const python = (job: string) => `web: ${job} may not install or run python3`;
  const LANE_JOBS = ["install-on", "postgres-on", "sqlite-on", "postgres-off", "sqlite-off"].map(
    (lane) => `collaboration-${lane}`,
  );
  const webCases: [string, string, Edit, string][] = [
    [
      "untimed-python-helper",
      "workspace-browser-build",
      swap(`        run: |\n${META_BUILD}`, `        run: ${META_PYTHON}\n`),
      helper("workspace-browser-build"),
    ],
    [
      "timed-python-helper",
      "collaboration-sqlite-off",
      swap(META_BUILD, `          ${META_PYTHON}\n`),
      helper("collaboration-sqlite-off"),
    ],
    [
      "auto-install-allowed",
      "collaboration-build",
      swap("            RUSTUP_AUTO_INSTALL=0 \\\n", ""),
      helper("collaboration-build"),
    ],
    [
      "target-dir-override-kept",
      "workspace-browser-shard",
      swap("-u CARGO_TARGET_DIR ", ""),
      helper("workspace-browser-shard"),
    ],
    [
      "cargo-alias",
      "collaboration-install-on",
      swap(
        "          xtask/target/debug/xtask prepare-rustup-ci-metadata",
        "          cargo xtask prepare-rustup-ci-metadata",
      ),
      helper("collaboration-install-on"),
    ],
    [
      "offline-helper-build",
      "collaboration-postgres-on",
      swap(
        "cargo build --quiet --locked --manifest",
        "cargo build --quiet --locked --offline --manifest",
      ),
      helper("collaboration-postgres-on"),
    ],
    [
      "step-env-override",
      "collaboration-sqlite-on",
      swap(META, `${META}        env:\n          RUSTUP_AUTO_INSTALL: "1"\n`),
      helper("collaboration-sqlite-on"),
    ],
    [
      "stale-timing-label",
      "collaboration-install-on",
      swap("collaboration-install-on-step-05 started", "collaboration-install-on-step-04 started"),
      helper("collaboration-install-on"),
    ],
    [
      "before-cargo-restore",
      "workspace-browser-build",
      swapWithNext("      - name: Restore Cargo downloads\n"),
      order("workspace-browser-build"),
    ],
    [
      "after-sqlite",
      "collaboration-postgres-off",
      swapWithNext(META),
      order("collaboration-postgres-off"),
    ],
    [
      "step-missing",
      "collaboration-build",
      swap(META, "      - name: Prepare Rustup component metadata\n"),
      once("collaboration-build"),
    ],
    [
      "step-duplicated",
      "workspace-browser-shard",
      duplicateStep(META),
      once("workspace-browser-shard"),
    ],
    [
      "extra-python-step",
      "collaboration-build",
      swap(SQLITE_STEP, `      - run: ${META_PYTHON}\n${SQLITE_STEP}`),
      elsewhere("collaboration-build"),
    ],
    [
      "unpinned-job",
      "web-native-checks",
      swap(
        SQLITE_STEP,
        '      - run: xtask/target/debug/xtask prepare-rustup-ci-metadata --output "$RUNNER_TEMP/m"\n' +
          SQLITE_STEP,
      ),
      elsewhere("web-native-checks"),
    ],
    [
      "native-checks-apt",
      "web-native-checks",
      swap(APT_NO_PYTHON, APT_PYTHON),
      python("web-native-checks"),
    ],
    [
      "producer-apt",
      "collaboration-build",
      swap(APT_NO_PYTHON, APT_PYTHON),
      python("collaboration-build"),
    ],
    [
      "static-python",
      "web-static",
      swap(
        "          bun run lint\n",
        "          bun run lint\n          python3 -m pip --version\n",
      ),
      python("web-static"),
    ],
    [
      "setup-python",
      "workspace-browser-shard",
      swap(SQLITE_STEP, `      - uses: actions/setup-python@${"2".repeat(40)}\n${SQLITE_STEP}`),
      python("workspace-browser-shard"),
    ],
    [
      "job-env-python",
      "workspace-browser-build",
      swap(
        "    timeout-minutes: 20\n",
        "    timeout-minutes: 20\n    env:\n      PYTHONPATH: scripts\n",
      ),
      python("workspace-browser-build"),
    ],
    [
      "lane-extra-python",
      "collaboration-postgres-on",
      swap(
        "          bun ci\n",
        "          bun ci\n          python3 scripts/ci_selection.py verify-workflows\n",
      ),
      python("collaboration-postgres-on"),
    ],
    [
      "checks-extra-python",
      "web-checks",
      swap("          bun ci\n", "          bun ci\n          python3 -m pip install pyyaml\n"),
      python("web-checks"),
    ],
    [
      "checks-python-step",
      "web-checks",
      swap(
        "      - name: Verify normal web e2e shard plan\n",
        "      - run: python3 scripts/selected-backend-ci/test_off_registration.py\n" +
          "      - name: Verify normal web e2e shard plan\n",
      ),
      python("web-checks"),
    ],
    [
      "checks-python-registration-line",
      "web-checks",
      swap(
        "          (cd apps/web && bun test e2e-pending/collab-playwright.config.test.ts --timeout 60000)\n",
        "          (cd apps/web && bun test e2e-pending/collab-playwright.config.test.ts --timeout 60000)\n" +
          "          python3 scripts/selected-backend-ci/test_off_registration.py\n",
      ),
      python("web-checks"),
    ],
    ...["web-checks", ...LANE_JOBS].map((job): [string, string, Edit, string] => [
      `apt-${job}`,
      job,
      swap(APT_NO_PYTHON, APT_PYTHON),
      python(job),
    ]),
  ];
  for (const [name, job, edit, needle] of webCases) {
    add(`web-rustup-${name}`, "web.yml", inJob(job, edit), needle);
  }
  const TOP_PERMISSIONS = "permissions:\n  contents: read\n";
  for (const [name, setting] of [
    ["env", "env:\n  PYTHONPATH: scripts\n"],
    ["defaults-shell", "defaults:\n  run:\n    shell: python3 {0}\n"],
  ] as const) {
    add(
      `web-python-workflow-${name}`,
      "web.yml",
      swap(TOP_PERMISSIONS, TOP_PERMISSIONS + setting),
      "web: workflow-level settings may not configure python",
    );
  }
  add(
    "web-rustup-restore-wrong-path",
    "web.yml",
    inJob(
      "collaboration-build",
      swap(
        "          path: |\n            ~/.cargo/registry\n            ~/.cargo/git\n",
        "          path: target\n",
      ),
    ),
    order("collaboration-build"),
  );
  add(
    "web-rustup-restore-wrong-action",
    "web.yml",
    inJob(
      "workspace-browser-shard",
      within("      - name: Restore Cargo downloads\n", "      - name:", (step) =>
        step.replace("uses: actions/cache@", "uses: actions/upload-artifact@"),
      ),
    ),
    order("workspace-browser-shard"),
  );
  const SMOKE = "web: web-static must type-check and test the install smoke helpers";
  add(
    "web-install-smoke-tests-dropped",
    "web.yml",
    swap("          bun test ./tools/install-smoke/\n", ""),
    SMOKE,
  );
  add(
    "web-install-smoke-types-dropped",
    "web.yml",
    swap("          bun --bun x --no-install tsc -p tools/install-smoke/tsconfig.json\n", ""),
    SMOKE,
  );
  add(
    "web-install-smoke-masked",
    "web.yml",
    swap(
      "      - name: Install smoke helper types and unit tests\n",
      "      - name: Install smoke helper types and unit tests\n        continue-on-error: true\n",
    ),
    SMOKE,
  );
  return cases;
}

export const MUTATION_CASES: readonly MutationCase[] = caseList();
