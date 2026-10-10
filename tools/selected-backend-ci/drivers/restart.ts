// Same owned DB/storage server restart for the two normal selected lanes
// (postgres, sqlite). The parent lane driver calls restartSameApp after its
// successful seed diagnostics. This module never prepares a fixture, builds or
// resets a database; it owns only the second exec/server it starts.
import { deepEquals } from "bun";
import { strict as assert } from "node:assert";
import {
  chmodSync,
  closeSync,
  existsSync,
  fchmodSync,
  lstatSync,
  openSync,
  readdirSync,
  renameSync,
  statSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative } from "node:path";
import process from "node:process";
import { localAllocation } from "../admission.ts";
import { env, jsonInteger, read, root, sha } from "../io.ts";
import type { Browser, Inputs } from "../types.ts";
import {
  attachmentJson,
  checkInterrupt,
  cleanupScope,
  errorFacts,
  frameLine,
  knownBrowserCheckpoint,
  privateWrite,
  readText,
  running,
  waitFor,
} from "./common.ts";
import type { Child, Command, Json, Receipt, Row } from "./common.ts";

export const restartHelper = import.meta.path;
const title = "selected normal main restart:";
const hex40 = /^[0-9a-f]{40}$/;

export interface RestartBinding {
  runId: string;
  runAttempt: string;
  source: string;
  tree: string;
  compiledSource: string;
  backend: string;
  runRoot: string;
  parentDriverSha256: string;
  restartHelperSha256: string;
  sourceInputsSha256: string;
  artifactHashes: Record<string, string>;
  assetHashes: Record<string, string>;
  browserInputs: Browser;
  abiHashes: Record<string, string>;
}
interface Allocation {
  schema?: unknown;
  status?: unknown;
  owner?: unknown;
  executionMode?: unknown;
  localAuthorizationSha256?: unknown;
  exclusiveCIJob?: unknown;
  currentCIJobConfirmed?: unknown;
  source?: unknown;
  compiledSource?: unknown;
  tree?: unknown;
  backend?: unknown;
  binding?: unknown;
  runId?: unknown;
  runAttempt?: unknown;
}

export function validateAllocation(allocation: Allocation, binding: RestartBinding): void {
  const owner = env("FVOCI_CI_OWNER");
  assert.ok(allocation.schema === 1 && allocation.status === "GRANTED");
  assert.ok(allocation.owner === owner);
  if (allocation.executionMode === "orca-local") {
    const local = localAllocation("run");
    assert.ok(
      allocation.localAuthorizationSha256 === env("FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256"),
    );
    assert.ok(allocation.runId === local.runId && allocation.runAttempt === local.dispatchId);
  } else assert.ok(allocation.exclusiveCIJob === true);
  assert.ok(
    allocation.source === allocation.compiledSource && allocation.source === binding.source,
  );
  assert.ok(allocation.tree === binding.tree && allocation.backend === binding.backend);
  assert.ok(typeof allocation.source === "string" && hex40.test(allocation.source));
  assert.ok(typeof allocation.tree === "string" && hex40.test(allocation.tree));
  assert.ok(allocation.backend === "postgres" || allocation.backend === "sqlite");
  assert.ok(deepEquals(allocation.binding, binding, true));
  assert.ok(allocation.runId === binding.runId && allocation.runAttempt === binding.runAttempt);
  if (allocation.executionMode !== "orca-local") {
    assert.ok(typeof allocation.runId === "string" && /^[0-9]+$/.test(allocation.runId));
    assert.ok(typeof allocation.runAttempt === "string" && /^[0-9]+$/.test(allocation.runAttempt));
    assert.ok(allocation.currentCIJobConfirmed === true);
  }
}

interface Report {
  config: { workers: number };
  errors: unknown[];
  stats: Record<string, number>;
  suites: { specs: { tests: { results: Json[] }[] }[] }[];
}
export function singleAttachment(path: string, name: string): Json {
  const report = read(path) as Report;
  assert.ok(
    jsonInteger(report.config, "workers") &&
      report.config.workers === 1 &&
      deepEquals(report.errors, []),
  );
  assert.ok(report.stats.expected === 1);
  assert.ok(["unexpected", "flaky", "skipped"].every((key) => report.stats[key] === 0));
  const tests = report.suites.flatMap((suite) => suite.specs.flatMap((spec) => spec.tests));
  assert.ok(tests.length === 1 && tests[0]?.results.length === 1);
  const actual = tests[0].results[0] as Json;
  assert.ok(actual.status === "passed" && actual.retry === 0 && deepEquals(actual.errors, []));
  const entries = (actual.attachments as Json[]).filter((entry) => entry.name === name);
  assert.ok(entries.length === 1 && entries[0]?.contentType === "application/json");
  const decoded = attachmentJson(entries[0].body);
  assert.ok(typeof decoded === "object" && decoded !== null && !Array.isArray(decoded));
  return decoded as Json;
}

export const browserPort = (base: string) => Number(base.slice(base.lastIndexOf(":") + 1));

// Reuse PG's admitted CLI; SQLite's body uses this same literal file.
export function restartBrowserArgs(context: {
  root: string;
  bun: string;
  spec: string;
  playwrightCli?: string;
  before: { external: Record<string, string> };
}): string[] {
  const expected = join(context.root, "node_modules/playwright/cli.js"),
    cli = context.playwrightCli ?? expected;
  assert.ok(cli === expected);
  assert.ok(!lstatSync(cli, { throwIfNoEntry: false })?.isSymbolicLink());
  assert.ok(statSync(cli, { throwIfNoEntry: false })?.isFile());
  assert.ok(
    sha(cli) === context.before.external[cli],
    "restart CLI must remain in the admitted input closure",
  );
  return [
    context.bun,
    "--no-install",
    cli,
    "test",
    "--config",
    "e2e-pending/collab-playwright.config.ts",
    "--reporter=line,json",
    "--grep",
    title,
    context.spec,
  ];
}

// The I/O the restart owns: commands, Docker rows, the app-role reader, the
// second server process and its HTTP/port probes.
export interface Seam {
  command: Command;
  ownedRows: (name: string) => Promise<Row[]>;
  identityGone: (row: Row) => boolean;
  pgSql: (sql: string, app: boolean) => Promise<unknown>;
  spawnServer: (args: string[], log: number) => Child;
  probeSetup: (base: string) => Promise<{ status: number; body: unknown }>;
  portClosed: (port: number) => Promise<boolean>;
  // One JSON line on this process's stdout.
  emit: (line: string) => void;
  // The running actor's uid and gid; the restart refuses anything but 1000:1000.
  actor: () => readonly [number | undefined, number | undefined];
}

// The parent lane's concrete state the restart reads. It is not a product DI.
export interface RestartContext {
  run: string;
  name: string;
  browserEnv: Record<string, string>;
  code: number;
  head: string;
  tree: string;
  compiledHead: string;
  identity: { runId: string; runAttempt: string; executionMode?: string };
  parentDriver: string;
  binaries: Record<string, { sha256: string }>;
  server: string;
  migrate: string;
  engine: string;
  distFiles: Record<string, string>;
  abiFiles: Record<string, string>;
  browserInputs: Browser;
  root: string;
  bun: string;
  spec: string;
  playwrightCli?: string;
  before: Inputs;
  sourceBefore: Inputs;
  inputCheck: () => Promise<Inputs>;
  treeHashes: (directory: string) => Record<string, string>;
  dist: string;
  storage: string;
  serverRow: Row;
  serverProcess: Child;
  base: string;
  // The lane receipt: database_inode (sqlite), the restricted role facts
  // (postgres), and the published browser projection on failure.
  receipt: Receipt;
  // Postgres projects the restart browser report through its known spec map.
  projectBrowser: boolean;
  role?: string;
  masterKey?: string;
  db?: string;
  dbroot?: string;
}

export const restrictedRoleQuery = `SELECT jsonb_build_object('user',current_user,'version',current_setting('server_version_num'),
      'superuser',r.rolsuper,'bypassrls',r.rolbypassrls,
      'owns_schema',EXISTS(SELECT 1 FROM pg_namespace WHERE nspname='fvoci' AND pg_get_userbyid(nspowner)=current_user),
      'owns_tables',EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='fvoci' AND pg_get_userbyid(c.relowner)=current_user),
      'versions',(SELECT jsonb_agg(version ORDER BY version) FROM fvoci.schema_migrations),
      'ledger',(SELECT jsonb_agg(jsonb_build_array(version,lineage,sql_sha256) ORDER BY version) FROM fvoci.schema_migrations),
      'rls',(SELECT jsonb_object_agg(c.relname,jsonb_build_object('enabled',c.relrowsecurity,'forced',c.relforcerowsecurity))
             FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='fvoci'
             AND c.relname IN('documents','document_states','document_collab_updates','wiki_create_commands','revisions')))
      FROM pg_roles r WHERE r.rolname=current_user`;
export interface RoleFacts {
  user: unknown;
  superuser: unknown;
  bypassrls: unknown;
  owns_schema: unknown;
  owns_tables: unknown;
  rls: Record<string, { enabled: unknown; forced?: unknown }>;
  [key: string]: unknown;
}
// The restarted app role must read back exactly the initial facts, ledger included.
export async function restartedRoleFacts(
  pgSql: Seam["pgSql"],
  role: string | undefined,
  initial: unknown,
): Promise<RoleFacts> {
  const facts = (await pgSql(restrictedRoleQuery, true)) as RoleFacts;
  assert.ok(deepEquals(facts, initial, true));
  assert.ok(facts.user === role && facts.superuser === false && facts.bypassrls === false);
  assert.ok(facts.owns_schema === false && facts.owns_tables === false);
  assert.ok(
    Object.keys(facts.rls).length === 5 &&
      Object.values(facts.rls).every((row) => row.enabled === true),
  );
  return facts;
}

function inode(path: string, link = false): [number, number] {
  const facts = link ? lstatSync(path, { bigint: true }) : statSync(path, { bigint: true });
  const result = [Number(facts.dev), Number(facts.ino)] as [number, number];
  assert.ok(result.every(Number.isSafeInteger) && BigInt(result[1]) === facts.ino);
  return result;
}
const mode = (value: number) => value & 0o7777;
const readyPattern = /fvoci-server listening on (http:\/\/127\.0\.0\.1:\d+)/;
const meiliKey = "/run/fvoci/meili/api_key";

export async function restartSameApp(g: RestartContext, seam: Seam): Promise<Receipt> {
  const { run, name } = g,
    selected = g.browserEnv.FVOCI_E2E_SELECTED_BACKEND;
  const owner = env("FVOCI_CI_OWNER");
  const [actorUid, actorGid] = seam.actor();
  assert.ok(actorUid === 1000 && actorGid === 1000);
  assert.ok(process.env.FVOCI_ROOT_RUN_OWNER === owner);
  assert.ok((selected === "postgres" || selected === "sqlite") && g.code === 0);
  assert.ok(g.head === g.compiledHead, "changed current Rust requires its actual compiled SHA");
  const grantPath = env("FVOCI_ROOT_RESTART_GRANT");
  assert.ok(isAbsolute(grantPath) && !lstatSync(grantPath).isSymbolicLink());
  const allocation = read(grantPath) as Allocation;
  const binding: RestartBinding = {
    runId: g.identity.runId,
    runAttempt: g.identity.runAttempt,
    source: g.head,
    tree: g.tree,
    compiledSource: g.compiledHead,
    backend: selected,
    runRoot: run,
    parentDriverSha256: sha(g.parentDriver),
    restartHelperSha256: sha(restartHelper),
    sourceInputsSha256: sha(join(run, "source-inputs-before.json")),
    artifactHashes: Object.fromEntries(
      Object.entries(g.binaries).map(([path, record]) => [path, record.sha256]),
    ),
    assetHashes: g.distFiles,
    browserInputs: g.browserInputs,
    abiHashes: g.abiFiles,
  };
  validateAllocation(allocation, binding);
  const receipt: Receipt = {
    scope:
      "current-schema same owned DB/storage server restart only; not upgrade/archive/restore/Turso/whole0.6",
    binding,
    allocationSha256: sha(grantPath),
    stage: "validated",
    restartBrowserExit: null,
    cleanupErrors: [],
  };
  const cleanupErrors = receipt.cleanupErrors as unknown[];
  let restartProcess: Child | null = null,
    restartLog: number | null = null,
    restartServer: Row | null = null,
    restartBase: string | null = null;
  const restartLogPath = join(run, "restarted-normal-server.log");
  const inputs = async () => {
    restartBrowserArgs(g);
    assert.ok(deepEquals(await g.inputCheck(), g.sourceBefore, true));
    for (const [path, record] of Object.entries(g.binaries)) assert.ok(sha(path) === record.sha256);
    assert.ok(deepEquals(g.treeHashes(g.dist), g.distFiles));
    for (const [path, expected] of Object.entries(g.abiFiles)) assert.ok(sha(path) === expected);
    const browser = g.browserInputs;
    assert.ok(sha(browser.bun.path) === browser.bun.sha256);
    assert.ok(sha(browser.chromium.path) === browser.chromium.sha256);
    assert.ok(
      deepEquals(g.treeHashes(dirname(browser.chromium.path)), browser.chromium_directory_files),
    );
    assert.ok(sha(grantPath) === receipt.allocationSha256);
    assert.ok(sha(restartHelper) === binding.restartHelperSha256);
    assert.ok(sha(g.parentDriver) === binding.parentDriverSha256);
    const copied = await seam.command([
      "docker",
      "exec",
      name,
      "sha256sum",
      "/fvoci/bin/fvoci-server",
      "/fvoci/bin/fvoci-migrate",
      "/fvoci/bin/collab-engine",
    ]);
    assert.ok(
      deepEquals(
        copied.stdout
          .split("\n")
          .filter((line) => line !== "")
          .map((line) => line.trim().split(/\s+/)[0]),
        [g.server, g.migrate, g.engine].map((path) => g.binaries[path]?.sha256),
      ),
    );
  };
  try {
    await inputs();
    const storageInode = inode(g.storage);
    receipt.storageInode = storageInode;
    const seed = singleAttachment(
      join(run, "playwright-result.private.json"),
      "selected-vue-native-readback.json",
    );
    assert.ok(seed.selected === selected && seed.firstAck !== seed.finalAck);
    assert.ok(seed.creatorId !== seed.freshActorId);
    assert.ok(
      (seed.canonicalEmojiOracleControls as unknown[]).length === 6 &&
        (seed.nativeHistoryOracleControls as unknown[]).length === 2,
    );
    receipt.seedReportSha256 = sha(join(run, "playwright-result.private.json"));
    const label = (
      await seam.command([
        "docker",
        "inspect",
        "--format",
        '{{index .Config.Labels "fvoci.owner"}}',
        name,
      ])
    ).stdout.trim();
    assert.ok(label === owner);
    let originalInode: [number, number] | null = null;
    if (selected === "sqlite") {
      originalInode = inode(g.db as string);
      assert.ok(deepEquals(originalInode, g.receipt.database_inode));
    }
    const beforeRows = (await seam.ownedRows(name)).filter((row) =>
      row.args.startsWith("/fvoci/bin/"),
    );
    assert.ok(
      beforeRows.some(
        (row) => row.pid === g.serverRow.pid && row.start_ticks === g.serverRow.start_ticks,
      ),
    );
    const stopped = await seam.command(
      [
        "docker",
        "exec",
        "--user",
        "0",
        name,
        "/bin/kill",
        "-TERM",
        String(g.serverRow.namespace_pid),
      ],
      { required: false },
    );
    receipt.seedSigtermExit = stopped.returncode;
    assert.ok(stopped.returncode === 0);
    receipt.seedServerExit = await waitFor(g.serverProcess, 10_000);
    assert.ok(receipt.seedServerExit === 0);
    assert.ok(
      beforeRows.every((row) => seam.identityGone(row)),
      "old server/native identities must retire before restart",
    );
    assert.ok(
      await seam.portClosed(browserPort(g.base)),
      "old listen socket must be closed before restart",
    );
    receipt.oldRecordedRows = beforeRows;
    receipt.oldPortClosed = true;
    if (selected === "sqlite") {
      // Only our confirmed actor receipt is relocated; keep its bytes and inode.
      const dbroot = g.dbroot as string;
      const actorFiles = readdirSync(dbroot)
        .filter((entry) => /^actor-.*\.json$/s.test(entry))
        .sort();
      assert.ok(actorFiles.length === 1);
      const actorName = actorFiles[0] as string,
        actorFile = join(dbroot, actorName),
        metadata = lstatSync(actorFile);
      assert.ok(metadata.isFile() && metadata.nlink === 1);
      assert.ok(
        metadata.uid === 1000 &&
          metadata.gid === 1000 &&
          [0o600, 0o644].includes(mode(metadata.mode)),
      );
      assert.ok(
        mode(statSync(dbroot).mode) === 0o700,
        "nonsecret actor receipt stays in owned private parent",
      );
      const actor = read(actorFile) as Json;
      assert.ok(actor.backend === "sqlite" && actor.commit === "confirmed");
      assert.ok(
        Boolean(actor.poolClosed) &&
          actor.connectionClose === "confirmed" &&
          Boolean(actor.operationSucceeded),
      );
      assert.ok(actorName === "actor-" + String(seed.freshActorId) + ".json");
      const savedHash = sha(actorFile),
        destination = join(run, "preserved-" + actorName),
        actorInode = inode(actorFile, true);
      assert.ok(!existsSync(destination) && inode(run)[0] === actorInode[0]);
      renameSync(actorFile, destination);
      const moved = lstatSync(destination);
      assert.ok(deepEquals(inode(destination, true), actorInode) && sha(destination) === savedHash);
      assert.ok(mode(moved.mode) === mode(metadata.mode));
      receipt.preservedActorReceipt = {
        path: destination,
        sha256: savedHash,
        sameInode: true,
        originalMode: mode(metadata.mode),
      };
      assert.ok(deepEquals(inode(g.db as string), originalInode));
    }
    const checkpoint = {
      schema: 1,
      source: g.head,
      tree: g.tree,
      compiledSource: g.compiledHead,
      selected,
      stopped: { serverExit: 0, portClosed: true, recordedIdentitiesRetired: true },
      seed,
    };
    const checkpointPath = join(run, "restart-checkpoint.private.json");
    privateWrite(checkpointPath, checkpoint);
    receipt.checkpointSha256 = sha(checkpointPath);
    await inputs();
    restartLog = openSync(restartLogPath, "wx", 0o600);
    fchmodSync(restartLog, 0o600);
    // Same container, literal private environment, DB and storage; port0 again.
    restartProcess = seam.spawnServer(
      [
        "docker",
        "exec",
        name,
        "/bin/sh",
        "-ec",
        ". /fvoci/inputs/environment.sh; exec /fvoci/bin/fvoci-migrate --start",
      ],
      restartLog,
    );
    const deadline = performance.now() + 10_000;
    for (;;) {
      const match = readyPattern.exec(readText(restartLogPath));
      if (match) {
        restartBase = match[1] as string;
        break;
      }
      assert.ok(
        running(restartProcess),
        "restarted normal preparation exited before listen; preserve log",
      );
      assert.ok(performance.now() < deadline, "unchanged10s listen observation expired");
      await Bun.sleep(20);
      checkInterrupt();
    }
    const servers = (await seam.ownedRows(name)).filter(
      (row) => row.args === "/fvoci/bin/fvoci-server" && !row.already_retired_at_observation,
    );
    assert.ok(servers.length === 1);
    restartServer = servers[0] as Row;
    assert.ok(restartServer.uid === 1000 && restartServer.gid === 1000);
    assert.ok(
      restartServer.pid !== g.serverRow.pid ||
        restartServer.start_ticks !== g.serverRow.start_ticks,
    );
    const setup = await seam.probeSetup(restartBase);
    assert.ok(setup.status === 200 && (setup.body as Json | null)?.needed === false);
    if (selected === "sqlite") {
      const meta = lstatSync(g.db as string);
      assert.ok(meta.isFile() && deepEquals(inode(g.db as string, true), originalInode));
      assert.ok(
        meta.uid === 1000 && meta.gid === 1000 && mode(meta.mode) === 0o600 && meta.nlink === 1,
      );
    } else {
      receipt.restartedAppRoleRegistryRLS = await restartedRoleFacts(
        seam.pgSql,
        g.role,
        g.receipt.actual_restricted_role_schema_rls,
      );
      const keyMetadata = (
        await seam.command(["docker", "exec", name, "stat", "-c", "%u %g %a", meiliKey])
      ).stdout.trim();
      assert.ok(keyMetadata === "0 1000 640");
      const key = (
        await seam.command(["docker", "exec", "--user", "1000", name, "cat", meiliKey])
      ).stdout.trim();
      assert.ok(key.length >= 16 && key !== g.masterKey);
      const log = readText(restartLogPath);
      assert.ok(log.includes("meilisearch enabled"));
      assert.ok(log.includes("outbox dispatcher started"));
      receipt.restartedScopedSearchStartup = true;
    }
    assert.ok(deepEquals(inode(g.storage), storageInode));
    Object.assign(receipt, {
      stage: "restarted normal main ready",
      restartedBaseURL: restartBase,
      restartedServer: restartServer,
    });
    // Neither owner DB URL/pepper nor actor executable reaches readback phase.
    const browserEnv: Record<string, string> = Object.fromEntries(
      Object.entries(g.browserEnv).filter(([key]) =>
        [
          "PATH",
          "LANG",
          "PLAYWRIGHT_BROWSERS_PATH",
          "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
          "TMPDIR",
        ].includes(key),
      ),
    );
    if (g.identity.executionMode !== "orca-local") browserEnv.CI = "true";
    Object.assign(browserEnv, {
      PLAYWRIGHT_BASE_URL: restartBase,
      FVOCI_E2E_SELECTED_BACKEND: selected,
      FVOCI_E2E_SELECTED_RESTART_SOURCE: g.head,
      FVOCI_E2E_SELECTED_RESTART_CHECKPOINT: checkpointPath,
      FVOCI_E2E_RESULT_DIR: join(run, "restart-browser"),
      PLAYWRIGHT_JSON_OUTPUT_FILE: join(run, "restart-playwright-result.private.json"),
    });
    const args = restartBrowserArgs(g);
    receipt.restartBrowserCommand = args;
    const started = performance.now();
    const result = await seam.command(args, {
      log: join(run, "restart-browser.log"),
      required: false,
      env: browserEnv,
      cwd: join(g.root, "apps/web"),
    });
    receipt.restartBrowserExit = result.returncode;
    receipt.restartBrowserSeconds = (performance.now() - started) / 1000;
    const reportPath = join(run, "restart-playwright-result.private.json");
    if (existsSync(reportPath)) chmodSync(reportPath, 0o600);
    assert.ok(result.returncode === 0, "preserve original restart browser failure");
    const readback = singleAttachment(reportPath, "selected-vue-restart-readback.json");
    assert.ok(
      readback.source === g.head && readback.tree === g.tree && readback.selected === selected,
    );
    assert.ok(
      readback.workspaceId === seed.workspaceId &&
        readback.documentId === (seed.document as Json | undefined)?.id,
    );
    assert.ok(
      readback.freshActorId === seed.freshActorId &&
        deepEquals(readback.persisted, seed.persisted, true) &&
        deepEquals(readback.revision, seed.revision, true),
    );
    assert.ok(readback.firstAck === seed.firstAck && readback.finalAck === seed.finalAck);
    assert.ok(
      (readback.canonicalEmojiOracleControls as unknown[]).length === 6 &&
        (readback.nativeHistoryOracleControls as unknown[]).length === 2,
    );
    privateWrite(join(run, "selected-vue-restart-readback.private.json"), readback);
    Object.assign(receipt, {
      stage: "readback passed",
      actualRestartBrowserTests: 1,
      retries: 0,
      ignored: 0,
      restartReportSha256: sha(reportPath),
      restartAttachmentSha256: sha(join(run, "selected-vue-restart-readback.private.json")),
    });
    await inputs();
  } catch (error) {
    receipt.originalFailure = errorFacts(error);
    // Publish only a fixed helper file:line and the maintained reporter projection.
    // Original messages/logs/reports remain private; no stack text is published.
    const line = frameLine(error, restartHelper);
    const safe: Record<string, unknown> = {
      restart_stage: receipt.stage,
      restart_helper_checkpoint:
        line === null ? null : relative(root, restartHelper) + ":" + String(line),
      restart_browser_exit: receipt.restartBrowserExit,
      known_browser_test: null,
      known_browser_status: null,
      known_browser_checkpoint: null,
      browser_report_state: "report-missing",
    };
    try {
      const reportPath = join(run, "restart-playwright-result.private.json");
      if (existsSync(reportPath)) {
        chmodSync(reportPath, 0o600);
        receipt.restartReportSha256 = sha(reportPath);
        if (g.projectBrowser) Object.assign(safe, knownBrowserCheckpoint(read(reportPath), true));
        else safe.browser_report_state = "report-unreadable";
      }
    } catch {
      safe.browser_report_state = "report-unreadable";
      receipt.diagnosticError = "restart-safe-failure-collection-failed";
    }
    receipt.safeFailure = safe;
    try {
      for (const key of [
        "known_browser_test",
        "known_browser_status",
        "known_browser_checkpoint",
        "browser_report_state",
      ])
        g.receipt[key] = safe[key];
      seam.emit(JSON.stringify(safe));
    } catch {
      receipt.diagnosticError = "restart-safe-failure-publication-failed";
    }
    throw error;
  } finally {
    // The caller keeps its own finally for the container and fixture wrappers.
    // This block owns only the second exec/server; a timeout stays a failure.
    await cleanupScope(async () => {
      if (restartProcess !== null) {
        try {
          const current = await seam.ownedRows(name);
          const live = current.filter(
            (row) => row.args === "/fvoci/bin/fvoci-server" && !row.already_retired_at_observation,
          );
          if (live.length) {
            assert.ok(live.length === 1);
            restartServer = live[0] as Row;
            assert.ok(restartServer.uid === 1000 && restartServer.gid === 1000);
            const stopped = await seam.command(
              [
                "docker",
                "exec",
                "--user",
                "0",
                name,
                "/bin/kill",
                "-TERM",
                String(restartServer.namespace_pid),
              ],
              { required: false },
            );
            receipt.restartSigtermExit = stopped.returncode;
            assert.ok(stopped.returncode === 0);
          }
          receipt.restartServerExit = await waitFor(restartProcess, 10_000);
          assert.ok(receipt.restartServerExit === 0);
          receipt.restartedRecordedIdentitiesRetired = current
            .filter((row) => row.args.startsWith("/fvoci/bin/"))
            .every((row) => seam.identityGone(row));
          assert.ok(receipt.restartedRecordedIdentitiesRetired);
          if (restartBase !== null) {
            receipt.restartedPortClosed = await seam.portClosed(browserPort(restartBase));
            assert.ok(receipt.restartedPortClosed);
          }
        } catch (error) {
          cleanupErrors.push(errorFacts(error));
        }
      }
      if (restartLog !== null) {
        closeSync(restartLog);
        receipt.restartServerLogSha256 = sha(restartLogPath);
      }
      const browserLog = join(run, "restart-browser.log");
      if (existsSync(browserLog)) {
        try {
          chmodSync(browserLog, 0o600);
          receipt.restartBrowserLogSha256 = sha(browserLog);
        } catch (error) {
          cleanupErrors.push({
            type: errorFacts(error).type,
            message: "restart browser log qualification failed",
          });
        }
      }
      privateWrite(join(run, "restart-receipt.private.json"), receipt);
    });
  }
  assert.ok(
    cleanupErrors.length === 0,
    "restart cleanup failure is not PASS; parent retains original failure",
  );
  return receipt;
}
