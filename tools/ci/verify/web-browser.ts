// web.yml browser budget and the current-run native build handoff: the only
// admitted cross-job native consumer, bound to this run's producer artifact.
// The handoff check also pins the native jobs' Rustup metadata preparation
// and the jobs that may still use python3.
import { get, has, isMapping, pyContains, pyEq, pySplitlines, type Mapping } from "./py.ts";
import { join } from "node:path";
import { rawBlockScalar } from "./raw-yaml.ts";
import { gatedWorkflowJobs } from "./registry.ts";
import { readUtf8, type VerifyContext } from "./rust-shared.ts";

export const WEB_WORKFLOW_FILE = "web.yml";
export function webWorkflowJobs(ctx: VerifyContext): Mapping | null {
  return gatedWorkflowJobs(ctx, WEB_WORKFLOW_FILE);
}

// Raw source text of the block-style scalar `jobs.<jobId>.<key>`, or null when
// the job or key is not written exactly once (in any quoting) in plain form.
export function rawJobScalar(source: string, jobId: string, key: string): string | null {
  return rawBlockScalar(source, ["jobs", jobId, key]);
}

// PyYAML (YAML 1.1) reads `20.0` as a float and `020` as octal, which the
// original refuses; Bun.YAML turns both into the number 20. The raw scalar
// therefore has to be a plain decimal integer as well.
const PLAIN_DECIMAL = /^(0|[1-9][0-9]*)$/;

export function verifyWebBrowserBudgetJobs(jobs: Mapping, rawBudget: string | null): string[] {
  const job = get(jobs, "workspace-browser-shard");
  if (!isMapping(job)) return ["web: normal browser shard must be a mapping"];
  const budget = get(job, "timeout-minutes");
  if (
    typeof budget !== "number" ||
    !Number.isInteger(budget) ||
    budget !== 20 ||
    rawBudget === null ||
    !PLAIN_DECIMAL.test(rawBudget)
  ) {
    return ["web: normal browser shard requires the measured 20 minute job budget"];
  }
  return [];
}

export function verifyWebBrowserBudget(ctx: VerifyContext): string[] {
  const jobs = webWorkflowJobs(ctx);
  if (jobs === null) return [];
  // The parsed jobs and the raw scalar must come from the same text.
  const texts = ctx.texts;
  const source =
    texts !== undefined && Object.hasOwn(texts, WEB_WORKFLOW_FILE)
      ? (texts[WEB_WORKFLOW_FILE] ?? null)
      : readUtf8(join(ctx.root, ".github", "workflows", WEB_WORKFLOW_FILE));
  const raw =
    source === null ? null : rawJobScalar(source, "workspace-browser-shard", "timeout-minutes");
  return verifyWebBrowserBudgetJobs(jobs, raw);
}

const CLOSED_INSTALL_RECEIPT =
  "${{ runner.temp }}/fvoci-closed-install/closed-install-receipt.json";
const PRODUCER_DOWNLOAD_WITH = {
  "artifact-ids": "${{ needs.collaboration-build.outputs.artifact_id }}",
  "merge-multiple": true,
  path: "${{ runner.temp }}/fvoci-web-build-handoff",
};
const RECEIPT_DOWNLOAD_WITH = {
  "artifact-ids": "${{ needs.collaboration-install-on.outputs.install_receipt_artifact_id }}",
  "merge-multiple": true,
  path: "${{ runner.temp }}/fvoci-closed-install",
};
export const DOWNLOAD_PIN = "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093";
const UPLOAD_PIN = "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02";

type Lane = {
  name: string;
  token: string;
  needs: string[];
  extraEnv: Record<string, string>;
  downloads: Mapping[];
};
const afterInstall = ["ci-plan", "collaboration-build", "collaboration-install-on"];
const receiptEnv = { FVOCI_CLOSED_INSTALL_RECEIPT: CLOSED_INSTALL_RECEIPT };
const bothDownloads = [PRODUCER_DOWNLOAD_WITH, RECEIPT_DOWNLOAD_WITH];
export const WEB_COLLAB_LANES: readonly Lane[] = [
  {
    name: "collaboration-install-on",
    token: "install/on",
    needs: ["ci-plan", "collaboration-build"],
    extraEnv: {},
    downloads: [PRODUCER_DOWNLOAD_WITH],
  },
  {
    name: "collaboration-postgres-on",
    token: "postgres/on",
    needs: afterInstall,
    extraEnv: receiptEnv,
    downloads: bothDownloads,
  },
  {
    name: "collaboration-sqlite-on",
    token: "sqlite/on",
    needs: afterInstall,
    extraEnv: receiptEnv,
    downloads: bothDownloads,
  },
  {
    name: "collaboration-postgres-off",
    token: "postgres/off",
    needs: afterInstall,
    extraEnv: receiptEnv,
    downloads: bothDownloads,
  },
  {
    name: "collaboration-sqlite-off",
    token: "sqlite/off",
    needs: afterInstall,
    extraEnv: receiptEnv,
    downloads: bothDownloads,
  },
];

// Pending registration controls are DB-free but outside apps/web's default
// src-only unit discovery, so their consumer step is pinned explicitly.
const UNIT_REGRESSION_STEP = "Web and editor unit regressions";
const UNIT_REGRESSION_COMMANDS = [
  "(cd apps/web && bun run test)",
  "(cd packages/editor && bun run test)",
  "(cd apps/web && bun test e2e-pending/collab-playwright.config.test.ts --timeout 60000)",
  "python3 scripts/selected-backend-ci/test_off_registration.py",
];

const unmasked = (step: Mapping) => !has(step, "if") && !has(step, "continue-on-error");
const usesPrefix = (step: Mapping, prefix: string) => {
  const uses = get(step, "uses", "");
  return typeof uses === "string" && uses.startsWith(prefix);
};

export function verifyWebBuildHandoffJobs(jobs: Mapping): string[] {
  const errors: string[] = [];
  const require = (ok: boolean, message: string) => {
    if (!ok) errors.push("web: current build handoff " + message);
  };
  // Shapes the original cannot evaluate (it raises) fail closed here.
  const jobOf = (name: string): Mapping => {
    const job = get(jobs, name, {});
    if (isMapping(job)) return job;
    require(false, `job ${name} must be a mapping`);
    return {};
  };
  const stepsOf = (name: string, job: Mapping): Mapping[] => {
    const steps = get(job, "steps", []);
    const list = Array.isArray(steps) ? steps : [];
    require(Array.isArray(steps) &&
      list.every(isMapping), `job ${name} steps must be a list of mappings`);
    return list.filter(isMapping);
  };

  const producer = jobOf("collaboration-build");
  require(get(producer, "needs") === "ci-plan", "producer needs ci-plan");
  const lanes = WEB_COLLAB_LANES.map((lane) => ({ lane, job: jobOf(lane.name) }));
  for (const { lane, job } of lanes) {
    require(pyEq(get(job, "needs"), lane.needs), "consumer needs successful registered producer");
  }
  const stepsByJob = new Map<string, Mapping[]>();
  for (const [name, job] of [
    ["collaboration-build", producer] as const,
    ...lanes.map(({ lane, job }) => [lane.name, job] as const),
  ]) {
    require(get(job, "runs-on") === "ubuntu-26.04" &&
      pyEq(get(job, "timeout-minutes"), 15), "fixed runner/budget");
    require(!["continue-on-error", "strategy", "env", "permissions"].some((key) =>
      has(job, key),
    ), "no masked/alternate authority");
    const steps = stepsOf(name, job);
    stepsByJob.set(name, steps);
    const checkout = steps.filter((step) => usesPrefix(step, "actions/checkout@"));
    require(checkout.length === 1 &&
      pyEq(get(checkout[0] as Mapping, "with"), {
        "persist-credentials": false,
      }), "default exact checkout without stored credentials");
    require(get(job, "if") ===
      "needs.ci-plan.outputs.select_" +
        name.replaceAll("-", "_") +
        " == 'true'", "selection only by registered plan");
  }

  const webChecks = jobOf("web-checks");
  const unit = stepsOf("web-checks", webChecks).filter(
    (step) => get(step, "name") === UNIT_REGRESSION_STEP,
  );
  const unitRun = unit.length === 1 ? get(unit[0] as Mapping, "run", "") : undefined;
  require(unit.length === 1 &&
    typeof unitRun === "string" &&
    pyEq(pySplitlines(unitRun), UNIT_REGRESSION_COMMANDS) &&
    unmasked(
      unit[0] as Mapping,
    ), "mandatory complete web/editor and selected registration fixtures");

  const producerSteps = stepsByJob.get("collaboration-build") ?? [];
  const prepare = producerSteps.filter((step) => get(step, "id") === "prepare");
  require(prepare.length === 1 &&
    pyEq(get(prepare[0] as Mapping, "env"), { FVOCI_E2E_PENDING: "1" }) &&
    pyContains(
      get(prepare[0] as Mapping, "run", ""),
      "bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-prepare-selected",
    ) &&
    unmasked(prepare[0] as Mapping), "unconditional qualified producer");
  const publish = producerSteps.filter((step) => get(step, "id") === "publish");
  require(publish.length === 1 &&
    get(publish[0] as Mapping, "uses") === UPLOAD_PIN &&
    pyEq(get(publish[0] as Mapping, "with"), {
      name: "web-current-build-${{ github.run_attempt }}",
      path: "${{ runner.temp }}/fvoci-web-build-handoff/handoff.json\n${{ runner.temp }}/fvoci-web-build-handoff/payload.tar\n",
      "if-no-files-found": "error",
      "retention-days": 1,
    }) &&
    unmasked(publish[0] as Mapping), "publish only successful complete packet");
  require(pyEq(get(producer, "outputs"), {
    artifact_id: "${{ steps.publish.outputs.artifact-id }}",
    handoff_sha256: "${{ steps.prepare.outputs.handoff_sha256 }}",
  }), "producer artifact identity and digest outputs");

  for (const { lane } of lanes) {
    const steps = stepsByJob.get(lane.name) ?? [];
    const downloads = steps.filter((step) => usesPrefix(step, "actions/download-artifact@"));
    const exact = lane.downloads.flatMap((expected) =>
      downloads
        .filter(
          (step) =>
            pyEq(get(step, "with"), expected) &&
            get(step, "uses") === DOWNLOAD_PIN &&
            unmasked(step),
        )
        .slice(0, 1),
    );
    require(downloads.length === lane.downloads.length &&
      exact.length ===
        lane.downloads.length, "current-run exact artifact ID without foreign token/ref/run");
    const runtime = steps.filter((step) => get(step, "id") === "browser");
    require(runtime.length === 1 &&
      pyEq(get(runtime[0] as Mapping, "env"), {
        FVOCI_E2E_PENDING: "1",
        FVOCI_WEB_BUILD_HANDOFF_SHA256: "${{ needs.collaboration-build.outputs.handoff_sha256 }}",
        FVOCI_COLLAB_LANE: lane.token,
        ...lane.extraEnv,
      }) &&
      pyContains(
        get(runtime[0] as Mapping, "run", ""),
        "bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-consume-selected",
      ) &&
      unmasked(runtime[0] as Mapping), "mandatory full original runtime after qualification");
    const borrowsTarget = steps
      .filter((step) => usesPrefix(step, "actions/cache@"))
      .some((step) => {
        const withs = get(step, "with", {});
        if (!isMapping(withs)) return true;
        const path = get(withs, "path");
        return pyEq(path, "target") || pyEq(path, "crates/collab-engine/target");
      });
    require(!borrowsTarget, "consumer cannot borrow target cache");
  }
  return errors;
}

export function verifyWebBuildHandoff(ctx: VerifyContext): string[] {
  const jobs = webWorkflowJobs(ctx);
  if (jobs === null) return [];
  const workflow = ctx.workflows[WEB_WORKFLOW_FILE];
  return [
    ...verifyWebBuildHandoffJobs(jobs),
    ...verifyWebRustupMetadataJobs(jobs, isMapping(workflow) ? workflow : {}),
    ...verifyWebStaticInstallSmokeJobs(jobs),
  ];
}

// Rustup component metadata is prepared by the xtask helper, built from its own
// lockfile on the host (the consumer's target and rustflags overrides cleared,
// no toolchain auto-install) and run directly. It runs once in every job that
// captures native build inputs: after the pinned toolchain install and the
// Cargo download restore, before the SQLite step and every product Cargo
// command. The helper build itself writes only xtask/target and the Cargo
// registry; with auto-install off it cannot change the pinned toolchain.
export const RUSTUP_METADATA_STEP =
  "Prepare owned pinned Rustup component metadata before input capture";
export const RUSTUP_METADATA_RUN =
  "env -u CARGO_BUILD_TARGET -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR \\\n" +
  "  -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \\\n" +
  "  RUSTUP_AUTO_INSTALL=0 \\\n" +
  "  cargo build --quiet --locked --manifest-path xtask/Cargo.toml --target-dir xtask/target\n" +
  'xtask/target/debug/xtask prepare-rustup-ci-metadata --output "$RUNNER_TEMP/fvoci-rustup-ci-metadata"\n';
const CARGO_DOWNLOADS = "~/.cargo/registry\n~/.cargo/git\n";
const TOOLCHAIN_INSTALL = "rustup toolchain install 1.98.1 --profile minimal --component clippy";
const UNTIMED_RUSTUP_JOBS = ["workspace-browser-build", "workspace-browser-shard"];
const TIMED_RUSTUP_JOBS = ["collaboration-build", ...WEB_COLLAB_LANES.map((lane) => lane.name)];

// The collaboration jobs wrap each run in a ci-step timer named by position.
function timingPreamble(job: string, position: number): string {
  const stage = `${job}-step-${String(position).padStart(2, "0")}`;
  return (
    "fvoci_timing_started=$SECONDS\n" +
    `printf 'ci-step stage=${stage} started at=%s\\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"\n` +
    `trap 'fvoci_timing_exit=$?; printf "ci-step stage=${stage} finished at=%s elapsed_seconds=%s exit=%s\\n" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$((SECONDS - \${fvoci_timing_started}))" "$fvoci_timing_exit"; exit "$fvoci_timing_exit"' EXIT\n`
  );
}

// python3 stays only where a caller still needs it: the selected-backend lane
// drivers (current-<lane>-driver.py) in the five collaboration lanes, and the
// pending OFF registration check in web-checks.
const APT_WITH_PYTHON =
  "sudo apt-get install -y --no-install-recommends python3 gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8";
const PYTHON_LINES: Readonly<Record<string, readonly string[]>> = {
  "web-checks": [APT_WITH_PYTHON, UNIT_REGRESSION_COMMANDS.at(-1) ?? ""],
  ...Object.fromEntries(WEB_COLLAB_LANES.map((lane) => [lane.name, [APT_WITH_PYTHON]])),
};
const PYTHON = /python/i;

const without = (value: unknown, key: string): unknown =>
  isMapping(value) ? Object.fromEntries(Object.entries(value).filter(([k]) => k !== key)) : value;
const stepRun = (step: unknown): string => {
  const run = isMapping(step) ? get(step, "run") : undefined;
  return typeof run === "string" ? run : "";
};

export function verifyWebRustupMetadataJobs(jobs: Mapping, workflow: Mapping): string[] {
  const errors: string[] = [];
  // Workflow-level env and defaults reach every job.
  if (PYTHON.test(JSON.stringify(without(workflow, "jobs")))) {
    errors.push("web: workflow-level settings may not configure python");
  }
  const pinned = new Set<unknown>();
  for (const name of [...UNTIMED_RUSTUP_JOBS, ...TIMED_RUSTUP_JOBS]) {
    const job = get(jobs, name);
    const steps = isMapping(job) ? get(job, "steps") : undefined;
    const list: unknown[] = Array.isArray(steps) ? steps : [];
    const at = list.flatMap((step, index) =>
      isMapping(step) && get(step, "name") === RUSTUP_METADATA_STEP ? [index] : [],
    );
    const index = at[0];
    if (at.length !== 1 || index === undefined) {
      errors.push(`web: ${name} requires exactly one Rustup metadata step`);
      continue;
    }
    const step = list[index] as Mapping;
    pinned.add(step);
    const preamble = TIMED_RUSTUP_JOBS.includes(name) ? timingPreamble(name, index + 1) : "";
    if (
      !pyEq(Object.keys(step).sort(), ["name", "run"]) ||
      stepRun(step) !== preamble + RUSTUP_METADATA_RUN
    ) {
      errors.push(
        `web: ${name} Rustup metadata must build and run the locked xtask helper without toolchain auto-install`,
      );
    }
    const install = pySplitlines(stepRun(list[index - 2])).at(-1);
    const restore = list[index - 1];
    const sqlite = list[index + 1];
    if (
      install !== TOOLCHAIN_INSTALL ||
      !isMapping(restore) ||
      get(restore, "name") !== "Restore Cargo downloads" ||
      !/^actions\/cache(\/restore)?@/.test(String(get(restore, "uses", ""))) ||
      !pyEq(
        get(isMapping(get(restore, "with")) ? (get(restore, "with") as Mapping) : {}, "path"),
        CARGO_DOWNLOADS,
      ) ||
      !isMapping(sqlite) ||
      get(sqlite, "id") !== "sqlite"
    ) {
      errors.push(
        `web: ${name} Rustup metadata must follow the toolchain install and Cargo download restore and precede SQLite preparation`,
      );
    }
  }
  for (const [name, job] of Object.entries(jobs)) {
    const steps = isMapping(job) ? get(job, "steps") : undefined;
    const list: unknown[] = Array.isArray(steps) ? steps : [];
    if (
      list.some((step) => !pinned.has(step) && stepRun(step).includes("prepare-rustup-ci-metadata"))
    ) {
      errors.push(`web: ${name} may prepare Rustup metadata only in its pinned step`);
    }
    const allowed = PYTHON_LINES[name] ?? [];
    const python =
      PYTHON.test(JSON.stringify(without(job, "steps"))) ||
      list.some(
        (step) =>
          PYTHON.test(JSON.stringify(without(step, "run"))) ||
          pySplitlines(stepRun(step)).some(
            (line) => PYTHON.test(line) && !allowed.includes(line.trim()),
          ),
      );
    if (python)
      errors.push(`web: ${name} may not install or run python3 beyond its registered callers`);
  }
  return errors;
}

// The install smoke helpers are DB- and Docker-free, so web-static owns their
// strict types and unit tests.
const INSTALL_SMOKE_STEP = "Install smoke helper types and unit tests";
const INSTALL_SMOKE_RUN =
  "set -euo pipefail\n" +
  "bun --bun x --no-install tsc -p tools/install-smoke/tsconfig.json\n" +
  "bun test ./tools/install-smoke/\n";

export function verifyWebStaticInstallSmokeJobs(jobs: Mapping): string[] {
  const job = get(jobs, "web-static");
  const steps = isMapping(job) ? get(job, "steps") : undefined;
  const list: unknown[] = Array.isArray(steps) ? steps : [];
  const found = list.filter((step) => isMapping(step) && get(step, "name") === INSTALL_SMOKE_STEP);
  const step = found[0];
  return found.length === 1 &&
    isMapping(step) &&
    pyEq(Object.keys(step).sort(), ["name", "run"]) &&
    get(step, "run") === INSTALL_SMOKE_RUN
    ? []
    : ["web: web-static must type-check and test the install smoke helpers"];
}
