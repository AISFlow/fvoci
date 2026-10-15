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

export const WEB_BUILDER_SHELL = [
  "docker run",
  "--rm",
  "--init",
  "--name fvoci-web-${{ github.run_id }}-${{ github.run_attempt }}-${{ github.job }}-${{ strategy.job-index }}",
  "--network host",
  "--ipc host",
  "--pid host",
  "--user 1001:1001",
  "--group-add ${{ steps.builder.outputs.docker_gid }}",
  "--label fvoci.web.run=${{ github.run_id }}",
  "--label fvoci.web.attempt=${{ github.run_attempt }}",
  "--label fvoci.web.source=${{ github.sha }}",
  "--mount type=bind,source=${{ github.workspace }},target=${{ github.workspace }}",
  "--mount type=bind,source=${{ runner.temp }},target=${{ runner.temp }}",
  "--mount type=bind,source=${{ runner.temp }}/fvoci-web-builder-state/cargo,target=/home/runner/.cargo",
  "--mount type=bind,source=${{ runner.temp }}/fvoci-web-builder-state/rustup,target=/home/runner/.rustup",
  "--mount type=bind,source=${{ runner.temp }}/fvoci-web-builder/docker-client,target=/usr/local/bin/docker,readonly",
  "--mount type=bind,source=/var/run/docker.sock,target=/var/run/docker.sock",
  "--workdir ${{ github.workspace }}",
  "--env HOME=/home/runner",
  "--env CARGO_HOME=/home/runner/.cargo",
  "--env RUSTUP_HOME=/home/runner/.rustup",
  "--env RUSTUP_TOOLCHAIN=1.99.0",
  "--env PATH=/home/runner/.cargo/bin:/usr/local/bin:/usr/bin:/bin",
  "--env PLAYWRIGHT_BROWSERS_PATH=${{ runner.temp }}/fvoci-web-builder-state/browser",
  "--env TINI_KILL_PROCESS_GROUP=1",
  "--env CI",
  "--env GITHUB_ACTIONS",
  "--env GITHUB_SHA",
  "--env GITHUB_REPOSITORY",
  "--env GITHUB_RUN_ID",
  "--env GITHUB_RUN_ATTEMPT",
  "--env GITHUB_JOB",
  "--env GITHUB_REF",
  "--env RUNNER_TEMP",
  "--env RUNNER_OS",
  "--env RUNNER_ARCH",
  "--env RUNNER_ENVIRONMENT",
  "--env GITHUB_WORKSPACE",
  "--env GITHUB_ENV",
  "--env GITHUB_OUTPUT",
  "--env GITHUB_STEP_SUMMARY",
  "--env LIBCLANG_PATH",
  "--env CARGO_BUILD_JOBS",
  "--env CARGO_INCREMENTAL",
  "--env CARGO_PROFILE_DEV_DEBUG",
  "--env CARGO_PROFILE_TEST_DEBUG",
  "--env SQLITE3_LIB_DIR",
  "--env SQLITE3_INCLUDE_DIR",
  "--env SQLITE3_NO_PKG_CONFIG",
  "--env SQLITE3_STATIC",
  "--env PRODUCER_RESULT",
  "--env PRODUCER_ARTIFACT",
  "--env PRODUCER_DIGEST",
  "--env FVOCI_WEB_BUILD_PHASE",
  "--env FVOCI_E2E_PENDING",
  "--env FVOCI_COLLAB_LANE",
  "--env FVOCI_WEB_BUILD_HANDOFF_SHA256",
  "--env FVOCI_CLOSED_INSTALL_RECEIPT ${{ needs.web-builder.outputs.image_id }} bash",
  "--noprofile",
  '--norc -e -o pipefail "{0}"',
].join(" ");
export const WEB_BAKED_CHROMIUM_RUN =
  'cd apps/web\ntest "$(bun --bun x --no-install playwright --version)" = \'Version 1.64.0\'\nchromium="$(bun -e \'import {chromium} from "@playwright/test"; process.stdout.write(chromium.executablePath())\')"\ncase "$chromium" in "$PLAYWRIGHT_BROWSERS_PATH"/*) ;; *) exit 1 ;; esac\ntest -x "$chromium"\nlibraries="$(ldd "$chromium")"\n[[ "$libraries" != *\'not found\'* ]]\n';
const WEB_BUILDER_DOWNLOAD: Mapping = {
  name: "Download this run exact scanned Web builder",
  uses: "actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333",
  with: {
    "artifact-ids": "${{ needs.web-builder.outputs.artifact_id }}",
    "merge-multiple": true,
    path: "${{ runner.temp }}/fvoci-web-builder",
  },
};
const WEB_BUILDER_ADMIT_SHA256 = "a52aaf334110712557fd06bdef6e24020c6065dda564e92919d0b9cda8acd3cc";
const WEB_BUILDER_RETIRE_SHA256 =
  "804363947c93c842c3d2a69d7845e3c65e7bb5a7a987ea2122747e8e2ddeee18";
const WEB_BUILDER_PRODUCER_SHA256 =
  "dc670da6d105b8fc03e7e2092fa8e2767f3942b001cb018d06a6602c3b549827";
function isWebBuilderDownload(step: Mapping): boolean {
  return pyEq(step, WEB_BUILDER_DOWNLOAD);
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

export const COLLABORATION_PREPARATION_JOBS = [
  "collaboration-main-build",
  "collaboration-test-build",
] as const;
// Each phase hands its compiled target to the next job as a run-scoped
// artifact: a cache entry can be evicted between save and restore in one run.
// The former cache key's arch/toolchain/SHA/attempt/SQLite identity travels as
// a producer output that the consumer must reproduce; the run ID is the
// artifact's own scope.
type NativePhase = "main" | "tests";
export const COLLABORATION_NATIVE_IDENTITY =
  "${{ runner.arch }}-1.99.0-${{ github.sha }}-${{ github.run_attempt }}-" +
  "${{ steps.sqlite.outputs.cache_identity }}";
export const COLLABORATION_NATIVE_OUTPUTS = {
  artifact_id: "${{ steps.publish_native.outputs.artifact-id }}",
  target_sha256: "${{ steps.native.outputs.target_sha256 }}",
  native_identity: COLLABORATION_NATIVE_IDENTITY,
};
const nativeArchive = (phase: NativePhase) => `"$RUNNER_TEMP/fvoci-native-${phase}/target.tar.zst"`;
// tar keeps the file modes and mtimes Cargo relies on; the artifact zip does not.
export function collaborationPackRun(phase: NativePhase): string {
  return (
    "set -euo pipefail\n" +
    `mkdir -m 0700 "$RUNNER_TEMP/fvoci-native-${phase}"\n` +
    `tar --posix -cf - target | zstd -T0 --no-progress -o ${nativeArchive(phase)}\n` +
    `sha256="$(sha256sum ${nativeArchive(phase)} | cut -d ' ' -f 1)"\n` +
    `printf 'target_sha256=%s\\n' "$sha256" >> "$GITHUB_OUTPUT"\n`
  );
}
export function collaborationUploadWith(phase: NativePhase): Mapping {
  return {
    name: `web-native-${phase}-\${{ github.run_attempt }}`,
    path: `\${{ runner.temp }}/fvoci-native-${phase}/target.tar.zst`,
    "compression-level": 0,
    "if-no-files-found": "error",
    "retention-days": 1,
  };
}
export function collaborationRequireEnv(producer: string): Mapping {
  return {
    PRODUCER_RESULT: `\${{ needs.${producer}.result }}`,
    PRODUCER_ARTIFACT: `\${{ needs.${producer}.outputs.artifact_id }}`,
    PRODUCER_DIGEST: `\${{ needs.${producer}.outputs.target_sha256 }}`,
    PRODUCER_IDENTITY: `\${{ needs.${producer}.outputs.native_identity }}`,
    CURRENT_IDENTITY: COLLABORATION_NATIVE_IDENTITY,
  };
}
export const COLLABORATION_REQUIRE_RUN =
  "set -euo pipefail\n" +
  '[[ "$PRODUCER_RESULT" == success && "$PRODUCER_ARTIFACT" =~ ^[0-9]+$ && "$PRODUCER_DIGEST" =~ ^[0-9a-f]{64}$ ]]\n' +
  '[[ "$CURRENT_IDENTITY" =~ ^[A-Z0-9]+-1\\.99\\.0-[0-9a-f]{40}-[1-9][0-9]*-[0-9a-f]{64}$ && "$PRODUCER_IDENTITY" == "$CURRENT_IDENTITY" ]]\n';
export function collaborationDownloadWith(producer: string, phase: NativePhase): Mapping {
  return {
    "artifact-ids": `\${{ needs.${producer}.outputs.artifact_id }}`,
    "merge-multiple": true,
    path: `\${{ runner.temp }}/fvoci-native-${phase}`,
  };
}
export function collaborationRestoreRun(phase: NativePhase): string {
  return (
    "set -euo pipefail\n" +
    `archive=${nativeArchive(phase)}\n` +
    `printf '%s  %s\\n' "$PRODUCER_DIGEST" "$archive" | sha256sum --check --strict --quiet -\n` +
    'zstd -dc --no-progress -- "$archive" | tar -xf -\n' +
    'rm -- "$archive"\n'
  );
}
export const COLLABORATION_PREPARATION_RUNS = {
  "collaboration-main-build":
    "cargo build --locked --offline --features db-tests,api-schema \\\n" +
    "  --bin fvoci-server --bin fvoci-migrate --bin fvoci-e2e-fixture\n",
  "collaboration-test-build":
    "cargo test --locked --offline --features db-tests,api-schema --lib --no-run\n" +
    "cargo test --locked --offline --features db-tests,api-schema --test selected_install_lifetime --no-run\n",
};

export const COLLABORATION_SOURCE_MTIME_RUN =
  "set -euo pipefail\n" +
  'source_epoch="$(git show -s --format=%ct HEAD)"\n' +
  'git ls-files -z | xargs -0 touch -h -d "@$source_epoch" --\n';

// Long-term seeds keep checkout mtimes so Cargo recompiles workspace sources.
// Only exact current-run targets can use normalized mtimes; the original
// producer still qualifies all outputs before publishing a selected packet.
export function verifyCollaborationPreparationJobs(jobs: Mapping): string[] {
  const errors: string[] = [];
  const require = (ok: boolean, message: string) => {
    if (!ok) errors.push("web: collaboration preparation " + message);
  };
  const list = (name: string): Mapping[] => {
    const value = safeGet(safeGet(jobs, name), "steps");
    return Array.isArray(value) ? value.flatMap((step) => (isMapping(step) ? [step] : [])) : [];
  };
  require(!list("collaboration-main-build").some((step) => {
    const run = safeGet(step, "run");
    return (
      safeGet(step, "name") === "Normalize tracked source mtimes for native cache reuse" ||
      (typeof run === "string" && /\btouch\b/.test(run))
    );
  }), "long-term seed must retain checkout source times");
  for (const [index, name] of COLLABORATION_PREPARATION_JOBS.entries()) {
    const job = safeGet(jobs, name);
    require(isMapping(job), `${name} must exist`);
    require(pyEq(
      safeGet(job, "needs"),
      index === 0
        ? ["ci-plan", "web-builder"]
        : ["ci-plan", "collaboration-main-build", "web-builder"],
    ), "ordered preparation dependencies");
    const steps = list(name);
    const compiles = steps.filter((step) => safeGet(step, "name") === "Compile native preparation");
    const compile = compiles[0];
    require(compiles.length === 1 &&
      compile !== undefined &&
      unmasked(compile) &&
      pyEq(Object.keys(compile).sort(), ["name", "run", "shell"]) &&
      safeGet(compile, "run") ===
        "set -euo pipefail\n" +
          COLLABORATION_PREPARATION_RUNS[name], "complete unmasked compiler commands");
    const phase = index === 0 ? "main" : "tests";
    const packs = steps.filter(
      (step) => safeGet(step, "name") === "Pack current native preparation",
    );
    const pack = packs[0];
    require(packs.length === 1 &&
      pack !== undefined &&
      unmasked(pack) &&
      pyEq(Object.keys(pack).sort(), ["id", "name", "run", "shell"]) &&
      safeGet(pack, "id") === "native" &&
      steps.indexOf(pack) > steps.indexOf(compile as Mapping) &&
      safeGet(pack, "run") ===
        collaborationPackRun(phase), "pack the compiled target with its modes, mtimes and digest");
    const saves = steps.filter(
      (step) => safeGet(step, "name") === "Save current native preparation",
    );
    const save = saves[0];
    require(saves.length === 1 &&
      save !== undefined &&
      unmasked(save) &&
      steps.indexOf(save) > steps.indexOf(pack as Mapping) &&
      safeGet(save, "id") === "publish_native" &&
      safeGet(save, "uses") === UPLOAD_PIN &&
      pyEq(
        safeGet(save, "with"),
        collaborationUploadWith(phase),
      ), "save only the packed current-run target as a run-scoped artifact");
    require(pyEq(
      safeGet(job, "outputs"),
      COLLABORATION_NATIVE_OUTPUTS,
    ), "outputs the artifact ID, digest and arch/SHA/attempt/SQLite identity");
    require(steps.filter(
      (step) => usesPrefix(step, "actions/upload-artifact@") && !isSQLiteHashUpload(step),
    ).length === 1, "preparation cannot publish qualified packets");
  }
  for (const [name, producer, phase] of [
    ["collaboration-test-build", "collaboration-main-build", "main"],
    ["collaboration-build", "collaboration-test-build", "tests"],
  ] as const) {
    const steps = list(name);
    const compileAt = steps.findIndex(
      (step) =>
        safeGet(step, "name") === "Compile native preparation" || safeGet(step, "id") === "prepare",
    );
    const sqliteAt = steps.findIndex((step) => safeGet(step, "id") === "sqlite");
    const checks = steps.filter(
      (step) => safeGet(step, "name") === "Require exact current native preparation",
    );
    const check = checks[0];
    require(checks.length === 1 &&
      check !== undefined &&
      unmasked(check) &&
      pyEq(Object.keys(check).sort(), ["env", "name", "run"]) &&
      pyEq(safeGet(check, "env"), collaborationRequireEnv(producer)) &&
      safeGet(check, "run") === COLLABORATION_REQUIRE_RUN &&
      sqliteAt >= 0 &&
      steps.indexOf(check) >
        sqliteAt, "producer must match this run's arch/SHA/attempt/SQLite identity after SQLite preparation");
    const downloads = steps.filter(
      (step) => usesPrefix(step, "actions/download-artifact@") && !isWebBuilderDownload(step),
    );
    const download = downloads[0];
    require(downloads.length === 1 &&
      download !== undefined &&
      unmasked(download) &&
      safeGet(download, "uses") === DOWNLOAD_PIN &&
      pyEq(safeGet(download, "with"), collaborationDownloadWith(producer, phase)) &&
      steps.indexOf(download) >
        steps.indexOf(
          check as Mapping,
        ), "exact current-run producer artifact ID after the identity check");
    const restores = steps.filter(
      (step) => safeGet(step, "name") === "Restore current native preparation",
    );
    const restore = restores[0];
    require(restores.length === 1 &&
      restore !== undefined &&
      unmasked(restore) &&
      pyEq(Object.keys(restore).sort(), ["env", "name", "run"]) &&
      pyEq(safeGet(restore, "env"), {
        PRODUCER_DIGEST: `\${{ needs.${producer}.outputs.target_sha256 }}`,
      }) &&
      safeGet(restore, "run") === collaborationRestoreRun(phase) &&
      steps.indexOf(restore) > steps.indexOf(download as Mapping) &&
      steps.indexOf(restore) <
        compileAt, "digest-verified current-run artifact restore before compilation");
    const normalizers = steps.filter(
      (step) => safeGet(step, "name") === "Normalize tracked source mtimes for native cache reuse",
    );
    const normalize = normalizers[0];
    require(normalizers.length === 1 &&
      normalize !== undefined &&
      unmasked(normalize) &&
      pyEq(Object.keys(normalize).sort(), ["name", "run"]) &&
      safeGet(normalize, "run") === COLLABORATION_SOURCE_MTIME_RUN &&
      steps.indexOf(normalize) > steps.indexOf(restore as Mapping) &&
      steps.indexOf(normalize) <
        compileAt, "deterministic tracked source mtimes after exact current-run artifact restore before compilation");
    require(steps
      .filter((step) => usesPrefix(step, "actions/cache"))
      .every((step) => {
        const path = safeGet(safeGet(step, "with"), "path");
        return typeof path === "string" && path.trim() === CARGO_DOWNLOADS.trim();
      }), "no long-term native cache in normalized jobs");
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
const afterInstall = ["ci-plan", "collaboration-build", "collaboration-install-on", "web-builder"];
const receiptEnv = { FVOCI_CLOSED_INSTALL_RECEIPT: CLOSED_INSTALL_RECEIPT };
const bothDownloads = [PRODUCER_DOWNLOAD_WITH, RECEIPT_DOWNLOAD_WITH];
export const WEB_COLLAB_LANES: readonly Lane[] = [
  {
    name: "collaboration-install-on",
    token: "install/on",
    needs: ["ci-plan", "collaboration-build", "web-builder"],
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
  require(pyEq(get(producer, "needs"), [
    "ci-plan",
    ...COLLABORATION_PREPARATION_JOBS,
    "web-builder",
  ]), "producer needs ci-plan and successful preparation");
  const lanes = WEB_COLLAB_LANES.map((lane) => ({ lane, job: jobOf(lane.name) }));
  for (const { lane, job } of lanes) {
    require(pyEq(get(job, "needs"), lane.needs), "consumer needs successful registered producer");
  }
  const stepsByJob = new Map<string, Mapping[]>();
  for (const [name, job] of [
    ["collaboration-build", producer] as const,
    ...COLLABORATION_PREPARATION_JOBS.map((name) => [name, jobOf(name)] as const),
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
    const downloads = steps.filter(
      (step) => usesPrefix(step, "actions/download-artifact@") && !isWebBuilderDownload(step),
    );
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
    ...verifyWebBuilderJobs(jobs),
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
const CARGO_DOWNLOADS =
  "${{ runner.temp }}/fvoci-web-builder-state/cargo/registry\n${{ runner.temp }}/fvoci-web-builder-state/cargo/git\n";
const TOOLCHAIN_INSTALL = "test \"$(rustc --version | cut -d' ' -f2)\" = 1.99.0";
const UNTIMED_RUSTUP_JOBS = ["workspace-browser-build", "workspace-browser-shard"];
const TIMED_RUSTUP_JOBS = [
  ...COLLABORATION_PREPARATION_JOBS,
  "collaboration-build",
  ...WEB_COLLAB_LANES.map((lane) => lane.name),
];

export const WEB_SQLITE_JOBS = [
  "web-checks",
  "web-native-checks",
  ...UNTIMED_RUSTUP_JOBS,
  ...TIMED_RUSTUP_JOBS,
];
// The same complete host preparation precedes every Web SQLite/cache identity.
const WEB_SQLITE_RUN =
  'set -euo pipefail\ntest "$(rustc --version | cut -d\' \' -f2)" = 1.99.0\ntest "$(bun --version)" = 1.4.3\ntest "$(dpkg-query -W -f=\'${Version}\' libclang-18-dev)" = 1:18.1.8-20ubuntu8\ntest -f "$LIBCLANG_PATH/libclang.so"\nmkdir "$RUNNER_TEMP/fvoci-sqlite"\ndpkg-query -W > "$RUNNER_TEMP/fvoci-sqlite/build-packages.txt"\nbash scripts/prepare-sqlite-ci.sh --parent "$RUNNER_TEMP/fvoci-sqlite" \\\n  --github-env "$GITHUB_ENV" --github-output "$GITHUB_OUTPUT"\n';
const SQLITE_HASH_RUN_SHA256 = "940de49368cfbd4955078f899e8f46c3233b63519845764e3e7696a2087189af";
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

export function verifyWebBuilderJobs(jobs: Mapping): string[] {
  const errors: string[] = [];
  const digest = (step: Mapping) => createHash("sha256").update(stepRun(step)).digest("hex");
  const list = (job: unknown): Mapping[] => {
    const value = isMapping(job) ? get(job, "steps") : undefined;
    return Array.isArray(value) ? value.filter(isMapping) : [];
  };
  const producer = get(jobs, "web-builder");
  const producerSteps = list(producer);
  const build = producerSteps[2];
  const publish = producerSteps[3];
  if (
    !isMapping(producer) ||
    !pyEq(Object.keys(producer).sort(), [
      "if",
      "needs",
      "outputs",
      "runs-on",
      "steps",
      "timeout-minutes",
    ]) ||
    get(producer, "needs") !== "ci-plan" ||
    get(producer, "if") !== "needs.ci-plan.outputs.select_web_builder == 'true'" ||
    get(producer, "runs-on") !== "ubuntu-26.04" ||
    get(producer, "timeout-minutes") !== 15 ||
    !pyEq(get(producer, "outputs"), {
      artifact_id: "${{ steps.publish_builder.outputs.artifact-id }}",
      image_id: "${{ steps.builder.outputs.image_id }}",
      receipt_sha256: "${{ steps.builder.outputs.receipt_sha256 }}",
      archive_sha256: "${{ steps.builder.outputs.archive_sha256 }}",
    }) ||
    producerSteps.length !== 4 ||
    !pyEq(producerSteps[0], {
      uses: "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
      with: { "persist-credentials": false },
    }) ||
    !pyEq(producerSteps[1], {
      uses: "oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6",
      with: { "bun-version-file": ".bun-version" },
    }) ||
    !build ||
    !pyEq(Object.keys(build).sort(), ["env", "id", "name", "run"]) ||
    get(build, "id") !== "builder" ||
    get(build, "name") !== "Build and scan this source Web userspace" ||
    !pyEq(get(build, "env"), { DOCKER_CONFIG: "${{ runner.temp }}/fvoci-web-builder-docker" }) ||
    digest(build) !== WEB_BUILDER_PRODUCER_SHA256 ||
    !publish ||
    !pyEq(publish, {
      name: "Publish this run scanned Web builder",
      id: "publish_builder",
      uses: UPLOAD_PIN,
      with: {
        name: "web-builder-${{ github.run_attempt }}",
        path: "${{ runner.temp }}/fvoci-web-builder/image.tar\n${{ runner.temp }}/fvoci-web-builder/image.json\n${{ runner.temp }}/fvoci-web-builder/docker-client\n${{ runner.temp }}/fvoci-web-builder/receipt.json",
        "compression-level": 0,
        "if-no-files-found": "error",
        "retention-days": 1,
      },
    })
  )
    errors.push(
      "web: builder requires one source-scanned native image/static client, read-only provenance and unchanged budget",
    );
  for (const name of WEB_SQLITE_JOBS) {
    const job = get(jobs, name),
      steps = list(job);
    const needs = isMapping(job) ? get(job, "needs") : undefined;
    const downloads = steps.filter(isWebBuilderDownload);
    const admits = steps.filter((step) => get(step, "id") === "builder");
    const admit = admits[0];
    const retires = steps.filter(
      (step) => get(step, "name") === "Retire this job owned Web builder",
    );
    const retire = retires[0];
    if (
      !Array.isArray(needs) ||
      needs.filter((value) => value === "web-builder").length !== 1 ||
      downloads.length !== 1 ||
      admits.length !== 1 ||
      !admit ||
      !pyEq(Object.keys(admit).sort(), ["env", "id", "name", "run"]) ||
      get(admit, "name") !== "Admit and load exact Web builder" ||
      !pyEq(get(admit, "env"), {
        BUILDER_RESULT: "${{ needs.web-builder.result }}",
        BUILDER_ARTIFACT: "${{ needs.web-builder.outputs.artifact_id }}",
        BUILDER_RECEIPT: "${{ needs.web-builder.outputs.receipt_sha256 }}",
        BUILDER_IMAGE: "${{ needs.web-builder.outputs.image_id }}",
      }) ||
      digest(admit) !== WEB_BUILDER_ADMIT_SHA256 ||
      steps.indexOf(admit) !== steps.indexOf(downloads[0] as Mapping) + 1 ||
      retires.length !== 1 ||
      !retire ||
      !pyEq(Object.keys(retire).sort(), ["env", "if", "name", "run"]) ||
      get(retire, "if") !== "always()" ||
      !pyEq(get(retire, "env"), {
        BUILDER_OWNED: "${{ steps.builder.outputs.image_owned }}",
        BUILDER_IMAGE: "${{ needs.web-builder.outputs.image_id }}",
      }) ||
      digest(retire) !== WEB_BUILDER_RETIRE_SHA256 ||
      steps.at(-2) !== retire
    )
      errors.push(
        `web: ${name} must admit/load this run builder before use and retire only its own resources`,
      );
    const browsers = steps.filter(
      (step) => get(step, "name") === "Prepare Chromium and system libraries",
    );
    const expectsBrowser = ![
      "web-checks",
      "web-native-checks",
      "collaboration-main-build",
      "collaboration-test-build",
    ].includes(name);
    if (
      browsers.length !== (expectsBrowser ? 1 : 0) ||
      browsers.some(
        (step) =>
          !pyEq(step, {
            name: "Prepare Chromium and system libraries",
            shell: WEB_BUILDER_SHELL,
            run:
              (TIMED_RUSTUP_JOBS.includes(name)
                ? timingPreamble(name, steps.indexOf(step) + 1)
                : "") + WEB_BAKED_CHROMIUM_RUN,
          }),
      )
    )
      errors.push(
        `web: ${name} must verify pinned baked Chromium and actual system libraries without installation`,
      );
    const hostNames = new Set([
      "Admit and load exact Web builder",
      "Retire this job owned Web builder",
      "Record SQLite input hashes",
      "Require exact current native preparation",
      "Restore current native preparation",
      "Normalize tracked source mtimes for native cache reuse",
    ]);
    for (const step of steps)
      if (has(step, "run")) {
        const host = hostNames.has(String(get(step, "name")));
        if (host ? has(step, "shell") : get(step, "shell") !== WEB_BUILDER_SHELL)
          errors.push(
            `web: ${name} must execute every build/input/browser call in the exact fixed userspace`,
          );
        if (!host && /(?:^|\s)(?:export\s+)?PLAYWRIGHT_BROWSERS_PATH\s*=/.test(stepRun(step)))
          errors.push(`web: ${name} cannot override its owned source-bound browser cache`);
        if (!host && admit && steps.indexOf(step) <= steps.indexOf(admit))
          errors.push(`web: ${name} cannot execute builder calls before admission`);
      }
  }
  return errors;
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
    const preamble = TIMED_RUSTUP_JOBS.includes(name) ? timingPreamble(name, at + 1) : "";
    if (
      sqlites.length !== 1 ||
      !sqlite ||
      !pyEq(sqlite, {
        name: "Prepare pinned SQLite root build inputs",
        id: "sqlite",
        env: { LIBCLANG_PATH: "/usr/lib/llvm-18/lib" },
        run: preamble + WEB_SQLITE_RUN,
        shell: WEB_BUILDER_SHELL,
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

// The collaboration jobs wrap each run in a ci-step timer named by position.
function timingPreamble(job: string, position: number): string {
  const stage = `${job}-step-${String(position).padStart(2, "0")}`;
  return (
    "fvoci_timing_started=$SECONDS\n" +
    `printf 'ci-step stage=${stage} started at=%s\\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"\n` +
    `trap 'fvoci_timing_exit=$?; printf "ci-step stage=${stage} finished at=%s elapsed_seconds=%s exit=%s\\n" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$((SECONDS - \${fvoci_timing_started}))" "$fvoci_timing_exit"; exit "$fvoci_timing_exit"' EXIT\n`
  );
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
      !pyEq(Object.keys(step).sort(), ["name", "run", "shell"]) ||
      get(step, "shell") !== WEB_BUILDER_SHELL ||
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
