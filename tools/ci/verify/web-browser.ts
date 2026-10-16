// web.yml browser budget and the current-run native build handoff: the only
// admitted cross-job native consumer, bound to this run's producer artifact.
// The handoff check also pins the native jobs' Rustup metadata preparation
// and that no job uses python.
import { get, has, isMapping, pyContains, pyEq, pySplitlines, type Mapping } from "./py.ts";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { rawBlockScalar } from "./raw-yaml.ts";
import { get as safeGet } from "./load.ts";
import { gatedWorkflowJobs } from "./registry.ts";
import { HOST_APT_PREPARATION_RUN, readUtf8, type VerifyContext } from "./rust-shared.ts";

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
export const DOWNLOAD_PIN = "actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333";
const UPLOAD_PIN = "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9";
export const HOST_BROWSER_IF = "contains(fromJSON('[0,3,5]'), matrix.shard)";
export const IMAGE_BROWSER_IF = "!" + HOST_BROWSER_IF;
const hostBrowserStep = (step: Mapping, job: string): Mapping =>
  job === "workspace-browser-shard" && step.if === HOST_BROWSER_IF
    ? Object.fromEntries(Object.entries(step).filter(([key]) => key !== "if"))
    : step;

export function verifyWebContainerHandoffJobs(jobs: Mapping): string[] {
  const errors: string[] = [];
  const require = (value: boolean, message: string) => {
    if (!value) errors.push("web: container browser " + message);
  };
  const producer = jobs["workspace-browser-container-build"];
  const consumer = jobs["workspace-browser-shard"];
  const steps = (job: unknown): Mapping[] =>
    isMapping(job) && Array.isArray(job.steps) ? job.steps.filter(isMapping) : [];
  const ps = steps(producer),
    cs = steps(consumer);
  const command = (phase: string) =>
    'bun --no-env-file tools/web-e2e/run-web-e2e.ts container-browser "$GITHUB_WORKSPACE" ' +
    phase +
    ' --image-pin "$FVOCI_CI_WEB_IMAGE" --sdk-sha256 "$FVOCI_CI_WEB_SDK_SHA256"';
  require(isMapping(producer) &&
    producer.needs === "ci-plan" &&
    producer.if === "needs.ci-plan.outputs.select_workspace_browser_shard == 'true'" &&
    producer["runs-on"] === "ubuntu-26.04" &&
    producer["timeout-minutes"] === 20 &&
    !has(producer, "continue-on-error"), "fixed selected producer and budget required");
  require(ps.length === 4 &&
    ps.every(
      unmasked,
    ), "producer must run checkout/setup/prepare/publish without host preparation or bypass");
  const prepare = ps.find((step) => step.id === "prepare");
  require(prepare?.run ===
    command(
      '--prepare --handoff "$RUNNER_TEMP/fvoci-browser-container-packet"',
    ), "shared explicit prepare command required");
  const publish = ps.find((step) => step.id === "publish");
  require(publish?.uses === UPLOAD_PIN &&
    pyEq(publish.with, {
      name: "web-browser-container-build-${{ github.run_attempt }}",
      path: "${{ runner.temp }}/fvoci-browser-container-packet/handoff.json\n${{ runner.temp }}/fvoci-browser-container-packet/payload.tar\n",
      "if-no-files-found": "error",
      "retention-days": 1,
    }) &&
    isMapping(producer) &&
    pyEq(producer.outputs, {
      artifact_id: "${{ steps.publish.outputs.artifact-id }}",
      handoff_sha256: "${{ steps.prepare.outputs.handoff_sha256 }}",
    }), "complete immutable packet and digest outputs required");
  require(isMapping(consumer) &&
    pyEq(consumer.needs, [
      "ci-plan",
      "workspace-browser-build",
      "workspace-browser-container-build",
    ]), "both independent host/image producers must be required");
  for (const list of [ps, cs]) {
    const checkout = list.find((step) => usesPrefix(step, "actions/checkout@"));
    require(pyEq(checkout?.with, {
      "persist-credentials": false,
      "fetch-depth": 0,
    }), "exact source and image history checkout required");
  }
  const download = cs.filter(
    (step) =>
      isMapping(step.with) &&
      step.with["artifact-ids"] ===
        "${{ needs.workspace-browser-container-build.outputs.artifact_id }}",
  );
  require(download.length === 1 &&
    download[0] !== undefined &&
    download[0].uses === DOWNLOAD_PIN &&
    download[0].if === IMAGE_BROWSER_IF &&
    pyEq(download[0].with, {
      "artifact-ids": "${{ needs.workspace-browser-container-build.outputs.artifact_id }}",
      "merge-multiple": true,
      path: "${{ runner.temp }}/fvoci-browser-container-packet",
    }), "image shards require the successful current-run immutable artifact ID");
  const admit = cs.find(
    (step) => step.name === "Require a successful exact container browser producer artifact",
  );
  require(admit?.if === IMAGE_BROWSER_IF &&
    admit.run === "bun --no-env-file tools/web-e2e/run-web-e2e.ts require-browser-producer" &&
    pyEq(admit.env, {
      PRODUCER_RESULT: "${{ needs.workspace-browser-container-build.result }}",
      PRODUCER_ARTIFACT: "${{ needs.workspace-browser-container-build.outputs.artifact_id }}",
      PRODUCER_DIGEST: "${{ needs.workspace-browser-container-build.outputs.handoff_sha256 }}",
    }), "producer success and artifact digest admission required");
  const runtime = cs.find((step) => step.id === "container_browser");
  require(runtime?.if === IMAGE_BROWSER_IF &&
    runtime.run ===
      command(
        '--consume ${{ matrix.shard }} --handoff "$RUNNER_TEMP/fvoci-browser-container-packet" --handoff-sha256 "$FVOCI_WEB_BUILD_HANDOFF_SHA256"',
      ) &&
    pyEq(runtime.env, {
      FVOCI_WEB_BUILD_HANDOFF_SHA256:
        "${{ needs.workspace-browser-container-build.outputs.handoff_sha256 }}",
    }) &&
    !has(runtime, "continue-on-error"), "one complete image shard after packet admission required");
  const first = cs.findIndex((step) => stepRun(step).startsWith("rustup toolchain install")),
    last = cs.findIndex((step) => step.id === "browser");
  require(first >= 0 && last > first, "host-only complete runtime must remain");
  for (const step of cs.slice(first, last + 1))
    if (step.id !== "sqlite_diagnostics")
      require(step.if ===
        HOST_BROWSER_IF, "host preparation and assertions restricted exactly to shards0/3/5");
  require(cs.indexOf(admit as Mapping) < cs.indexOf(download[0] as Mapping) &&
    cs.indexOf(download[0] as Mapping) <
      cs.indexOf(runtime as Mapping), "admission/download/runtime ordering required");
  return errors;
}

// The selected runner owns the existing main/lib/install/engine compiler stages
// and qualifies their complete packet once in this job. No intermediate target
// artifact or normalized checkout timestamp is needed inside one producer.
export function verifyCollaborationPreparationJobs(jobs: Mapping): string[] {
  const errors: string[] = [];
  const require = (ok: boolean, message: string) => {
    if (!ok) errors.push("web: collaboration preparation " + message);
  };
  for (const name of ["collaboration-main-build", "collaboration-test-build"])
    require(!has(jobs, name), "obsolete preparation job must be absent: " + name);
  const producer = safeGet(jobs, "collaboration-build");
  require(isMapping(producer) &&
    safeGet(producer, "needs") === "ci-plan", "single producer needs only ci-plan");
  const value = safeGet(producer, "steps");
  const steps = Array.isArray(value)
    ? value.flatMap((step) => (isMapping(step) ? [step] : []))
    : [];
  require(!steps.some((step) => {
    const run = safeGet(step, "run");
    return typeof run === "string" && /\btouch\b/.test(run);
  }), "long-term seed must retain checkout source times");
  require(!steps.some((step) =>
    usesPrefix(step, "actions/download-artifact@"),
  ), "single producer cannot borrow an intermediate target");
  const at = steps.findIndex((step) => safeGet(step, "id") === "prepare");
  const prepare = steps[at];
  require(prepare !== undefined &&
    unmasked(prepare) &&
    pyEq(Object.keys(prepare).sort(), ["env", "id", "name", "run"]) &&
    pyEq(safeGet(prepare, "env"), { FVOCI_E2E_PENDING: "1" }) &&
    safeGet(prepare, "run") ===
      "set -euo pipefail\n" +
        'export FVOCI_SELECTED_CI_OUTPUT="$RUNNER_TEMP/fvoci-selected-current"\n' +
        'export FVOCI_SELECTED_CI_SQLITE_PARENT="$RUNNER_TEMP/fvoci-sqlite"\n' +
        'export FVOCI_WEB_BUILD_HANDOFF="$RUNNER_TEMP/fvoci-web-build-handoff"\n' +
        'mkdir -m 0700 "$FVOCI_SELECTED_CI_OUTPUT"\n' +
        "bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-prepare-selected\n", "complete unmasked compiler and packet qualification in one owned producer");
  for (const step of steps.filter((step) => usesPrefix(step, "actions/cache"))) {
    const withs = safeGet(step, "with");
    const path = safeGet(withs, "path");
    const key = safeGet(withs, "key");
    if (typeof path === "string" && path.trim() === "~/.cargo/registry\n~/.cargo/git") continue;
    require(isMapping(withs) &&
      path === "target" &&
      !has(withs, "restore-keys") &&
      typeof key === "string" &&
      key.includes("steps.sqlite.outputs.cache_identity") &&
      steps.indexOf(step) >
        steps.findIndex(
          (item) => safeGet(item, "id") === "sqlite",
        ), "only identity-bound root seed after SQLite preparation; no helper fallback cache");
  }
  return errors;
}

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
  require(get(producer, "needs") === "ci-plan", "producer needs only ci-plan");
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
  return [...errors, ...verifyCollaborationPreparationJobs(jobs)];
}

export function verifyWebBuildHandoff(ctx: VerifyContext): string[] {
  const jobs = webWorkflowJobs(ctx);
  if (jobs === null) return [];
  const workflow = ctx.workflows[WEB_WORKFLOW_FILE];
  return [
    ...verifyWebBuildHandoffJobs(jobs),
    ...verifyNoRunScopedCacheKeys(jobs),
    ...verifyWebRustupMetadataJobs(jobs, isMapping(workflow) ? workflow : {}),
    ...verifyWebStaticInstallSmokeJobs(jobs),
    ...verifyWebHostInputsJobs(jobs),
    ...verifyWebContainerHandoffJobs(jobs),
  ];
}

// A run-scoped key can only carry outputs to a later job of the same run, and
// GitHub may evict that entry before the consumer restores it.
const RUN_SCOPED = /\bgithub\.run_(id|attempt)\b/;
export function verifyNoRunScopedCacheKeys(jobs: Mapping): string[] {
  return Object.entries(jobs).flatMap(([name, job]) => {
    const steps = isMapping(job) ? get(job, "steps") : undefined;
    const list: unknown[] = Array.isArray(steps) ? steps : [];
    return list.some(
      (step) =>
        isMapping(step) &&
        usesPrefix(step, "actions/cache") &&
        RUN_SCOPED.test(JSON.stringify(get(step, "with") ?? null)),
    )
      ? [`web: ${name} must hand off run-scoped outputs by artifact, not by a cache key`]
      : [];
  });
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
const TOOLCHAIN_INSTALL = "rustup toolchain install 1.99.0 --profile minimal --component clippy";
const WEB_NATIVE_JOBS = [
  "workspace-browser-build",
  "workspace-browser-shard",
  "collaboration-build",
  ...WEB_COLLAB_LANES.map((lane) => lane.name),
];

export const WEB_SQLITE_JOBS = ["web-checks", "web-native-checks", ...WEB_NATIVE_JOBS];
// The same complete host preparation precedes every Web SQLite/cache identity.
const WEB_SQLITE_RUN = String.raw`set -euo pipefail
# Select the signed Ubuntu primary archive only on x64; preserve ARM mirrors.
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 26.04 ]]
case "$(dpkg --print-architecture)" in
  amd64)
    test -f /etc/apt/apt-mirrors.txt
    grep -Fq 'mirror+file:/etc/apt/apt-mirrors.txt' /etc/apt/sources.list.d/ubuntu.sources
    grep -Fq 'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' /etc/apt/sources.list.d/ubuntu.sources
    test -s /usr/share/keyrings/ubuntu-archive-keyring.gpg
    printf '%s\n' 'https://archive.ubuntu.com/ubuntu/' | sudo tee /etc/apt/apt-mirrors.txt >/dev/null
    ;;
  arm64) ;;
  *) exit 1 ;;
esac
${HOST_APT_PREPARATION_RUN}mkdir "$RUNNER_TEMP/fvoci-sqlite"
dpkg-query -W > "$RUNNER_TEMP/fvoci-sqlite/build-packages.txt"
bash scripts/prepare-sqlite-ci.sh --parent "$RUNNER_TEMP/fvoci-sqlite" \
  --github-env "$GITHUB_ENV" --github-output "$GITHUB_OUTPUT"
`;
const SQLITE_HASH_RUN_SHA256 = "a4b8443336684c2b7b3222bfd9a05ce3bf3fde5ac07c96ca0dc16fbeadb7fe62";
const SQLITE_HASH_CONDITION = "${{ always() && steps.sqlite.outcome == 'success' }}";
const SQLITE_HASH_UPLOAD_CONDITION =
  "${{ always() && steps.sqlite_diagnostics.outputs.written == 'true' }}";
const SQLITE_HASH_UPLOAD: Mapping = {
  name: "Retain SQLite input hashes",
  if: SQLITE_HASH_UPLOAD_CONDITION,
  uses: UPLOAD_PIN,
  with: {
    name: "sqlite-input-hashes-${{ github.job }}-${{ strategy.job-index }}-${{ github.run_attempt }}",
    path: "${{ runner.temp }}/fvoci-sqlite-input-hashes/hashes.json",
    "if-no-files-found": "warn",
    "retention-days": 3,
  },
};
function isSQLiteHashUpload(step: Mapping): boolean {
  return pyEq(step, SQLITE_HASH_UPLOAD);
}

export function verifyWebHostInputsJobs(jobs: Mapping): string[] {
  const errors: string[] = [];
  for (const name of WEB_SQLITE_JOBS) {
    const job = get(jobs, name);
    const raw = isMapping(job) ? get(job, "steps") : undefined;
    const steps = Array.isArray(raw) ? raw.filter(isMapping) : [];
    const sqlites = steps.filter((step) => get(step, "id") === "sqlite");
    const sqlite = sqlites[0];
    const at = sqlite ? steps.indexOf(sqlite) : -1;
    if (
      sqlites.length !== 1 ||
      !sqlite ||
      !pyEq(hostBrowserStep(sqlite, name), {
        name: "Prepare pinned SQLite root build inputs",
        id: "sqlite",
        env: { LIBCLANG_PATH: "/usr/lib/llvm-18/lib" },
        run: WEB_SQLITE_RUN,
      })
    )
      errors.push(
        `web: ${name} requires complete unmasked host preparation before whole SQLite inputs`,
      );
    const records = steps.filter((step) => get(step, "id") === "sqlite_diagnostics");
    const record = records[0];
    if (
      records.length !== 1 ||
      !record ||
      steps.indexOf(record) !== at + 1 ||
      !pyEq(Object.keys(record).sort(), ["id", "if", "name", "run"]) ||
      get(record, "name") !== "Record SQLite input hashes" ||
      get(record, "if") !== SQLITE_HASH_CONDITION ||
      createHash("sha256").update(stepRun(record)).digest("hex") !== SQLITE_HASH_RUN_SHA256
    )
      errors.push(
        `web: ${name} requires the bounded hash-only diagnostic after successful SQLite preparation`,
      );
    const uploads = steps.filter((step) => get(step, "name") === "Retain SQLite input hashes");
    if (
      uploads.length !== 1 ||
      !uploads[0] ||
      !isSQLiteHashUpload(uploads[0]) ||
      steps.at(-1) !== uploads[0]
    )
      errors.push(`web: ${name} retains only the safe hash diagnostic after mandatory steps`);
  }
  return errors;
}

// No web.yml job installs or runs python.
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
  for (const name of WEB_NATIVE_JOBS) {
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
    const original = list[index] as Mapping;
    const step = hostBrowserStep(original, name);
    pinned.add(original);
    if (!pyEq(Object.keys(step).sort(), ["name", "run"]) || stepRun(step) !== RUSTUP_METADATA_RUN) {
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
    // Job keys, step fields and every run line.
    if (PYTHON.test(JSON.stringify(job)))
      errors.push(`web: ${name} may not install or run python3`);
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
