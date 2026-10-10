import {
  deepEqual,
  get,
  has,
  isMapping,
  sameKeys,
  triggersOf,
  type Mapping,
  type Value,
} from "./load.ts";

export const TURSO_MANUAL_WORKFLOW_FILE = "turso-test.yml";

const REPOSITORY = "AISFlow/fvoci";
const REVIEWED_REF = "refs/heads/fvoci/v060-turso-verified-connection";
const UI_REVIEWED_REF = "refs/heads/fvoci/v060-product-integration-20261005";

// Every command the manual Turso workflow runs through the repository's guard
// and fixture tools. Moving a tool to another runtime changes only this table.
export const TURSO_COMMANDS = {
  fixtures: "python3 scripts/selected-backend-ci/turso-test-fixtures.py",
  admit: "python3 scripts/selected-backend-ci/turso-test-guard.py --admit",
  freeze: "python3 scripts/selected-backend-ci/turso-test-guard.py --freeze",
  diagnosticUnit: "python3 scripts/selected-backend-ci/turso-test-guard.py --diagnostic-unit",
  consume: "python3 scripts/selected-backend-ci/turso-test-guard.py --consume",
  uiRecordBefore: "python3 scripts/selected-backend-ci/turso-ui.py --record-before",
  uiFreeze: "python3 scripts/selected-backend-ci/turso-ui.py --freeze",
  /** Packages the compiler preparation installs; python3 is there for the guard. */
  aptPackages: "python3 gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8",
  /** Extra turso-ui job environment for the guard runtime. */
  uiRuntimeEnv: { PYTHONDONTWRITEBYTECODE: "1" } as Mapping,
} as const;

const CHECKOUT = {
  uses: "actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
  with: { ref: "${{ github.sha }}", "persist-credentials": false },
};

const TARGET_INIT = `printf 'CARGO_TARGET_DIR=%s/turso-target\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n`;
const SQLITE_PREPARATION =
  "sudo apt-get update\n" +
  `sudo apt-get install -y --no-install-recommends ${TURSO_COMMANDS.aptPackages}\n` +
  'mkdir "$RUNNER_TEMP/fvoci-sqlite"\n' +
  'dpkg-query -W > "$RUNNER_TEMP/fvoci-sqlite/build-packages.txt"\n' +
  'bash scripts/prepare-sqlite-ci.sh --parent "$RUNNER_TEMP/fvoci-sqlite" \\\n' +
  '  --github-env "$GITHUB_ENV" --github-output "$GITHUB_OUTPUT"\n' +
  "cargo fetch --locked\n";

const CONNECTION_PREPARATION =
  "set -euo pipefail\n" +
  TARGET_INIT +
  "rustup toolchain install 1.98.1 --profile minimal\n" +
  SQLITE_PREPARATION;

const CONNECTION_COMPILE =
  "set -euo pipefail\n" +
  'cargo test --locked --offline --lib --features db-tests --jobs 4 --no-run --message-format=json > "$RUNNER_TEMP/turso-compile.json"\n' +
  TURSO_COMMANDS.freeze +
  "\n";

const DISPATCH_INPUTS = {
  phase: {
    description:
      "Connection, inventory and ui-baseline read-only; migration, reset and ui-ack require both destructive gates; others NOT IMPLEMENTED",
    type: "choice",
    default: "connection",
    options: [
      "connection",
      "crud",
      "transactions",
      "migration",
      "inventory",
      "reset",
      "persistence",
      "restore",
      "ui-ack",
      "ui-baseline",
    ],
  },
  destructive: {
    description:
      "Explicit isolated test DB mutation confirmation (connection, inventory and ui-baseline must be false)",
    type: "boolean",
    default: false,
  },
  ui_source_sha: {
    description: "ROOT reviewed exact UI source SHA (ui-baseline and ui-ack only)",
    type: "string",
    default: "",
  },
  ui_baseline_sha256: {
    description: "ROOT verified just-observed current dataset digest (ui-ack only)",
    type: "string",
    default: "",
  },
  ui_target_sha256: {
    description: "ROOT verified just-observed primary target digest (ui-ack only)",
    type: "string",
    default: "",
  },
};

const TRUSTED = `github.event_name == 'workflow_dispatch' && github.repository == '${REPOSITORY}' && (github.ref == 'refs/heads/main' || github.ref == '${REVIEWED_REF}')`;
const BOOTSTRAP_ADMISSION =
  `github.repository == '${REPOSITORY}' && ((github.event_name == 'push' && github.ref == '${REVIEWED_REF}') || ` +
  `(github.event_name == 'workflow_dispatch' && (github.ref == 'refs/heads/main' || github.ref == '${REVIEWED_REF}')) || ` +
  `(github.event_name == 'workflow_dispatch' && github.ref == '${UI_REVIEWED_REF}' && (github.event.inputs.phase == 'ui-baseline' || github.event.inputs.phase == 'ui-ack')))`;
const RUNTIME_IF =
  TRUSTED.replace(
    "github.event_name == 'workflow_dispatch'",
    "github.event_name == 'workflow_dispatch' && github.event.inputs.phase != 'ui-baseline' && github.event.inputs.phase != 'ui-ack'",
  ) + " && needs.admission.result == 'success' && needs.admission.outputs.environment_id != ''";

const UI_JOB = {
  needs: "admission",
  if:
    "github.event_name == 'workflow_dispatch' && (github.event.inputs.phase == 'ui-baseline' || " +
    `github.event.inputs.phase == 'ui-ack') && github.repository == '${REPOSITORY}' && (github.ref == ` +
    `'refs/heads/main' || github.ref == '${REVIEWED_REF}' || github.ref == '${UI_REVIEWED_REF}') && ` +
    "needs.admission.result == 'success' && needs.admission.outputs.environment_id != ''",
  environment: "fvoci-turso-test",
  "runs-on": "ubuntu-26.04",
  "timeout-minutes": 40,
  env: {
    FVOCI_BUILD_SHA: "${{ github.sha }}",
    LIBCLANG_PATH: "/usr/lib/llvm-18/lib",
    CARGO_BUILD_JOBS: 2,
    CARGO_INCREMENTAL: 0,
    CARGO_PROFILE_DEV_DEBUG: 0,
    CARGO_PROFILE_TEST_DEBUG: 0,
    ...TURSO_COMMANDS.uiRuntimeEnv,
  },
  steps: [
    CHECKOUT,
    {
      name: "Credential-free current UI input preparation",
      run:
        "set -euo pipefail\n" +
        `printf 'CARGO_TARGET_DIR=%s/turso-ui-target\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n` +
        `printf 'BUN_INSTALL_CACHE_DIR=%s/turso-bun-cache\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n` +
        `printf 'PLAYWRIGHT_BROWSERS_PATH=%s/turso-browsers\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n` +
        "rustup toolchain install 1.98.1 --profile minimal\n" +
        SQLITE_PREPARATION +
        "cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml\n",
    },
    {
      uses: "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6",
      with: { "bun-version": "1.4.2" },
    },
    {
      name: "Credential-free fixed dependencies and fresh browser assets",
      run:
        "set -euo pipefail\n" +
        "bun install --frozen-lockfile\n" +
        "bun --bun x playwright install --with-deps chromium\n" +
        `${TURSO_COMMANDS.uiRecordBefore}\n` +
        "bun --bun run --cwd apps/web build\n",
    },
    {
      name: "Credential-free current native UI cohort and freeze",
      run:
        "set -euo pipefail\n" +
        "cargo build --locked --offline --features api-schema,db-tests --jobs 2 --bin " +
        "fvoci-server --bin fvoci-migrate --bin fvoci-e2e-fixture --message-format=json > " +
        '"$RUNNER_TEMP/ui-compile.json"\n' +
        "cargo build --locked --offline --manifest-path crates/collab-engine/Cargo.toml " +
        "--features worker --jobs 2 --bin collab-engine --message-format=json > " +
        '"$RUNNER_TEMP/ui-engine-compile.json"\n' +
        `${TURSO_COMMANDS.uiFreeze}\n`,
    },
    {
      name: "Actual current primary UI baseline or guarded ON restart OFF consumer",
      env: {
        FVOCI_DATABASE_BACKEND: "libsql-remote",
        FVOCI_LIBSQL_URL: "${{ secrets.FVOCI_TEST_TURSO_DATABASE_URL }}",
        FVOCI_LIBSQL_AUTH_TOKEN: "${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}",
        FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "${{ vars.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE }}",
      },
      run: TURSO_COMMANDS.consume,
    },
  ],
};

/** Named manual-only exception; never changes PR selection or stable gates. */
export function verifyTursoWorkflow(data: Mapping, name = TURSO_MANUAL_WORKFLOW_FILE): string[] {
  const errors: string[] = [];
  const require = (condition: boolean, boundary: string): void => {
    if (!condition) errors.push(`${name}: ${boundary}`);
  };

  const triggers = triggersOf(data);
  require(sameKeys(triggers, ["push", "workflow_dispatch"]) &&
    deepEqual(get(triggers, "push"), {
      branches: ["fvoci/v060-turso-verified-connection"],
    }), "manual dispatch only with fixed credential-free bootstrap");
  const dispatch: Value | undefined = isMapping(triggers)
    ? has(triggers, "workflow_dispatch")
      ? get(triggers, "workflow_dispatch")
      : {}
    : {};
  require(isMapping(dispatch) &&
    deepEqual(
      get(dispatch, "inputs"),
      DISPATCH_INPUTS,
    ), "fixed phase inputs and non-destructive default");
  require(deepEqual(get(data, "permissions"), { contents: "read" }), "contents read only");
  require(!has(data, "env"), "no global credential environment");
  require(Object.keys(data).every((key) =>
    ["name", "on", "permissions", "concurrency", "jobs"].includes(key),
  ), "no top-level defaults, run-name or other workflow keys");
  require(deepEqual(get(data, "concurrency"), {
    group: "fvoci-turso-test-database",
    "cancel-in-progress": false,
  }), "fixed database concurrency without cancellation");
  const jobs = get(data, "jobs");
  if (!sameKeys(jobs, ["admission", "turso-connection", "turso-ui"])) {
    return [...errors, `${name}: exactly admission, turso-connection and turso-ui jobs required`];
  }
  const admission = get(jobs, "admission");
  const runtime = get(jobs, "turso-connection");
  if (!isMapping(admission) || !isMapping(runtime)) {
    return [...errors, `${name}: job mappings required`];
  }
  require(sameKeys(admission, [
    "if",
    "runs-on",
    "timeout-minutes",
    "outputs",
    "steps",
  ]), "admission has no Environment or credentials");
  require(get(admission, "if") === BOOTSTRAP_ADMISSION &&
    deepEqual(get(admission, "outputs"), {
      environment_id: "${{ steps.admit.outputs.environment_id }}",
    }), "trusted admission and existence output");
  require(deepEqual(get(admission, "steps"), [
    CHECKOUT,
    { name: "Pure admission fixtures (no credentials or network)", run: TURSO_COMMANDS.fixtures },
    {
      name: "Verify preexisting Environment (no configuration writes)",
      id: "admit",
      run: TURSO_COMMANDS.admit,
    },
  ]), "pre-Environment admission steps");
  require(sameKeys(runtime, [
    "needs",
    "if",
    "environment",
    "runs-on",
    "timeout-minutes",
    "env",
    "steps",
  ]), "runtime job cannot add unchecked execution or permissions");
  require(get(runtime, "needs") === "admission" &&
    get(runtime, "if") === RUNTIME_IF, "runtime needs successful trusted admission");
  require(get(runtime, "environment") === "fvoci-turso-test", "fixed Environment");
  require(deepEqual(get(runtime, "env"), {
    LIBCLANG_PATH: "/usr/lib/llvm-18/lib",
    CARGO_BUILD_JOBS: 4,
    CARGO_INCREMENTAL: 0,
    CARGO_PROFILE_DEV_DEBUG: 0,
    CARGO_PROFILE_TEST_DEBUG: 0,
  }), "credential-free compiler environment");
  require(get(admission, "runs-on") === "ubuntu-26.04" &&
    get(runtime, "runs-on") === "ubuntu-26.04" &&
    get(admission, "timeout-minutes") === 5 &&
    get(runtime, "timeout-minutes") === 15, "fixed runner and budgets");
  const steps = get(runtime, "steps");
  if (!Array.isArray(steps) || steps.length !== 5 || !steps.every(isMapping)) {
    return [...errors, `${name}: fixed credential-free build then single consuming step`];
  }
  const [checkout, preparation, compile, unit, consume] = steps;
  require(deepEqual(checkout, CHECKOUT), "exact SHA checkout with stripped credentials");
  require(sameKeys(preparation, ["name", "run"]) &&
    sameKeys(compile, ["name", "run"]), "no compilation credentials");
  require(get(preparation, "run") ===
    CONNECTION_PREPARATION, "maintained pinned compiler/native preparation");
  require(get(compile, "run") === CONNECTION_COMPILE, "fixed fresh compilation and ELF binding");
  require(deepEqual(unit, {
    name: "Credential-free frozen diagnostic unit (exactly one test)",
    run: TURSO_COMMANDS.diagnosticUnit,
  }), "credential-free exact frozen diagnostic unit before secret consumption");
  require(deepEqual(consume, {
    name: "Real primary selected phase (exactly one test)",
    env: {
      FVOCI_DATABASE_BACKEND: "libsql-remote",
      FVOCI_TEST_TURSO_DATABASE_URL: "${{ secrets.FVOCI_TEST_TURSO_DATABASE_URL }}",
      FVOCI_TEST_TURSO_AUTH_TOKEN: "${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}",
      FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "${{ vars.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE }}",
    },
    run: TURSO_COMMANDS.consume,
  }), "only one sanitized runtime step consumes two secrets");
  // The private UI allocation is closed: no extra unchecked step, secret in
  // preparation, artifact upload, mutable source, bypass or other target.
  require(deepEqual(
    get(jobs, "turso-ui"),
    UI_JOB,
  ), "fixed private Turso UI job and current source baseline consumer");
  return errors;
}

function count(text: string, needle: string): number {
  return text.split(needle).length - 1;
}

const SECRET_STEP = "      - name: Real primary selected phase";
const CONNECTION_PREPARATION_MARKER =
  "      - name: Credential-free compiler and maintained SQLite inputs\n        run: |\n";

/**
 * Literal-text order checks on turso-test.yml that the structural check cannot
 * express: no secret reference anywhere before the one consuming step (comments
 * included), and the freeze -> diagnostic unit -> secret step order.
 */
export function verifyTursoWorkflowText(text: string, name = TURSO_MANUAL_WORKFLOW_FILE): string[] {
  const errors: string[] = [];
  const require = (condition: boolean, boundary: string): void => {
    if (!condition) errors.push(`${name}: text: ${boundary}`);
  };

  const marker = text.indexOf(CONNECTION_PREPARATION_MARKER);
  require(marker >= 0, "connection preparation step is a literal block");
  if (marker >= 0) {
    const preparation =
      text.slice(marker + CONNECTION_PREPARATION_MARKER.length).split("      - name:", 1)[0] ?? "";
    const prefix = ["set -euo pipefail\n", TARGET_INIT].map((line) => "          " + line).join("");
    require(preparation.startsWith(prefix), "CARGO_TARGET_DIR is published before preparation");
  }
  require(!text.includes("      CARGO_TARGET_DIR:"), "CARGO_TARGET_DIR is never a job or step env");

  const freeze = text.indexOf(TURSO_COMMANDS.freeze);
  const unit = text.indexOf(TURSO_COMMANDS.diagnosticUnit);
  const secret = text.indexOf(SECRET_STEP);
  require(freeze >= 0 &&
    unit > freeze &&
    secret > unit, "freeze, then the diagnostic unit, then the secret step");
  require(count(text, TURSO_COMMANDS.diagnosticUnit) === 1, "exactly one diagnostic unit");
  require(secret >= 0 &&
    !text.slice(0, secret).includes("secrets."), "no secret before the secret step");

  for (const literal of [
    "options: [connection, crud, transactions, migration, inventory, reset, persistence, restore, ui-ack, ui-baseline]",
    "        default: connection\n",
    "        type: boolean\n        default: false\n",
    "  group: fvoci-turso-test-database\n  cancel-in-progress: false\n",
    "permissions:\n  contents: read\n",
  ]) {
    require(text.includes(literal), `retains ${JSON.stringify(literal)}`);
  }
  for (const [literal, expected] of [
    ["persist-credentials: false", 3],
    ["ref: ${{ github.sha }}", 3],
    [`github.repository == '${REPOSITORY}'`, 3],
    [REVIEWED_REF, 4],
    [UI_REVIEWED_REF, 2],
    ["timeout-minutes: 5", 1],
    ["timeout-minutes: 15", 1],
    ["environment: fvoci-turso-test", 2],
  ] as const) {
    require(count(text, literal) ===
      expected, `${JSON.stringify(literal)} occurs ${String(expected)} times`);
  }
  const jobsAt = text.indexOf("jobs:");
  const connectionAt = text.indexOf("  turso-connection:");
  const uiAt = text.indexOf("  turso-ui:", connectionAt + 1);
  require(jobsAt >= 0 &&
    !text.slice(0, jobsAt).includes(UI_REVIEWED_REF), "UI reviewed ref is not a trigger");
  require(connectionAt >= 0 &&
    uiAt > connectionAt &&
    !text
      .slice(connectionAt, uiAt)
      .includes(UI_REVIEWED_REF), "UI reviewed ref never admits the connection job");
  return errors;
}
