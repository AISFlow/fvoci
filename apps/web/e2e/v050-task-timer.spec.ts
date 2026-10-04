import { execFileSync, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { expect, test as base, type Page, type Route } from "@playwright/test";
import { z } from "zod";
import { createE2eUser, login } from "./helpers";
import { isoToDatetimeLocalInTimeZone } from "../src/lib/datetime";

const credentials = { email: "timer@example.com", password: "supersecret1" };
const identityShape = z.object({ userId: z.string(), sessionId: z.string() });
const workspaces = z.object({
  items: z.array(z.object({ id: z.string(), slug: z.string() })),
});
const taskShape = z.object({
  id: z.string(),
  number: z.number(),
  statusId: z.string(),
});
const timerShape = z.object({
  run: z
    .object({
      id: z.string(),
      status: z.enum(["running", "paused", "stopped"]),
      version: z.number(),
      runningSince: z.string().nullable(),
      elapsedMilliseconds: z.number(),
    })
    .nullable(),
  actualMilliseconds: z.number(),
  canControl: z.boolean(),
});

type TimerNative = {
  pid: number;
  parentPid: number;
  launcher: string;
  configuredBin: string;
  knownFileSha256: string;
  listeningOrigin: string;
  procExeIdentity: string;
  procStartTicks: string;
  supervision?: "direct Playwright fixture child";
};
type NativeClose = {
  code: number | null;
  signal: NodeJS.Signals | null;
  closeObserved: true;
  stdoutEnded: boolean;
  stderrEnded: boolean;
};
type RestartedTimerServer = {
  child: ChildProcessWithoutNullStreams;
  closed: Promise<NativeClose>;
  witness: TimerNative;
  original: TimerNative;
  log: string;
};
const restartedTimerServers = new WeakMap<Page, RestartedTimerServer>();
type TimerRestart = () => Promise<string>;
const test = base.extend<{ restartTimerServer: TimerRestart }>({
  page: async ({ page }, use) => {
    try {
      await use(page);
    } finally {
      // The owned page/context fixture must finish body observations before
      // Playwright destroys its response identifiers during teardown.
      if (loadedEntrypoints.has(page)) await drainLoadedEntrypoints(page);
    }
  },
  restartTimerServer: async ({ page }, use, testInfo) => {
    const transitions: Array<Record<string, unknown>> = [];
    const restartSnapshots: Array<Record<string, unknown>> = [];
    const resultDir = process.env.FVOCI_E2E_RESULT_DIR;
    if (!resultDir) throw new Error("owned restart result namespace missing");
    const ownedProcesses: Array<{
      witness: TimerNative;
      log: string;
      exit?: {
        exitCode: number | null;
        signal: NodeJS.Signals | null;
        unexpectedExit: boolean;
        outputCompletion: NativeClose;
      };
    }> = [];
    const ownershipPath = path.join(resultDir, "w5-native-restart-owned-processes.json");
    const persistOwnership = () => {
      writeFileSync(
        ownershipPath,
        JSON.stringify({ originalNamespace: resultDir, ownedProcesses }, null, 2),
        { mode: 0o600 },
      );
    };
    const preparationVariable = (name: string) =>
      name !== "DATABASE_APP_URL" &&
      (/DATABASE/i.test(name) ||
        /^(PG|POSTGRES_)/.test(name) ||
        name === "FVOCI_MIGRATION_URL" ||
        /^(FVOCI_E2E_|FVOCI_TEST_|FVOCI_W5_)/.test(name));
    const removedNativeNames = Object.keys(process.env).filter(preparationVariable).sort();
    const runtimeNames = Object.keys(process.env)
      .filter((name) => !preparationVariable(name))
      .filter((name) =>
        /^(DATABASE_APP_URL$|PASSWORD_PEPPER_|ENCRYPTION_|FVOCI_|SMTP_|RUST_LOG$|MEILI_)/.test(
          name,
        ),
      )
      .sort();
    const runtimeHash = (env: NodeJS.ProcessEnv) =>
      createHash("sha256")
        .update(JSON.stringify(runtimeNames.map((name) => [name, env[name]])))
        .digest("hex");
    const inheritedHash = runtimeHash(process.env);
    const exitChild = async (owned: RestartedTimerServer) => {
      const unexpectedExit = owned.child.exitCode !== null || owned.child.signalCode !== null;
      let code = owned.child.exitCode;
      let signal = owned.child.signalCode;
      if (!unexpectedExit) {
        const stat = readFileSync(`/proc/${String(owned.witness.pid)}/stat`, "utf8");
        expect(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[19]).toBe(
          owned.witness.procStartTicks,
        );
        expect(
          readFileSync(`/proc/${String(owned.witness.pid)}/cmdline`, "utf8").split("\0")[0],
        ).toBe(owned.witness.configuredBin);
        const exited = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
          (resolve) => {
            owned.child.once("exit", (exitCode, exitSignal) => {
              resolve({ code: exitCode, signal: exitSignal });
            });
          },
        );
        expect(owned.child.kill("SIGTERM")).toBe(true);
        ({ code, signal } = await exited);
      }
      // Node close follows exit/error and closure of all child stdio. This
      // promise was enrolled at spawn, so an already-exited child cannot miss it.
      const outputCompletion = await owned.closed;
      const record = ownedProcesses.find((process) => process.witness.pid === owned.witness.pid);
      if (!record) throw new Error("restart ownership record missing");
      record.exit = { exitCode: code, signal, unexpectedExit, outputCompletion };
      persistOwnership();
      const procAbsent = !existsSync(`/proc/${String(owned.witness.pid)}`);
      expect(procAbsent).toBe(true);
      return {
        pid: owned.witness.pid,
        exitCode: code,
        signal,
        unexpectedExit,
        procAbsent,
        outputCompletion,
      };
    };
    let current: RestartedTimerServer | undefined;
    const restart: TimerRestart = async () => {
      const { native, configuredBin, binaryHash } = captureTimerNative(page);
      expect(native).toHaveLength(1);
      // Finish this generation's browser/served-byte observation while its
      // exact server is still live; later generations retain separate records.
      await verifyLoadedEntrypoints(page, native);
      const before = native[0];
      if (!before) throw new Error("verified original native witness missing");
      expect(before.procStartTicks).toMatch(/^\d+$/);
      const original = current?.original ?? before;
      const launcherArguments = readFileSync(
        `/proc/${String(original.parentPid)}/cmdline`,
        "utf8",
      ).split("\0");
      expect(launcherArguments).toContain(path.resolve("../../scripts/web-e2e-inner.sh"));
      const inherited = Object.fromEntries(
        Object.entries(process.env).filter(([name]) => !preparationVariable(name)),
      );
      expect(Object.keys(inherited).filter(preparationVariable)).toEqual([]);
      for (const name of [
        "DATABASE_URL",
        "FVOCI_MIGRATION_URL",
        "FVOCI_E2E_ADMIN_DATABASE_URL",
        "TEST_DATABASE_URL",
      ])
        expect(inherited[name]).toBeUndefined();
      // Owner URL remains only in the diagnostic test process; native receives
      // the same app credential and runtime inputs, with no preparation secrets.
      expect(process.env.FVOCI_E2E_ADMIN_DATABASE_URL).toBeTruthy();
      expect(runtimeHash(inherited)).toBe(inheritedHash);
      expect(inherited.FVOCI_BIND).toBe("127.0.0.1:0");
      expect(inherited.DATABASE_URL).toBeUndefined();
      expect(inherited.DATABASE_APP_URL).toBeTruthy();
      const phase = before.supervision
        ? "measured same-database restart"
        : "process ownership preparation";
      const retainSnapshot = (stage: "before-stop" | "after-stop" | "after-ready", raw: string) => {
        const basename = `timer-restart-${String(transitions.length + 1)}-${stage}.json`;
        const target = path.join(resultDir, basename);
        const evidence = process.env.FVOCI_W5_EVIDENCE_DIR;
        const durablePath = evidence ? path.join(evidence, basename) : null;
        const snapshot = {
          restartNumber: transitions.length + 1,
          phase,
          stage,
          before,
          raw,
          rawSha256: createHash("sha256").update(raw).digest("hex"),
          path: target,
          durablePath,
        };
        // Preserve actual unfiltered six-table SQL bytes before equality oracles,
        // including a failed transition that never reaches transitions.push.
        restartSnapshots.push(snapshot);
        const content = JSON.stringify(snapshot, null, 2);
        writeFileSync(target, content, { mode: 0o600 });
        if (evidence && durablePath) {
          mkdirSync(evidence, { recursive: true });
          writeFileSync(durablePath, content, { mode: 0o600 });
        }
        return snapshot;
      };
      const beforeRaw = timerDatabaseEffectsForRestart();
      const beforeStopSnapshot = retainSnapshot("before-stop", beforeRaw);
      // Leave the app before SIGTERM, closing its real SSE/collaboration transports.
      // The paused/running database rows are never changed by this fixture.
      await page.goto("about:blank");
      let stopped: Record<string, unknown>;
      if (current) {
        const exit = await exitChild(current);
        expect(exit.unexpectedExit).toBe(false);
        expect(exit.exitCode).toBe(0);
        expect(exit.signal).toBeNull();
        stopped = exit;
      } else {
        const originalStat = readFileSync(`/proc/${String(before.pid)}/stat`, "utf8");
        expect(originalStat.slice(originalStat.lastIndexOf(")") + 2).split(" ")[19]).toBe(
          before.procStartTicks,
        );
        expect(readFileSync(`/proc/${String(before.pid)}/cmdline`, "utf8").split("\0")[0]).toBe(
          configuredBin,
        );
        process.kill(before.pid, "SIGTERM");
        await expect.poll(() => existsSync(`/proc/${String(before.pid)}`)).toBe(false);
        stopped = {
          pid: before.pid,
          signalSent: "SIGTERM",
          procAbsent: true,
          exitCode: "NOTCAPTURED: original server is the surviving launcher's child",
        };
      }
      // The original shell is still the owner of this SAME database/storage group.
      process.kill(original.parentPid, 0);
      expect(
        readFileSync(`/proc/${String(original.parentPid)}/cmdline`, "utf8").split("\0"),
      ).toContain(original.launcher);
      const afterStopRaw = timerDatabaseEffectsForRestart();
      const afterStopSnapshot = retainSnapshot("after-stop", afterStopRaw);
      expect(afterStopRaw).toBe(beforeRaw);
      const log = path.join(resultDir, `timer-restart-${String(transitions.length + 1)}.log`);
      writeFileSync(log, "", { mode: 0o600, flag: "wx" });
      const child = spawn(configuredBin, [], { env: inherited, stdio: "pipe" });
      // Enroll before any await or readiness work; close includes stdio drain.
      const closed = new Promise<NativeClose>((resolve) => {
        child.once("close", (code, signal) => {
          resolve({
            code,
            signal,
            closeObserved: true,
            stdoutEnded: child.stdout.readableEnded,
            stderrEnded: child.stderr.readableEnded,
          });
        });
      });
      child.stdin.end();
      let nativeOutput = "";
      const ready = new Promise<string>((resolve, reject) => {
        let settled = false;
        const removeReadinessListeners = () => {
          child.off("error", nativeError);
          child.off("exit", nativeExited);
        };
        const nativeError = (error: Error) => {
          if (settled) return;
          settled = true;
          removeReadinessListeners();
          reject(error);
        };
        const nativeExited = (code: number | null, signal: NodeJS.Signals | null) => {
          nativeError(
            new Error(`native exited before readiness: ${String(code)}/${String(signal)}`),
          );
        };
        child.once("error", nativeError);
        child.once("exit", nativeExited);
        const nativeOutputReceived = (chunk: Buffer) => {
          const text = chunk.toString("utf8");
          appendFileSync(log, text);
          nativeOutput += text;
          const origin = nativeOutput
            .split("\n")
            .slice(0, -1)
            .find((line) => line.includes("fvoci-server listening on "))
            ?.split("fvoci-server listening on ")[1]
            ?.trim();
          if (origin && !settled) {
            settled = true;
            removeReadinessListeners();
            resolve(origin);
          }
        };
        // main.rs announces readiness on stderr; capture both real native streams.
        child.stdout.on("data", nativeOutputReceived);
        child.stderr.on("data", nativeOutputReceived);
      });
      if (!child.pid) throw new Error("spawned native PID unavailable");
      // Save ownership immediately so cancellation/readiness failure also cleans this child.
      current = {
        child,
        closed,
        original,
        log,
        witness: {
          pid: child.pid,
          parentPid: process.pid,
          launcher: "Playwright restartTimerServer fixture",
          configuredBin,
          knownFileSha256: binaryHash,
          listeningOrigin: "PENDING",
          procExeIdentity: "NOTCAPTURED: intentional nondumpability",
          procStartTicks: "PENDING",
          supervision: "direct Playwright fixture child",
        },
      };
      restartedTimerServers.set(page, current);
      ownedProcesses.push({ witness: current.witness, log });
      persistOwnership();
      const childStat = readFileSync(`/proc/${String(child.pid)}/stat`, "utf8");
      const childStartTicks = childStat.slice(childStat.lastIndexOf(")") + 2).split(" ")[19];
      if (!childStartTicks || !/^\d+$/.test(childStartTicks))
        throw new Error("actual owned child start ticks missing");
      current.witness.procStartTicks = childStartTicks;
      persistOwnership();
      const origin = await ready;
      current.witness.listeningOrigin = origin;
      persistOwnership();
      expect(new URL(origin).hostname).toBe("127.0.0.1");
      expect(origin).not.toBe(before.listeningOrigin);
      expect(current.witness.pid).not.toBe(before.pid);
      expect(createHash("sha256").update(readFileSync(configuredBin)).digest("hex")).toBe(
        binaryHash,
      );
      const setup = await page.request.get(`${origin}/api/v1/setup`);
      expect(setup.status(), await setup.text()).toBe(200);
      expect(runtimeHash(inherited)).toBe(inheritedHash);
      const afterReadyRaw = timerDatabaseEffectsForRestart();
      const afterReadySnapshot = retainSnapshot("after-ready", afterReadyRaw);
      expect(afterReadyRaw).toBe(beforeRaw);
      transitions.push({
        phase,
        before,
        stopped,
        after: current.witness,
        originalLauncherSurvives: true,
        sameDatabaseStorageAndSecurityNames: runtimeNames,
        sameDatabaseStorageAndSecurityHash: inheritedHash,
        removedNativeEnvironmentNames: removedNativeNames,
        forbiddenNativeVariableNamesAbsent: true,
        originalLauncherCredentialBoundary:
          "NOT PROVEN: original shared launcher inherited owner diagnostics; fixture children omit these, app-role witness alone does not prove original absence",
        exportedInputBasis:
          "identical inner.sh runtime input subset including app credential, filtered preparation/owner variables for fixture spawns; original /proc/environ intentionally unavailable",
        originalLauncherSourceSha256: createHash("sha256")
          .update(readFileSync(original.launcher))
          .digest("hex"),
        nativeFeaturesBasis:
          "identical executable bytes across generations; actual build flags/fingerprints belong the batch build receipt",
        samePersistedRows: true,
        sixTableSnapshots: [beforeStopSnapshot, afterStopSnapshot, afterReadySnapshot],
      });
      await page.goto(origin);
      return origin;
    };
    let fixtureError: unknown;
    try {
      await use(restart);
    } catch (error) {
      fixtureError = error;
    }
    // Persist the actual test outcome before any child/cleanup assertion can fail.
    const beforeCleanupOutcome = {
      status: testInfo.status,
      expectedStatus: testInfo.expectedStatus,
      errors: testInfo.errors.map(({ message }) => ({ message })),
      fixtureFailure:
        fixtureError instanceof Error
          ? { name: fixtureError.name, message: fixtureError.message }
          : fixtureError === undefined
            ? null
            : { type: typeof fixtureError },
      transitions,
      restartSnapshots,
      ownedProcesses,
      phase: "before cleanup assertions",
    };
    writeFileSync(
      path.join(resultDir, "w5-native-restart-before-cleanup-outcome.json"),
      JSON.stringify(beforeCleanupOutcome, null, 2),
      { mode: 0o600 },
    );
    const outcomeEvidence = process.env.FVOCI_W5_EVIDENCE_DIR;
    if (outcomeEvidence) {
      mkdirSync(outcomeEvidence, { recursive: true });
      writeFileSync(
        path.join(outcomeEvidence, "native-restart-before-cleanup-outcome.json"),
        JSON.stringify(beforeCleanupOutcome, null, 2),
        { mode: 0o600 },
      );
    }
    let navigationError: unknown;
    if (current) {
      try {
        if (!page.isClosed()) await page.goto("about:blank");
      } catch (error) {
        navigationError = error;
      }
      let finalExit: Awaited<ReturnType<typeof exitChild>> | undefined;
      const cleanupErrors: unknown[] = [];
      const nativeLogs: Array<{
        pid: number;
        procStartTicks: string;
        path: string;
        sha256: string;
      }> = [];
      try {
        finalExit = await exitChild(current);
      } catch (error) {
        cleanupErrors.push(error);
      } finally {
        restartedTimerServers.delete(page);
        // Retain every available child's real log even if exit validation/read
        // failed. Retention failures remain failures, never successful cleanup.
        for (const { witness, log } of ownedProcesses) {
          try {
            const content = readFileSync(log, "utf8").replace(
              /(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL)=[^\s]+/g,
              "$1=redacted",
            );
            const target = testInfo.outputPath(path.basename(log));
            writeFileSync(target, content, { mode: 0o600 });
            const evidence = process.env.FVOCI_W5_EVIDENCE_DIR;
            if (evidence) {
              mkdirSync(evidence, { recursive: true });
              writeFileSync(path.join(evidence, path.basename(log)), content, { mode: 0o600 });
            }
            nativeLogs.push({
              pid: witness.pid,
              procStartTicks: witness.procStartTicks,
              path: target,
              sha256: createHash("sha256").update(content).digest("hex"),
            });
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
      }
      const proof = {
        transitions,
        restartSnapshots,
        ownedProcesses,
        nativeLogs,
        finalExit,
        cleanupFailures: cleanupErrors.map((error) =>
          error instanceof Error
            ? { name: error.name, message: error.message }
            : { type: typeof error },
        ),
        diagnosticDatabase: diagnosticDatabase(),
      };
      try {
        const target = testInfo.outputPath("native-same-db-restart-process-proof.json");
        writeFileSync(target, JSON.stringify(proof, null, 2));
        const evidence = process.env.FVOCI_W5_EVIDENCE_DIR;
        if (evidence)
          writeFileSync(
            path.join(evidence, "native-same-db-restart-process-proof.json"),
            JSON.stringify(proof, null, 2),
          );
        await testInfo.attach("native-same-db-restart-process-proof", {
          path: target,
          contentType: "application/json",
        });
        expect(finalExit).toBeDefined();
        expect(finalExit?.unexpectedExit).toBe(false);
        expect(finalExit?.exitCode).toBe(0);
        expect(finalExit?.signal).toBeNull();
      } catch (error) {
        cleanupErrors.push(error);
      }
      if (cleanupErrors.length) {
        fixtureError = new AggregateError(
          [fixtureError, ...cleanupErrors].filter((error) => error !== undefined),
          "timer restart cleanup or evidence retention failed",
        );
      }
    }
    if (fixtureError !== undefined || navigationError !== undefined)
      throw new AggregateError(
        [fixtureError, navigationError].filter((error) => error !== undefined),
        "timer restart fixture or teardown failed",
      );
  },
});

function timerDatabaseEffectsForRestart() {
  return diagnosticSql(`SELECT jsonb_build_object(
    'runs',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM fvoci.task_timer_runs t),
    'segments',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM fvoci.task_timer_segments t),
    'commands',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY user_id,request_id),'[]'::jsonb) FROM fvoci.task_timer_commands t),
    'audit',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM fvoci.task_timer_audit t),
    'legacy',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY time_entry_id),'[]'::jsonb) FROM fvoci.task_timer_legacy_open t),
    'entries',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM fvoci.time_entries t))`);
}

// Test-only invoker witness inside this group's isolated database. No product
// policy is modified and no credential or task content enters diagnostics.
function diagnosticDatabase() {
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  const admin = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  const app = process.env.DATABASE_APP_URL;
  if (!container || !admin || !app) throw new Error("isolated timer diagnostic database missing");
  return {
    container,
    database: new URL(admin).pathname.slice(1),
    role: new URL(app).username,
  };
}
function diagnosticSql(sql: string): string {
  const { container, database } = diagnosticDatabase();
  return execFileSync(
    "docker",
    [
      "exec",
      "-i",
      container,
      "psql",
      "-U",
      "postgres",
      "-d",
      database,
      "-v",
      "ON_ERROR_STOP=1",
      "-qAt",
    ],
    { input: sql, encoding: "utf8", stdio: ["pipe", "pipe", "pipe"] },
  ).trim();
}
const witnessRows = z.array(
  z.object({
    pid: z.number(),
    role: z.string(),
    actor: z.string(),
    tenant: z.string().nullable(),
    system: z.string().nullable(),
    tables: z.array(
      z.object({
        name: z.string(),
        superuser: z.boolean(),
        bypass: z.boolean(),
        nonowner: z.boolean(),
        force: z.boolean(),
        active: z.boolean(),
      }),
    ),
  }),
);
function captureTimerNative(page: Page) {
  const serverBin = process.env.FVOCI_E2E_SERVER_BIN;
  if (!serverBin) throw new Error("own server binary missing");
  const configuredBin = realpathSync(serverBin);
  const binaryHash = createHash("sha256").update(readFileSync(configuredBin)).digest("hex");
  const native: TimerNative[] = [];
  const owned = restartedTimerServers.get(page);
  if (owned) {
    expect(owned.child.pid).toBe(owned.witness.pid);
    expect(owned.child.exitCode).toBeNull();
    expect(owned.child.signalCode).toBeNull();
    const argv = readFileSync(`/proc/${String(owned.witness.pid)}/cmdline`, "utf8").split("\0");
    expect(argv[0]).toBe(configuredBin);
    const stat = readFileSync(`/proc/${String(owned.witness.pid)}/stat`, "utf8");
    const parent = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
    expect(parent).toBe(process.pid);
    expect(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[19]).toBe(owned.witness.procStartTicks);
    expect(owned.witness.parentPid).toBe(process.pid);
    expect(owned.witness.configuredBin).toBe(configuredBin);
    expect(owned.witness.knownFileSha256).toBe(binaryHash);
    const origin = readFileSync(owned.log, "utf8")
      .split("\n")
      .find((line) => line.includes("fvoci-server listening on "))
      ?.split("fvoci-server listening on ")[1]
      ?.trim();
    expect(origin).toBe(owned.witness.listeningOrigin);
    expect(origin).toBe(new URL(page.url()).origin);
    process.kill(owned.original.parentPid, 0);
    expect(
      readFileSync(`/proc/${String(owned.original.parentPid)}/cmdline`, "utf8").split("\0"),
    ).toContain(owned.original.launcher);
    native.push({ ...owned.witness });
  }
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      // main.rs deliberately makes the server nondumpable. Bind the readable
      // launch identity to its supervised parent and actual listening origin;
      // /proc/exe identity remains unavailable, rather than disabling hardening.
      const argv = readFileSync(`/proc/${entry}/cmdline`, "utf8").split("\0");
      if (argv[0] !== configuredBin) continue;
      const stat = readFileSync(`/proc/${entry}/stat`, "utf8");
      const parentPid = stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1];
      if (!parentPid || !/^\d+$/.test(parentPid)) continue;
      const parentArgv = readFileSync(`/proc/${parentPid}/cmdline`, "utf8").split("\0");
      const launcher = path.resolve("../../scripts/web-e2e-inner.sh");
      if (!parentArgv.includes(launcher)) continue;
      const allowed = new Set([
        "RUN_DIR",
        "SERVER_LOG",
        "FVOCI_STATIC_DIR",
        "FVOCI_W5_EVIDENCE_DIR",
      ]);
      const namespace = new Map<string, string>();
      for (const value of readFileSync(`/proc/${parentPid}/environ`, "utf8").split("\0")) {
        const separator = value.indexOf("=");
        const key = value.slice(0, separator);
        if (separator > 0 && allowed.has(key)) namespace.set(key, value.slice(separator + 1));
      }
      if (
        namespace.get("RUN_DIR") !== process.env.FVOCI_E2E_RESULT_DIR ||
        namespace.get("FVOCI_STATIC_DIR") !== process.env.FVOCI_STATIC_DIR ||
        namespace.get("FVOCI_W5_EVIDENCE_DIR") !== process.env.FVOCI_W5_EVIDENCE_DIR
      )
        continue;
      const serverLog = namespace.get("SERVER_LOG");
      if (!serverLog) continue;
      const origin = readFileSync(serverLog, "utf8")
        .split("\n")
        .find((line) => line.includes("fvoci-server listening on "))
        ?.split("fvoci-server listening on ")[1]
        ?.trim();
      if (origin !== new URL(page.url()).origin) continue;
      native.push({
        pid: Number(entry),
        parentPid: Number(parentPid),
        launcher,
        configuredBin,
        knownFileSha256: binaryHash,
        listeningOrigin: origin,
        procExeIdentity: "NOTCAPTURED: intentional nondumpability",
        procStartTicks: stat.slice(stat.lastIndexOf(")") + 2).split(" ")[19] ?? "MISSING",
      });
    } catch {
      /* Processes can disappear between directory read and inspection. */
    }
  }
  expect(createHash("sha256").update(readFileSync(configuredBin)).digest("hex")).toBe(binaryHash);
  return { native, configuredBin, binaryHash };
}

test.beforeAll(() => {
  const { role } = diagnosticDatabase();
  if (!/^[a-zA-Z0-9_]+$/.test(role)) throw new Error("unexpected isolated role identifier");
  diagnosticSql(`
    CREATE TABLE IF NOT EXISTS public.w5_timer_runtime_proof (id uuid PRIMARY KEY, value jsonb NOT NULL);
    GRANT INSERT ON public.w5_timer_runtime_proof TO "${role}";
    CREATE OR REPLACE FUNCTION public.w5_timer_runtime_witness() RETURNS trigger
      LANGUAGE plpgsql SECURITY INVOKER SET search_path='' AS $$
    BEGIN
      INSERT INTO public.w5_timer_runtime_proof(id,value)
      SELECT NEW.id,jsonb_build_object('pid',pg_backend_pid(),'role',current_user,'actor',public.app_self_user_id(),
        'tenant',nullif(current_setting('app.tenant_id',true),''),'system',nullif(current_setting('app.system_ctx',true),''),
        'tables',(SELECT jsonb_agg(jsonb_build_object('name',c.relname,'superuser',r.rolsuper,'bypass',r.rolbypassrls,
          'nonowner',c.relowner<>r.oid,'force',c.relforcerowsecurity,'active',row_security_active(c.oid)) ORDER BY c.relname)
          FROM pg_roles r CROSS JOIN pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
          WHERE r.rolname=current_user AND n.nspname='fvoci' AND c.relname IN
          ('time_entries','task_timer_runs','task_timer_segments','task_timer_legacy_open','task_timer_commands','task_timer_audit')));
      RETURN NEW;
    END; $$;
    DROP TRIGGER IF EXISTS w5_timer_runtime_witness ON fvoci.task_timer_audit;
    CREATE TRIGGER w5_timer_runtime_witness AFTER INSERT ON fvoci.task_timer_audit
      FOR EACH ROW EXECUTE FUNCTION public.w5_timer_runtime_witness();
  `);
});
// Read entrypoints in the fresh about:blank document before any test navigation.
// Capture actual browser responses while the app loads: auth retirement may
// destroy the final document before afterEach, but cannot replace this evidence.
type EntrypointEvidence = {
  path: string;
  url: string;
  documentUrl: string;
  pageUrl: string;
  servedUrl: string;
  native: TimerNative;
  browserLoadedSha256: string;
  servedSha256: string;
  ownStaticSha256: string;
};
type EntrypointCapture = EntrypointEvidence | { url: string; error: unknown };
const loadedEntrypoints = new WeakMap<
  Page,
  {
    paths: string[];
    responses: Map<string, Promise<EntrypointCapture>>;
    errors: Array<{ url: string; error: unknown }>;
  }
>();
function nativeGeneration(native: TimerNative) {
  return `${String(native.pid)}:${native.procStartTicks}:${native.listeningOrigin}`;
}
function assertEntrypointEvidence(evidence: EntrypointEvidence) {
  const origin = new URL(evidence.url).origin;
  expect(new URL(evidence.url).pathname).toBe(evidence.path);
  expect(origin).toBe(new URL(evidence.documentUrl).origin);
  expect(origin).toBe(new URL(evidence.pageUrl).origin);
  expect(origin).toBe(new URL(evidence.servedUrl).origin);
  expect(new URL(evidence.servedUrl).pathname).toBe(evidence.path);
  expect(origin).toBe(evidence.native.listeningOrigin);
  expect(evidence.native.pid).toBeGreaterThan(0);
  expect(evidence.native.procStartTicks).toMatch(/^\d+$/);
  expect(evidence.native.knownFileSha256).toMatch(/^[0-9a-f]{64}$/);
  expect(evidence.browserLoadedSha256, evidence.path).toBe(evidence.ownStaticSha256);
  expect(evidence.servedSha256, evidence.path).toBe(evidence.ownStaticSha256);
}
async function drainLoadedEntrypoints(page: Page) {
  const entrypoints = loadedEntrypoints.get(page);
  if (!entrypoints) throw new Error("browser entrypoint capture missing");
  if (entrypoints.errors.length) throw entrypoints.errors[0]?.error;
  const assets: EntrypointEvidence[] = [];
  for (const pending of entrypoints.responses.values()) {
    const loaded = await pending;
    if ("error" in loaded)
      throw new Error(`browser/served entrypoint capture failed: ${loaded.url}`, {
        cause: loaded.error,
      });
    assertEntrypointEvidence(loaded);
    assets.push(loaded);
  }
  // Events may fail while an earlier body's promise is being awaited. They
  // remain hard failures even when the completed records themselves are valid.
  if (entrypoints.errors.length) throw entrypoints.errors[0]?.error;
  return assets;
}
async function verifyLoadedEntrypoints(page: Page, native: TimerNative[]) {
  const entrypoints = loadedEntrypoints.get(page);
  if (!entrypoints) throw new Error("browser entrypoint capture missing");
  const assets = await drainLoadedEntrypoints(page);
  expect(native).toHaveLength(1);
  const current = native[0];
  if (!current) throw new Error("current native generation missing");
  for (const pathname of entrypoints.paths) {
    expect(
      assets.some(
        (asset) =>
          asset.path === pathname && nativeGeneration(asset.native) === nativeGeneration(current),
      ),
      `entrypoint was not loaded by this native generation: ${pathname}`,
    ).toBe(true);
  }
  return assets;
}
test.beforeEach(async ({ page }) => {
  const staticDir = process.env.FVOCI_STATIC_DIR;
  if (!staticDir) throw new Error("own static namespace missing");
  const paths = await page.evaluate(
    (html) => {
      const document = new DOMParser().parseFromString(html, "text/html");
      return Array.from(document.querySelectorAll('script[src*="/assets/"]'), (element) =>
        element.getAttribute("src"),
      ).filter((value): value is string => Boolean(value));
    },
    readFileSync(path.join(staticDir, "index.html"), "utf8"),
  );
  expect(paths.length).toBeGreaterThan(0);
  const responses = new Map<string, Promise<EntrypointCapture>>();
  const errors: Array<{ url: string; error: unknown }> = [];
  loadedEntrypoints.set(page, { paths, responses, errors });
  const goto = page.goto.bind(page);
  page.goto = async (...args) => {
    await drainLoadedEntrypoints(page);
    return goto(...args);
  };
  const reload = page.reload.bind(page);
  page.reload = async (...args) => {
    await drainLoadedEntrypoints(page);
    return reload(...args);
  };
  const close = page.close.bind(page);
  page.close = async (...args) => {
    await drainLoadedEntrypoints(page);
    return close(...args);
  };
  page.on("response", (response) => {
    const pathname = new URL(response.url()).pathname;
    if (
      response.request().resourceType() === "script" &&
      paths.includes(pathname) &&
      response.ok()
    ) {
      const url = response.url();
      try {
        expect(response.frame()).toBe(page.mainFrame());
        const documentUrl = response.frame().url();
        const pageUrl = page.url();
        const owned = restartedTimerServers.get(page)?.witness;
        // The original wrapper is immutable for this page's test namespace;
        // each physical replacement has an explicit PID/startTicks witness.
        const generation = owned
          ? nativeGeneration(owned)
          : `initial-wrapper:${new URL(url).origin}`;
        const key = `${generation}:${pathname}`;
        if (responses.has(key)) return;
        // Enroll CDP's real body read immediately. Native hashing/proc reads
        // must wait until these bytes are safe from an auth/navigation reload.
        const browserBody = response.body();
        // Independent HTTP bytes are read at capture, before this server can
        // stop. Keep every generation's first browser response, never overwrite.
        const capture = async (): Promise<EntrypointEvidence> => {
          const body = await browserBody;
          const { native } = captureTimerNative(page);
          expect(native).toHaveLength(1);
          const witness = native[0];
          if (!witness) throw new Error("browser response native generation missing");
          if (owned) expect(nativeGeneration(witness)).toBe(nativeGeneration(owned));
          const served = await page.request.get(url);
          expect(served.ok(), url).toBe(true);
          const evidence = {
            path: pathname,
            url,
            documentUrl,
            pageUrl,
            servedUrl: served.url(),
            native: witness,
            browserLoadedSha256: createHash("sha256").update(body).digest("hex"),
            servedSha256: createHash("sha256")
              .update(await served.body())
              .digest("hex"),
            ownStaticSha256: createHash("sha256")
              .update(readFileSync(path.join(staticDir, pathname)))
              .digest("hex"),
          };
          assertEntrypointEvidence(evidence);
          return evidence;
        };
        responses.set(
          key,
          capture().catch((error: unknown) => ({ url, error })),
        );
      } catch (error) {
        // Event callbacks cannot await. Retain the original hard failure for
        // the pre-stop fence/afterEach instead of losing an unhandled rejection.
        errors.push({ url, error });
      }
    }
  });
});
test.afterEach(async ({ page }, testInfo) => {
  const rows = witnessRows.parse(
    JSON.parse(
      diagnosticSql(
        "SELECT COALESCE(jsonb_agg(value ORDER BY id),'[]'::jsonb) FROM public.w5_timer_runtime_proof",
      ),
    ),
  );
  const { native } = captureTimerNative(page);
  const assets = await verifyLoadedEntrypoints(page, native);
  const { container, database, role } = diagnosticDatabase();
  const proof = {
    test: testInfo.title,
    rows,
    native,
    assets,
    serverOrigin: new URL(page.url()).origin,
    container,
    database,
    role,
  };
  const target = testInfo.outputPath("timer-runtime-proof.json");
  writeFileSync(target, JSON.stringify(proof, null, 2));
  await testInfo.attach("timer-runtime-proof", {
    path: target,
    contentType: "application/json",
  });
  const evidenceDir = process.env.FVOCI_W5_EVIDENCE_DIR;
  if (evidenceDir) {
    mkdirSync(evidenceDir, { recursive: true });
    writeFileSync(
      path.join(evidenceDir, `runtime-${testInfo.testId}.json`),
      JSON.stringify(proof, null, 2),
    );
  }
  expect(native).toHaveLength(1);
  expect(assets.length).toBeGreaterThan(0);
  // View-only case may have no write in its fresh worker. The first flow and
  // revocation case must prove the real server invoker's context before denial.
  if (!testInfo.title.startsWith("MyTasks timer controls")) expect(rows.length).toBeGreaterThan(0);
  for (const row of rows) {
    expect(row.role).toBe(role);
    expect(row.system).toBeNull();
    expect(row.tables).toHaveLength(6);
    for (const table of row.tables) {
      expect(table.superuser).toBe(false);
      expect(table.bypass).toBe(false);
      expect(table.nonowner).toBe(true);
      expect(table.force).toBe(true);
      expect(table.active).toBe(true);
    }
  }
});
test.afterAll(() => {
  diagnosticSql(
    "DROP TRIGGER IF EXISTS w5_timer_runtime_witness ON fvoci.task_timer_audit; DROP FUNCTION IF EXISTS public.w5_timer_runtime_witness(); DROP TABLE IF EXISTS public.w5_timer_runtime_proof;",
  );
});

test("detail start commits server intervals; a new MyTasks client pauses, resumes and stops the same task", async ({
  page,
  browser,
}) => {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃" })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("연구");
    await page.getByLabel("이메일").fill(credentials.email);
    await page.getByLabel("비밀번호").fill(credentials.password);
    await page.getByLabel("워크스페이스 이름").fill("읽기 연구 계획");
    await page.getByLabel("주소(영문)").fill("w5timer");
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else await login(page, credentials.email, credentials.password);
  const workspaceResponse = await page.request.get("/api/v1/me/workspaces");
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = workspaces
    .parse(await workspaceResponse.json())
    .items.find((row) => row.slug === "w5timer");
  expect(workspace).toBeTruthy();
  if (!workspace) throw new Error("missing timer fixture workspace");
  const me = z
    .object({ userId: z.string() })
    .parse(await (await page.request.get("/api/v1/auth/me")).json());
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key: "READ", name: "연구 목표와 자료", visibility: "workspace" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  const taskResponse = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${project.id}/tasks`,
    { data: { title: "한글 자료 읽기와 연결된 연구 노트" } },
  );
  expect(taskResponse.status(), await taskResponse.text()).toBe(201);
  const task = taskShape.parse(await taskResponse.json());
  const assigned = await page.request.patch(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`, {
    data: { assigneeIds: [me.userId], estimate: "30" },
  });
  expect(assigned.ok(), await assigned.text()).toBe(true);
  const detailUrl = `/w/w5timer/READ-${String(task.number)}`;
  const apiUrl = `/api/v1/workspaces/${workspace.id}/tasks/${task.id}/timer`;
  await page.goto(detailUrl);
  const detail = page.getByTestId(`task-stopwatch-${task.id}`);
  await expect(detail.getByTestId("timer-start")).toBeEnabled();
  await detail.getByLabel("측정 메모").fill("읽기 구간");
  await detail.getByTestId("timer-start").click();
  await expect(detail.getByTestId("timer-state")).toHaveText("측정 중");
  const committed = timerShape.parse(await (await page.request.get(apiUrl)).json());
  expect(committed.run?.status).toBe("running");
  // Fresh browser context/session, not this page's optimistic state/cache.
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const fresh = await context.newPage();
    await login(fresh, credentials.email, credentials.password);
    await fresh.goto("/w/w5timer/my-tasks");
    const mine = fresh.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mine.getByTestId("timer-state")).toHaveText("측정 중");
    expect(timerShape.parse(await (await fresh.request.get(apiUrl)).json()).run?.id).toBe(
      committed.run?.id,
    );
    await mine.getByTestId("timer-pause").click();
    await expect(mine.getByTestId("timer-state")).toHaveText("일시정지");
    const paused = timerShape.parse(await (await fresh.request.get(apiUrl)).json());
    expect(paused.run?.runningSince).toBeNull();
    await fresh.reload();
    await expect(mine.getByTestId("timer-state")).toHaveText("일시정지");
    const stillPaused = timerShape.parse(await (await fresh.request.get(apiUrl)).json());
    expect(stillPaused.run?.elapsedMilliseconds).toBe(paused.run?.elapsedMilliseconds);
    await mine.getByTestId("timer-resume").click();
    await expect(mine.getByTestId("timer-state")).toHaveText("측정 중");
    await mine.getByTestId("timer-stop").click();
    await expect(mine.getByTestId("timer-start")).toBeEnabled();
    expect(timerShape.parse(await (await fresh.request.get(apiUrl)).json()).run).toBeNull();
    const after = taskShape.parse(
      await (await fresh.request.get(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`)).json(),
    );
    expect(after.statusId).toBe(task.statusId);
    await page.reload();
    await expect(detail.getByTestId("timer-start")).toBeEnabled();
    await expect(detail.getByTestId("timer-actual")).toHaveText(
      await mine.getByTestId("timer-actual").innerText(),
    );
  } finally {
    await context.close();
  }
});

async function timerWorkspace(page: import("@playwright/test").Page): Promise<string> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃" })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("연구");
    await page.getByLabel("이메일").fill(credentials.email);
    await page.getByLabel("비밀번호").fill(credentials.password);
    await page.getByLabel("워크스페이스 이름").fill("읽기 연구 계획");
    await page.getByLabel("주소(영문)").fill("w5timer");
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else await login(page, credentials.email, credentials.password);
  const workspaceResponse = await page.request.get("/api/v1/me/workspaces");
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = workspaces
    .parse(await workspaceResponse.json())
    .items.find((row) => row.slug === "w5timer");
  expect(workspace).toBeTruthy();
  if (!workspace) throw new Error("missing timer fixture workspace");
  return workspace.id;
}

async function permissionFixture(
  page: import("@playwright/test").Page,
  key: string,
  role: "member" | "viewer",
) {
  const workspaceId = await timerWorkspace(page);
  const email = `${key.toLowerCase()}@example.com`;
  createE2eUser(email, credentials.password, "권한 검사", {
    workspaceSlug: "w5timer",
    membershipRole: "member",
  });
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key, name: "권한 회수 검사", visibility: "private" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  return { workspaceId, email, project, role };
}

test("authoritative revoked timer GET retires mounted private state while 503 preserves the server anchor", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await permissionFixture(page, "TACL", "member");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const editor = await context.newPage();
    await login(editor, fixture.email, credentials.password);
    const me = identityShape.parse(await (await editor.request.get("/api/v1/auth/me")).json());
    const memberUrl = `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`;
    const grant = await page.request.post(memberUrl, {
      data: { userId: me.userId, role: fixture.role },
    });
    expect(grant.ok(), await grant.text()).toBe(true);
    const taskResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      { data: { title: "권한 회수 뒤 사적인 측정" } },
    );
    expect(taskResponse.status(), await taskResponse.text()).toBe(201);
    const task = taskShape.parse(await taskResponse.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    const sentinelProjectResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects`,
      { data: { key: "SAFE", name: "계속 볼 수 있는 작업", visibility: "workspace" } },
    );
    expect(sentinelProjectResponse.status(), await sentinelProjectResponse.text()).toBe(201);
    const sentinelProject = z
      .object({ id: z.string() })
      .parse(await sentinelProjectResponse.json());
    const sentinelResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${sentinelProject.id}/tasks`,
      { data: { title: "권한 회수와 무관한 내 작업" } },
    );
    expect(sentinelResponse.status(), await sentinelResponse.text()).toBe(201);
    const sentinelTask = taskShape.parse(await sentinelResponse.json());
    const sentinelAssign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${sentinelTask.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(sentinelAssign.ok(), await sentinelAssign.text()).toBe(true);
    await editor.goto("/w/w5timer/my-tasks");
    const mounted = editor.getByTestId(`task-stopwatch-${task.id}`);
    const sentinel = editor.getByTestId(`task-stopwatch-${sentinelTask.id}`);
    await expect(sentinel.getByTestId("timer-actual")).toBeVisible();
    const sentinelActual = await sentinel.getByTestId("timer-actual").innerText();
    await expect(mounted.getByTestId("timer-start")).toBeEnabled();
    await mounted.getByTestId("timer-start").click();
    await expect(mounted.getByTestId("timer-state")).toHaveText("측정 중");
    const committed = timerShape.parse(await (await editor.request.get(timerUrl)).json());
    expect(committed.run?.id).toBeTruthy();
    const timerMatch = (url: URL) => url.pathname === timerUrl;
    const unavailableHandler = async (route: Route) => {
      if (route.request().method() !== "GET") return route.continue();
      const capture = new URL(route.request().url()).searchParams;
      expect(capture.get("expectedActorId")).toBe(me.userId);
      expect(capture.get("expectedSessionId")).toBe(me.sessionId);
      await route.fulfill({
        status: 503,
        contentType: "application/problem+json",
        body: JSON.stringify({ type: "about:blank", title: "일시적인 연결 실패", status: 503 }),
      });
    };
    await editor.route(timerMatch, unavailableHandler);
    const unavailableWaitStarted = Date.now();
    const unavailable = editor.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 503,
    );
    // Keep the actual consumer foregrounded, then observe its existing 5s
    // polling response before the unchanged UI assertions. The app disables
    // refetchOnWindowFocus, so this does not pretend focus itself refetches.
    await editor.bringToFront();
    await unavailable;
    await testInfo.attach("timer-real-503-precondition", {
      body: JSON.stringify({
        status: 503,
        method: "GET",
        observedWaitMilliseconds: Date.now() - unavailableWaitStarted,
      }),
      contentType: "application/json",
    });
    await expect(mounted.getByRole("alert")).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveText("측정 중");
    await expect(mounted.getByTestId("timer-actual")).toBeVisible();
    const sentinelTimerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${sentinelTask.id}/timer`;
    const sentinelUnavailable = editor.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === sentinelTimerUrl &&
        response.request().method() === "GET" &&
        response.status() === 503,
    );
    await editor.route(
      (url) => url.pathname === sentinelTimerUrl,
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const capture = new URL(route.request().url()).searchParams;
        expect(capture.get("expectedActorId")).toBe(me.userId);
        expect(capture.get("expectedSessionId")).toBe(me.sessionId);
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "다른 작업 연결 실패", status: 503 }),
        });
      },
    );
    await sentinelUnavailable;
    await expect(sentinel.getByTestId("timer-actual")).toHaveText(sentinelActual);
    // Only the unrelated list transport is held to keep the original consumer
    // mounted. The timer denial below comes from real Rust/current project ACL.
    await editor.route(
      (url) => url.pathname === `/api/v1/workspaces/${fixture.workspaceId}/tasks`,
      async (route) => {
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "목록 연결 실패", status: 503 }),
        });
      },
    );
    await editor.unroute(timerMatch, unavailableHandler);
    const denialWaitStarted = Date.now();
    const realDenial = editor.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 404,
    );
    const revoke = await page.request.delete(`${memberUrl}/${me.userId}`);
    expect(revoke.ok(), await revoke.text()).toBe(true);
    const denial = await realDenial;
    await testInfo.attach("timer-real-404-precondition", {
      body: JSON.stringify({
        status: denial.status(),
        method: "GET",
        observedWaitMilliseconds: Date.now() - denialWaitStarted,
      }),
      contentType: "application/json",
    });
    expect((await denial.text()).includes("권한 회수 뒤 사적인 측정")).toBe(false);
    await expect(mounted).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-actual")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-elapsed")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-start")).toBeDisabled();
    await expect(sentinel.getByTestId("timer-actual")).toHaveText(sentinelActual);
    await expect(sentinel.getByTestId("timer-elapsed")).toBeVisible();
    expect((await editor.request.get(sentinelTimerUrl)).status()).toBe(200);
    const actualDenied = await editor.request.get(timerUrl);
    expect(actualDenied.status()).toBe(404);
    const membership = await editor.request.get("/api/v1/me/workspaces");
    expect(
      workspaces.parse(await membership.json()).items.some((row) => row.id === fixture.workspaceId),
    ).toBe(true);
  } finally {
    await context.close();
  }
});

test("MyTasks timer controls follow actual View Edit archive and demotion capability", async ({
  page,
  browser,
}) => {
  const fixture = await permissionFixture(page, "TCAP", "viewer");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const viewer = await context.newPage();
    await login(viewer, fixture.email, credentials.password);
    const me = z
      .object({ userId: z.string() })
      .parse(await (await viewer.request.get("/api/v1/auth/me")).json());
    const membersUrl = `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`;
    const grant = await page.request.post(membersUrl, {
      data: { userId: me.userId, role: "viewer" },
    });
    expect(grant.ok(), await grant.text()).toBe(true);
    const taskResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      { data: { title: "읽기 권한으로 보는 내 작업" } },
    );
    expect(taskResponse.status(), await taskResponse.text()).toBe(201);
    const task = taskShape.parse(await taskResponse.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const entriesUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/time-entries`;
    const capability = async () =>
      z
        .object({ canCreate: z.boolean() })
        .parse(await (await viewer.request.get(entriesUrl)).json()).canCreate;
    expect(await capability()).toBe(false);
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    const initialControl = viewer.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 200 &&
        !z.object({ canControl: z.boolean() }).parse(await response.json()).canControl,
    );
    await viewer.goto("/w/w5timer/my-tasks");
    await initialControl;
    const timer = viewer.getByTestId(`task-stopwatch-${task.id}`);
    await expect(timer).toBeVisible();
    await expect(timer.getByTestId("timer-actual")).toBeVisible();
    await expect(timer.getByTestId("timer-elapsed")).toBeVisible();
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const nextControl = (expected: boolean) =>
      viewer.waitForResponse(async (response) => {
        if (
          new URL(response.url()).pathname !== timerUrl ||
          response.request().method() !== "GET" ||
          response.status() !== 200
        )
          return false;
        return (
          z.object({ canControl: z.boolean() }).parse(await response.json()).canControl === expected
        );
      });
    const promotedControl = nextControl(true);
    const promote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(promote.ok(), await promote.text()).toBe(true);
    expect(await capability()).toBe(true);
    await promotedControl;
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const demotedControl = nextControl(false);
    const demote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "viewer" },
    });
    expect(demote.ok(), await demote.text()).toBe(true);
    expect(await capability()).toBe(false);
    await demotedControl;
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const restoredControl = nextControl(true);
    const again = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(again.ok(), await again.text()).toBe(true);
    await restoredControl;
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const archivedControl = nextControl(false);
    const archive = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/archive`,
    );
    expect(archive.ok(), await archive.text()).toBe(true);
    expect(await capability()).toBe(false);
    await archivedControl;
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
  } finally {
    await context.close();
  }
});

test("captured timer GET cannot cache another actor run under a stale mounted identity", async ({
  page,
  browser,
}, testInfo) => {
  await timerWorkspace(page);
  // Keep this identity boundary independent of earlier tests' project streams.
  // The real MyTasks consumer still installs its normal workspace/project SSEs.
  const slug = "w5timer-cookie";
  const workspaceResponse = await page.request.post("/api/v1/workspaces", {
    data: { name: "측정 작성자 경계", slug },
  });
  expect(workspaceResponse.status(), await workspaceResponse.text()).toBe(201);
  const workspaceId = z.object({ id: z.string() }).parse(await workspaceResponse.json()).id;
  const email = "cookie@example.com";
  createE2eUser(email, credentials.password, "작성자 경계", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "COOKIE", name: "작성자 경계", visibility: "private" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const fixture = {
    workspaceId,
    email,
    project: z.object({ id: z.string() }).parse(await projectResponse.json()),
  };
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let releaseOwner = () => {};
  let releaseMe = () => {};
  try {
    const other = await context.newPage();
    await login(other, fixture.email, credentials.password);
    const identity = identityShape;
    const actorA = identity.parse(await (await page.request.get("/api/v1/auth/me")).json());
    const actorB = identity.parse(await (await other.request.get("/api/v1/auth/me")).json());
    expect(actorB.userId).not.toBe(actorA.userId);
    const grant = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`,
      {
        data: { userId: actorB.userId, role: "member" },
      },
    );
    expect(grant.ok(), await grant.text()).toBe(true);
    const created = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      {
        data: { title: "같은 작업에서 분리된 개인 측정" },
      },
    );
    expect(created.status(), await created.text()).toBe(201);
    const task = taskShape.parse(await created.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      {
        data: { assigneeIds: [actorA.userId] },
      },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    const actorARun = await startPausedTimer(page, timerUrl, actorA, "이전 작성자의 실제 구간");
    const start = await other.request.post(timerUrl, {
      data: {
        expectedActorId: actorB.userId,
        expectedSessionId: actorB.sessionId,
        requestId: crypto.randomUUID(),
        operation: "start",
        expectedVersion: 0,
        runId: null,
        note: "다른 작성자의 사적인 구간",
      },
    });
    expect(start.ok(), await start.text()).toBe(true);
    const run = z.object({ runId: z.string(), version: z.number() }).parse(await start.json());
    const pause = await other.request.post(timerUrl, {
      data: {
        expectedActorId: actorB.userId,
        expectedSessionId: actorB.sessionId,
        requestId: crypto.randomUUID(),
        operation: "pause",
        expectedVersion: run.version,
        runId: run.runId,
      },
    });
    expect(pause.ok(), await pause.text()).toBe(true);
    await page.goto(`/w/${slug}/my-tasks`);
    const mounted = page.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mounted.getByTestId("timer-actual")).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
    await expect(mounted.getByTestId("timer-elapsed")).toBeVisible();
    await expect(mounted.getByTestId("timer-resume")).toBeEnabled();
    await expect(page.getByTestId("timer-owner")).toBeVisible();
    const actorRead = timerShape.parse(await (await page.request.get(timerUrl)).json());
    expect(actorRead.run?.id).toBe(actorARun.runId);
    const meDelivery = new Promise<void>((resolve) => {
      releaseMe = resolve;
    });
    await page.route(
      (url) => url.pathname === "/api/v1/auth/me",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const native = await route.fetch();
        // Deliver the unchanged real identity only after both old consumers have
        // consumed their denials. Product identity/navigation guards remain live.
        await meDelivery;
        if (!page.isClosed()) await route.fulfill({ response: native });
      },
    );
    const ownerDelivery = new Promise<void>((resolve) => {
      releaseOwner = resolve;
    });
    type OwnerObservation = {
      status: number;
      capturedActorParameter: string | null;
      capturedSessionParameter: string | null;
      returnedOtherRun: boolean;
    };
    let observeOwner: (value: OwnerObservation) => void = () => {};
    const ownerRead = new Promise<OwnerObservation>((resolve) => {
      observeOwner = resolve;
    });
    await page.route(
      (url) => url.pathname === "/api/v1/me/task-timer",
      async (route) => {
        // Fetch the actual Rust response with the browser's original headers.
        // Hold only its delivery so me refresh cannot cancel the task oracle.
        if (route.request().method() !== "GET") return route.continue();
        const url = new URL(route.request().url());
        if (url.searchParams.get("expectedSessionId") !== actorA.sessionId) return route.continue();
        const native = await route.fetch();
        const value = z
          .object({ runId: z.string().nullable().optional() })
          .parse(await native.json());
        observeOwner({
          status: native.status(),
          capturedActorParameter: url.searchParams.get("expectedActorId"),
          capturedSessionParameter: url.searchParams.get("expectedSessionId"),
          returnedOtherRun: value.runId === run.runId,
        });
        await ownerDelivery;
        if (!page.isClosed()) await route.fulfill({ response: native });
      },
    );
    await page.route(
      (url) => url.pathname === timerUrl,
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        if (
          new URL(route.request().url()).searchParams.get("expectedSessionId") !== actorA.sessionId
        )
          return route.continue();
        const native = await route.fetch();
        // Both ordinary polls must reach Rust before either denial can refresh
        // me and cancel the other captured scope. No query is forced or forged.
        await ownerRead;
        if (!page.isClosed()) await route.fulfill({ response: native });
      },
    );
    const switchedResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === timerUrl && response.request().method() === "GET",
    );
    // A real, independently logged-in credential replaces the cookie while
    // the actual mounted me/query capture still belongs to actor A.
    await page.context().addCookies(await context.cookies());
    await page.bringToFront();
    const response = await switchedResponse;
    const ownerObservation = await ownerRead;
    const body: unknown = await response.json();
    const observed = z
      .object({ run: z.object({ id: z.string() }).nullable().optional() })
      .passthrough()
      .parse(body);
    const authenticated = identity.parse(await (await page.request.get("/api/v1/auth/me")).json());
    expect(authenticated.userId).toBe(actorB.userId);
    if (response.ok() && observed.run?.id === run.runId) {
      // Record the actual mounted consumer's effect before the guard oracle.
      await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
    }
    // The native denial must arrive while the nonempty old consumer remains
    // mounted. Detachment/navigation cannot stand in for privacy retirement.
    expect(response.status()).toBe(409);
    await expect(mounted).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-actual")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-elapsed")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-start")).toBeDisabled();
    await expect(page.getByTestId("timer-owner")).toBeVisible();
    const ownerDelivered = page.waitForResponse(
      (reply) =>
        new URL(reply.url()).pathname === "/api/v1/me/task-timer" &&
        new URL(reply.url()).searchParams.get("expectedSessionId") === actorA.sessionId &&
        reply.status() === 409,
    );
    releaseOwner();
    await ownerDelivered;
    await expect(page.getByTestId("timer-owner")).toHaveCount(0);
    await expect(mounted).toBeVisible();
    const successorOwner = page.waitForResponse(
      (reply) =>
        new URL(reply.url()).pathname === "/api/v1/me/task-timer" &&
        new URL(reply.url()).searchParams.get("expectedActorId") === actorB.userId &&
        new URL(reply.url()).searchParams.get("expectedSessionId") === actorB.sessionId &&
        reply.status() === 200,
    );
    releaseMe();
    const successor = await successorOwner;
    expect(z.object({ runId: z.string().nullable() }).parse(await successor.json()).runId).toBe(
      run.runId,
    );
    await expect(page.getByTestId("timer-owner")).toBeVisible();
    await testInfo.attach("timer-cookie-read-guard", {
      body: JSON.stringify({
        status: response.status(),
        capturedActorParameter: new URL(response.url()).searchParams.get("expectedActorId"),
        capturedSessionParameter: new URL(response.url()).searchParams.get("expectedSessionId"),
        expectedActor: actorA.userId,
        expectedSession: actorA.sessionId,
        authenticatedActor: authenticated.userId,
        returnedOtherRun: observed.run?.id === run.runId,
        oldTaskRetirementObservedBeforeIdentityDelivery: true,
        oldOwnerRetirementObservedBeforeIdentityDelivery: true,
        successorAuthorizedOwnerRun: run.runId,
        ownerObservation,
      }),
      contentType: "application/json",
    });
    expect(
      response.status(),
      "stale scoped query must be rejected before any other actor state",
    ).toBe(409);
    expect(new URL(response.url()).searchParams.get("expectedActorId")).toBe(actorA.userId);
    expect(new URL(response.url()).searchParams.get("expectedSessionId")).toBe(actorA.sessionId);
    expect(observed.run).toBeUndefined();
    expect(ownerObservation.status).toBe(409);
    expect(ownerObservation.capturedActorParameter).toBe(actorA.userId);
    expect(ownerObservation.capturedSessionParameter).toBe(actorA.sessionId);
    expect(ownerObservation.returnedOtherRun).toBe(false);
  } finally {
    releaseOwner();
    releaseMe();
    await context.close();
  }
});

async function startPausedTimer(
  page: import("@playwright/test").Page,
  timerUrl: string,
  actor: z.infer<typeof identityShape>,
  note: string,
) {
  const start = await page.request.post(timerUrl, {
    data: {
      expectedActorId: actor.userId,
      expectedSessionId: actor.sessionId,
      requestId: crypto.randomUUID(),
      operation: "start",
      expectedVersion: 0,
      runId: null,
      note,
    },
  });
  expect(start.ok(), await start.text()).toBe(true);
  const run = z.object({ runId: z.string(), version: z.number() }).parse(await start.json());
  const pause = await page.request.post(timerUrl, {
    data: {
      expectedActorId: actor.userId,
      expectedSessionId: actor.sessionId,
      requestId: crypto.randomUUID(),
      operation: "pause",
      expectedVersion: run.version,
      runId: run.runId,
    },
  });
  expect(pause.ok(), await pause.text()).toBe(true);
  return z.object({ runId: z.string(), version: z.number() }).parse(await pause.json());
}

test("a same-actor new-session denial cannot retire a populated successor or returning scope", async ({
  page,
  browser,
}, testInfo) => {
  await timerWorkspace(page);
  const slug = "w5timer-session";
  const workspaceResponse = await page.request.post("/api/v1/workspaces", {
    data: { name: "세션 전환 측정", slug },
  });
  expect(workspaceResponse.status(), await workspaceResponse.text()).toBe(201);
  const workspaceId = z.object({ id: z.string() }).parse(await workspaceResponse.json()).id;
  const email = "timer-session@example.com";
  createE2eUser(email, credentials.password, "같은 작성자의 세션", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "SESSION", name: "세션 전환 측정", visibility: "private" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  const firstContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  const secondContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let releaseTransitionMe = () => {};
  let transitionMeDelivery = Promise.resolve();
  const events: {
    kind: string;
    endpoint: string;
    session: string | null;
    status?: number;
    failure?: string;
  }[] = [];
  try {
    const first = await firstContext.newPage();
    const second = await secondContext.newPage();
    await login(first, email, credentials.password);
    await login(second, email, credentials.password);
    const s1 = identityShape.parse(await (await first.request.get("/api/v1/auth/me")).json());
    const s2 = identityShape.parse(await (await second.request.get("/api/v1/auth/me")).json());
    expect(s1.userId).toBe(s2.userId);
    expect(s1.sessionId).not.toBe(s2.sessionId);
    const grant = await page.request.post(
      `/api/v1/workspaces/${workspaceId}/projects/${project.id}/members`,
      {
        data: { userId: s1.userId, role: "member" },
      },
    );
    expect(grant.ok(), await grant.text()).toBe(true);
    const created = await page.request.post(
      `/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`,
      {
        data: { title: "새 세션에도 남아야 하는 개인 구간" },
      },
    );
    expect(created.status(), await created.text()).toBe(201);
    const task = taskShape.parse(await created.json());
    const assign = await page.request.patch(`/api/v1/workspaces/${workspaceId}/tasks/${task.id}`, {
      data: { assigneeIds: [s1.userId] },
    });
    expect(assign.ok(), await assign.text()).toBe(true);
    const timerUrl = `/api/v1/workspaces/${workspaceId}/tasks/${task.id}/timer`;
    const ownerUrl = "/api/v1/me/task-timer";
    const run = await startPausedTimer(first, timerUrl, s1, "실제 세션 경계 구간");
    const control = await second.request.get(timerUrl, {
      params: {
        expectedActorId: s2.userId,
        expectedSessionId: s2.sessionId,
      },
    });
    expect(control.status(), await control.text()).toBe(200);
    expect(timerShape.parse(await control.json()).run?.id).toBe(run.runId);
    await first.goto(`/w/${slug}/my-tasks`);
    const mounted = first.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
    await expect(mounted.getByTestId("timer-resume")).toBeEnabled();
    await expect(first.getByTestId("timer-owner")).toBeVisible();
    await first.route(
      (url) => url.pathname === "/api/v1/auth/me",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const native = await route.fetch();
        const delivery = transitionMeDelivery;
        await delivery;
        if (!first.isClosed()) await route.fulfill({ response: native });
      },
    );
    const cookies1 = await firstContext.cookies();
    const cookies2 = await secondContext.cookies();
    const endpoint = (url: string) => {
      const parsed = new URL(url);
      return [timerUrl, ownerUrl].includes(parsed.pathname) ? parsed : undefined;
    };
    first.on("requestfailed", (request) => {
      const url = endpoint(request.url());
      if (url && request.method() === "GET")
        events.push({
          kind: "requestfailed",
          endpoint: url.pathname,
          session: url.searchParams.get("expectedSessionId"),
          failure: request.failure()?.errorText,
        });
    });
    first.on("response", (response) => {
      const url = endpoint(response.url());
      if (url && response.request().method() === "GET")
        events.push({
          kind: "response",
          endpoint: url.pathname,
          session: url.searchParams.get("expectedSessionId"),
          status: response.status(),
        });
    });
    // Real cookie changes and the existing poll/me refresh exercise S1 -> S2
    // -> S1 -> S2. No injected query/cache, fake auth, timer body, or reload.
    for (const [previous, successor, cookies] of [
      [s1, s2, cookies2],
      [s2, s1, cookies1],
      [s1, s2, cookies2],
    ] as const) {
      const transitionStart = events.length;
      transitionMeDelivery = new Promise<void>((resolve) => {
        releaseTransitionMe = resolve;
      });
      const denied = first.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === timerUrl &&
          new URL(response.url()).searchParams.get("expectedSessionId") === previous.sessionId &&
          response.status() === 409,
      );
      const refreshed = first.waitForResponse(
        async (response) =>
          new URL(response.url()).pathname === "/api/v1/auth/me" &&
          response.status() === 200 &&
          identityShape.parse(await response.json()).sessionId === successor.sessionId,
      );
      const successorRead = first.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === timerUrl &&
          new URL(response.url()).searchParams.get("expectedSessionId") === successor.sessionId &&
          response.status() === 200,
      );
      const successorOwner = first.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === ownerUrl &&
          new URL(response.url()).searchParams.get("expectedSessionId") === successor.sessionId &&
          response.status() === 200,
      );
      await firstContext.addCookies(cookies);
      await first.bringToFront();
      const oldResult = await denied;
      expect(
        z.object({ params: z.object({ code: z.string() }) }).parse(await oldResult.json()).params
          .code,
      ).toBe("timer_context_changed");
      releaseTransitionMe();
      await refreshed;
      const result = await successorRead;
      expect(timerShape.parse(await result.json()).run?.id).toBe(run.runId);
      expect(
        z.object({ runId: z.string().nullable() }).parse(await (await successorOwner).json()).runId,
      ).toBe(run.runId);
      await expect(mounted).toBeVisible();
      await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
      await expect(mounted.getByTestId("timer-resume")).toBeEnabled();
      await expect(first.getByTestId("timer-owner")).toBeVisible();
      expect(
        events
          .slice(transitionStart)
          .filter(
            (event) => event.kind === "requestfailed" && event.session === successor.sessionId,
          ),
        "an old denial must not cancel the new session's authorized timer query",
      ).toEqual([]);
    }
  } finally {
    releaseTransitionMe();
    await testInfo.attach("timer-real-session-boundaries", {
      body: JSON.stringify({ events, productQueryOrAuthInjection: false }),
      contentType: "application/json",
    });
    await firstContext.close();
    await secondContext.close();
  }
});

const personalRecordShape = z.object({
  id: z.string(),
  kind: z.enum(["manual", "segment"]),
  startedAt: z.string(),
  endedAt: z.string().nullable(),
  note: z.string().nullable(),
  revision: z.number(),
});
const recordResultShape = z.object({ record: personalRecordShape });

function timerDatabaseEffects(actor: string, task: string): string {
  if (![actor, task].every((id) => /^[0-9a-f-]{36}$/.test(id)))
    throw new Error("invalid fixture UUID");
  return diagnosticSql(`SELECT jsonb_build_object(
    'runs',(SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY r.id),'[]'::jsonb) FROM fvoci.task_timer_runs r WHERE r.user_id='${actor}'),
    'segments',(SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY s.id),'[]'::jsonb) FROM fvoci.task_timer_segments s WHERE s.user_id='${actor}'),
    'receipts',(SELECT COALESCE(jsonb_agg(to_jsonb(c) ORDER BY c.request_id),'[]'::jsonb) FROM fvoci.task_timer_commands c WHERE c.user_id='${actor}'),
    'audit',(SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]'::jsonb) FROM fvoci.task_timer_audit a WHERE a.user_id='${actor}'),
    'legacy',(SELECT COALESCE(jsonb_agg(to_jsonb(l) ORDER BY l.time_entry_id),'[]'::jsonb) FROM fvoci.task_timer_legacy_open l WHERE l.user_id='${actor}'),
    'history',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY e.id),'[]'::jsonb) FROM fvoci.time_entries e WHERE e.user_id='${actor}'),
    'events',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY e.id),'[]'::jsonb) FROM fvoci.events e WHERE e.target_id='${task}'),
    'taskAudit',(SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]'::jsonb) FROM fvoci.audit_log a WHERE a.target_id='${task}'),
    'task',(SELECT jsonb_build_object('id',t.id,'statusId',t.status_id,'startDate',t.start_date,'dueDate',t.due_date,'dueAt',t.due_at,'recurrence',t.recurrence,'estimate',t.estimate,'updatedAt',t.updated_at) FROM fvoci.tasks t WHERE t.id='${task}')
  )`);
}

// Observe the real API client's JSON consumption without changing requests,
// status, body or Vue state. A MessageChannel task runs after the response's
// promise continuations and Vue's microtask flush, including stale-run return.
async function observeOwnerCompletion(page: import("@playwright/test").Page): Promise<void> {
  await page.evaluate(() => {
    const witness = document.createElement("script");
    witness.type = "application/json";
    witness.dataset.testid = "owner-delivery-witness";
    document.body.append(witness);
    const requested: string[] = [];
    const completed: string[] = [];
    const publish = () => {
      witness.textContent = JSON.stringify({ requested, completed });
    };
    publish();
    const nativeFetch = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input, init) => {
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      const url = input instanceof Request ? input.url : String(input);
      if (
        method !== "POST" ||
        new URL(url, location.href).pathname !== "/api/v1/me/task-timer/stop"
      )
        return nativeFetch(input, init);
      let body: unknown;
      if (input instanceof Request) body = await input.clone().json();
      else {
        const requestBody = init?.body;
        if (typeof requestBody !== "string")
          throw new Error("owner request witness requires the actual JSON request");
        body = JSON.parse(requestBody);
      }
      if (
        typeof body !== "object" ||
        body === null ||
        !("runId" in body) ||
        typeof body.runId !== "string"
      )
        throw new Error("owner request witness requires the actual run identity");
      const runId = body.runId;
      requested.push(runId);
      publish();
      const response = await nativeFetch(input, init);
      const nativeJson = response.json.bind(response);
      response.json = async () => {
        const result: unknown = await nativeJson();
        const delivery = new MessageChannel();
        delivery.port1.onmessage = () => {
          completed.push(runId);
          publish();
          delivery.port1.close();
          delivery.port2.close();
        };
        delivery.port2.postMessage(runId);
        return result;
      };
      return response;
    };
  });
}
async function ownerCompletion(
  page: import("@playwright/test").Page,
  runId: string,
): Promise<void> {
  await expect
    .poll(async () => {
      const value = await page.getByTestId("owner-delivery-witness").textContent();
      return z.object({ completed: z.array(z.string()) }).parse(JSON.parse(value ?? "null"))
        .completed;
    })
    .toContain(runId);
}

async function ordinaryTimerTask(
  page: import("@playwright/test").Page,
  key: string,
  independentBootstrap = false,
) {
  if (independentBootstrap) {
    await page.goto("/");
    await expect(
      page
        .getByRole("button", { name: "시작하기" })
        .or(page.getByRole("button", { name: "로그인", exact: true })),
    ).toBeVisible();
    if (await page.getByRole("button", { name: "시작하기" }).count()) {
      await timerWorkspace(page);
    } else {
      // The group intentionally retains the real login limiter. Only new cases
      // use independent bootstrap accounts; their actual task writer stays a
      // normal member, and the existing nine flows keep their exact fixtures.
      const bootstrap = `timer-bootstrap-${key.toLowerCase()}@example.com`;
      createE2eUser(bootstrap, credentials.password, "측정 준비 작성자", {
        workspaceSlug: "w5timer",
        membershipRole: "member",
      });
      // Same setup capability as the original site's bootstrap administrator,
      // on this isolated synthetic preparation identity only. The measured
      // member below never receives this flag or a different database role.
      expect(/^[a-z0-9@.-]+$/.test(bootstrap)).toBe(true);
      const prepared = diagnosticSql(
        `UPDATE fvoci.users SET is_instance_admin=true WHERE email='${bootstrap}' AND NOT is_instance_admin RETURNING id`,
      );
      expect(/^[0-9a-f-]{36}$/.test(prepared)).toBe(true);
      await login(page, bootstrap, credentials.password);
      z.object({ isInstanceAdmin: z.literal(true) }).parse(
        await (await page.request.get("/api/v1/auth/me")).json(),
      );
    }
  } else await timerWorkspace(page);
  const slug = `w5-${key.toLowerCase()}-timer`;
  const createdWorkspace = await page.request.post("/api/v1/workspaces", {
    data: { name: "독립 측정 검증", slug },
  });
  expect(createdWorkspace.status(), await createdWorkspace.text()).toBe(201);
  const workspaceId = z.object({ id: z.string() }).parse(await createdWorkspace.json()).id;
  const email = `timer-${key.toLowerCase()}@example.com`;
  createE2eUser(email, credentials.password, "기록 검증 작성자", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const loggedOut = await page.request.post("/api/v1/auth/logout");
  expect(loggedOut.ok(), await loggedOut.text()).toBe(true);
  await login(page, email, credentials.password);
  const actorResponse: unknown = await (await page.request.get("/api/v1/auth/me")).json();
  const actor = identityShape.parse(actorResponse);
  if (independentBootstrap) z.object({ isInstanceAdmin: z.literal(false) }).parse(actorResponse);
  const createdProject = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key, name: "기록 검증", visibility: "workspace" },
  });
  expect(createdProject.status(), await createdProject.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await createdProject.json());
  const created = await page.request.post(
    `/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`,
    {
      data: { title: `${key} 기록 검증` },
    },
  );
  expect(created.status(), await created.text()).toBe(201);
  const task = taskShape.parse(await created.json());
  const assigned = await page.request.patch(`/api/v1/workspaces/${workspaceId}/tasks/${task.id}`, {
    data: { assigneeIds: [actor.userId] },
  });
  expect(assigned.ok(), await assigned.text()).toBe(true);
  return {
    workspaceId,
    actor,
    task,
    slug,
    email,
    detail: `/w/${slug}/${key}-${String(task.number)}`,
    timerUrl: `/api/v1/workspaces/${workspaceId}/tasks/${task.id}/timer`,
  };
}

test("mounted personal manual correction keeps a conflicting draft and fresh-client day week history", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TMAN");
  const me = z
    .object({ timezone: z.string() })
    .parse(await (await page.request.get("/api/v1/auth/me")).json());
  await page.goto(fixture.detail);
  const panel = page.getByTestId("task-personal-time-records");
  await expect(panel.getByRole("button", { name: "기록 추가", exact: true })).toBeEnabled();
  const sampled = z
    .object({ serverNow: z.string() })
    .parse(await (await page.request.get(fixture.timerUrl)).json());
  const end = new Date(Date.parse(sampled.serverNow) - 60_000).toISOString();
  const start = new Date(Date.parse(end) - 900_000).toISOString();
  await panel.getByRole("button", { name: "기록 추가", exact: true }).click();
  await panel
    .getByLabel("시작", { exact: true })
    .fill(isoToDatetimeLocalInTimeZone(start, me.timezone));
  await panel
    .getByLabel("종료", { exact: true })
    .fill(isoToDatetimeLocalInTimeZone(end, me.timezone));
  await panel.getByLabel("메모", { exact: true }).fill("최초 읽기 기록");
  await panel.getByLabel("기록·수정 사유", { exact: true }).fill("읽은 시간을 직접 입력");
  const createdResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === `${fixture.timerUrl}/history` &&
      response.request().method() === "POST",
  );
  await panel.getByRole("button", { name: "기록 추가", exact: true }).click();
  const created = await createdResponse;
  expect(created.status(), await created.text()).toBe(200);
  const first = recordResultShape.parse(await created.json()).record;
  const row = panel.locator(`[data-record-id="${first.id}"]`);
  await expect(row).toContainText("최초 읽기 기록");
  await expect(panel.getByTestId("task-personal-time-summary")).toContainText("00:15:00.000");
  await row.getByRole("button", { name: "기록 수정", exact: true }).click();
  await panel.getByLabel("메모", { exact: true }).fill("충돌 뒤 보존할 내 초안");
  await panel.getByLabel("기록·수정 사유", { exact: true }).fill("독서 메모 수정");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    const correctionUrl = `${fixture.timerUrl}/records/${first.id}/correct`;
    const other = await fresh.request.post(correctionUrl, {
      data: {
        expectedActorId: identity.userId,
        expectedSessionId: identity.sessionId,
        requestId: crypto.randomUUID(),
        kind: first.kind,
        expectedRevision: first.revision,
        expectedStartedAt: first.startedAt,
        expectedEndedAt: first.endedAt,
        expectedNote: first.note,
        startedAt: first.startedAt,
        endedAt: first.endedAt,
        note: "다른 창의 현재 기록",
        reason: "다른 창 수정",
      },
    });
    expect(other.status(), await other.text()).toBe(200);
    const beforeConflict = timerDatabaseEffects(identity.userId, fixture.task.id);
    const conflictResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === correctionUrl &&
        response.request().method() === "POST",
    );
    await panel.getByRole("button", { name: "기록 수정", exact: true }).last().click();
    const conflict = await conflictResponse;
    expect(conflict.status(), await conflict.text()).toBe(409);
    expect(
      z.object({ params: z.object({ code: z.string() }) }).parse(await conflict.json()).params.code,
    ).toBe("time_record_version");
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeConflict);
    await expect(panel.getByLabel("메모", { exact: true })).toHaveValue("충돌 뒤 보존할 내 초안");
    await expect(row).toContainText("다른 창의 현재 기록");
    await panel
      .getByRole("button", { name: "현재 기록을 확인하고 다시 적용", exact: true })
      .click();
    const correctedResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === correctionUrl &&
        response.request().method() === "POST",
    );
    await panel.getByRole("button", { name: "기록 수정", exact: true }).last().click();
    const corrected = await correctedResponse;
    expect(corrected.status(), await corrected.text()).toBe(200);
    const final = recordResultShape.parse(await corrected.json()).record;
    expect(final.revision).toBe(2);
    expect(final.startedAt).toBe(first.startedAt);
    expect(final.endedAt).toBe(first.endedAt);
    await expect(row).toContainText("충돌 뒤 보존할 내 초안");
    await expect(panel.getByTestId("task-personal-time-summary")).toContainText("00:15:00.000");
    expect(diagnosticSql(`SELECT note FROM fvoci.time_entries WHERE id='${first.id}'`)).toBe(
      "최초 읽기 기록",
    );
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE time_entry_id='${first.id}' AND reason='독서 메모 수정'`,
      ),
    ).toBe("1");
    await fresh.goto(fixture.detail);
    await expect(
      fresh.getByTestId("task-personal-time-records").locator(`[data-record-id="${first.id}"]`),
    ).toContainText("충돌 뒤 보존할 내 초안");
    await expect(fresh.getByTestId("task-personal-time-summary")).toContainText("00:15:00.000");
    expect(
      taskShape.parse(
        await (
          await fresh.request.get(
            `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`,
          )
        ).json(),
      ).statusId,
    ).toBe(fixture.task.statusId);
    const evidence = process.env.FVOCI_W5_EVIDENCE_DIR;
    if (!evidence) throw new Error("missing owned evidence namespace");
    for (const zoom of [100, 200]) {
      await page.setViewportSize({ width: 320, height: 900 });
      await page.evaluate((percent) => {
        document.documentElement.style.fontSize = `${String(percent)}%`;
      }, zoom);
      await panel.scrollIntoViewIfNeeded();
      await expect
        .poll(() => panel.evaluate((element) => element.scrollWidth <= element.clientWidth))
        .toBe(true);
      await page.screenshot({
        path: path.join(evidence, `manual-personal-320-text-${String(zoom)}.png`),
        fullPage: true,
      });
    }
    await page.evaluate(() => {
      document.documentElement.style.fontSize = "";
    });
    await page.setViewportSize({ width: 1280, height: 900 });
    await row.getByRole("button", { name: "기록 수정", exact: true }).click();
    for (const zoom of [100, 200]) {
      await page.setViewportSize({ width: 320, height: 900 });
      await page.evaluate((percent) => {
        document.documentElement.style.fontSize = `${String(percent)}%`;
      }, zoom);
      await panel.scrollIntoViewIfNeeded();
      await expect
        .poll(() => panel.evaluate((element) => element.scrollWidth <= element.clientWidth))
        .toBe(true);
      await page.screenshot({
        path: path.join(evidence, `manual-personal-open-320-text-${String(zoom)}.png`),
        fullPage: true,
      });
    }
    await page.evaluate(() => {
      document.documentElement.style.fontSize = "";
    });
    await page.setViewportSize({ width: 1280, height: 900 });
    // Native datetime-local controls contain multiple keyboard segments. Move
    // with real Tab events, accepting only that input or the intended next
    // field; unrelated focus and a trap retain the literal failing oracle.
    const nativeTabs = async (currentLabel: string, nextLabel: string) => {
      const currentId = await panel.getByLabel(currentLabel, { exact: true }).getAttribute("id");
      const nextId = await panel.getByLabel(nextLabel, { exact: true }).getAttribute("id");
      if (!currentId || !nextId || currentId === nextId)
        throw new Error("native keyboard fields require distinct associated IDs");
      const focusPath: { step: number; activeId: string | null }[] = [];
      try {
        const initial = await page.evaluate(() => document.activeElement?.id ?? null);
        focusPath.push({ step: 0, activeId: initial });
        expect(initial).toBe(currentId);
        for (let step = 1; step <= 12; step++) {
          await page.keyboard.press("Tab");
          const activeId = await page.evaluate(() => document.activeElement?.id ?? null);
          focusPath.push({ step, activeId });
          expect([currentId, nextId]).toContain(activeId);
          if (activeId === nextId) break;
        }
      } finally {
        await testInfo.attach(`manual-native-keyboard-${currentId}`, {
          body: JSON.stringify({ currentLabel, nextLabel, currentId, nextId, focusPath }),
          contentType: "application/json",
        });
      }
    };
    await panel.getByLabel("시작", { exact: true }).focus();
    await nativeTabs("시작", "종료");
    await expect(panel.getByLabel("종료", { exact: true })).toBeFocused();
    await nativeTabs("종료", "메모");
    await expect(panel.getByLabel("메모", { exact: true })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(panel.getByLabel("기록·수정 사유", { exact: true })).toBeFocused();
    await page.screenshot({
      path: path.join(evidence, "manual-personal-keyboard-focus.png"),
      fullPage: true,
    });
    await testInfo.attach("personal-manual-correction-contract", {
      body: JSON.stringify({
        record: first.id,
        revision: final.revision,
        rawRangePreserved: true,
        conflictFullSnapshotUnchanged: true,
        raw034NoteUnchanged: true,
        correctionReasonPersisted: true,
        freshSessionRead: identity.sessionId !== fixture.actor.sessionId,
      }),
      contentType: "application/json",
    });
  } finally {
    await context.close();
  }
});

test("a committed withheld owner stop cannot keep a successor run's current control pending", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TABA");
  const run1 = await startPausedTimer(page, fixture.timerUrl, fixture.actor, "첫 번째 측정");
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  const owner = page.getByTestId("timer-owner");
  await expect(owner.getByRole("button", { name: "현재 측정 종료", exact: true })).toBeEnabled();
  await observeOwnerCompletion(page);
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let release = () => {};
  const delivery = new Promise<void>((resolve) => {
    release = resolve;
  });
  let committed = () => {};
  const committedSignal = new Promise<void>((resolve) => {
    committed = resolve;
  });
  let body: unknown;
  let outcome: unknown;
  await page.route(
    (url) => url.pathname === "/api/v1/me/task-timer/stop",
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      body = route.request().postDataJSON();
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      outcome = await response.json();
      committed();
      await delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  try {
    await owner.getByRole("button", { name: "현재 측정 종료", exact: true }).click();
    await committedSignal;
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    expect(
      timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run,
    ).toBeNull();
    const beforeReplay = timerDatabaseEffects(identity.userId, fixture.task.id);
    const requestBody = z
      .object({
        expectedActorId: z.string(),
        expectedSessionId: z.string(),
        requestId: z.string(),
        runId: z.string(),
        expectedVersion: z.number(),
      })
      .parse(body);
    expect(requestBody.runId).toBe(run1.runId);
    const replayed = await fresh.request.post("/api/v1/me/task-timer/stop", { data: requestBody });
    expect(replayed.status(), await replayed.text()).toBe(200);
    expect(await replayed.json()).toEqual(outcome);
    const changed = await fresh.request.post("/api/v1/me/task-timer/stop", {
      data: { ...requestBody, expectedVersion: requestBody.expectedVersion + 1 },
    });
    expect(changed.status(), await changed.text()).toBe(409);
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeReplay);
    const run2 = await startPausedTimer(fresh, fixture.timerUrl, identity, "後続 측정");
    const successor = page.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer" &&
        response.status() === 200 &&
        z.object({ runId: z.string().nullable() }).parse(await response.json()).runId ===
          run2.runId,
    );
    await page.bringToFront();
    await successor;
    const beforeDelivery = timerDatabaseEffects(identity.userId, fixture.task.id);
    await testInfo.attach("committed-response-withheld-run-ABA", {
      body: JSON.stringify({
        run1: run1.runId,
        run2: run2.runId,
        actualNativeCommit200: true,
        browserResponseStillWithheld: true,
        genuineFreshSessionReplaySameOutcome: true,
        changedPayload409: true,
        replayFullSnapshotUnchanged: true,
      }),
      contentType: "application/json",
    });
    // Original product negative: an R1 pending completion cannot disable the
    // actual canonical R2 control. Keep this literal oracle through the fix.
    await expect(owner.getByRole("button", { name: "현재 측정 종료", exact: true })).toBeEnabled();
    release();
    await ownerCompletion(page, run1.runId);
    await expect(owner.getByRole("button", { name: "현재 측정 종료", exact: true })).toBeEnabled();
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeDelivery);
    expect(timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run?.id).toBe(
      run2.runId,
    );
  } finally {
    release();
    await context.close();
  }
});

test("an ordinary mounted time-entry GET cannot render another actor's private correction under stale identity", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TPRIV");
  const email = "timer-private-overlay@example.com";
  createE2eUser(email, credentials.password, "다른 작성자", {
    workspaceSlug: fixture.slug,
    membershipRole: "member",
  });
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let releaseMe = () => {};
  const meDelivery = new Promise<void>((resolve) => {
    releaseMe = resolve;
  });
  try {
    const other = await context.newPage();
    await login(other, email, credentials.password);
    const identity = identityShape.parse(await (await other.request.get("/api/v1/auth/me")).json());
    const created = await other.request.post(`${fixture.timerUrl}/history`, {
      data: {
        expectedActorId: identity.userId,
        expectedSessionId: identity.sessionId,
        requestId: crypto.randomUUID(),
        startedAt: "2026-09-30T00:00:00Z",
        endedAt: "2026-09-30T00:15:00Z",
        note: "공유된 원래 기록",
        reason: "수동 기록",
      },
    });
    expect(created.status(), await created.text()).toBe(200);
    const row = recordResultShape.parse(await created.json()).record;
    const corrected = await other.request.post(`${fixture.timerUrl}/records/${row.id}/correct`, {
      data: {
        expectedActorId: identity.userId,
        expectedSessionId: identity.sessionId,
        requestId: crypto.randomUUID(),
        kind: row.kind,
        expectedRevision: row.revision,
        expectedStartedAt: row.startedAt,
        expectedEndedAt: row.endedAt,
        expectedNote: row.note,
        startedAt: row.startedAt,
        endedAt: row.endedAt,
        note: "다른 작성자만 볼 사적인 수정",
        reason: "개인 수정",
      },
    });
    expect(corrected.status(), await corrected.text()).toBe(200);
    await page.goto(`/w/${fixture.slug}/my-tasks`);
    await expect(page.getByTestId(`my-task-${fixture.task.id}`)).toBeVisible();
    const before = timerDatabaseEffects(identity.userId, fixture.task.id);
    await page.route(
      (url) => url.pathname === "/api/v1/auth/me",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const response = await route.fetch();
        await meDelivery;
        if (!page.isClosed()) await route.fulfill({ response });
      },
    );
    // Hold only captured timer transports with a genuine network-unavailable
    // status. The ordinary GET below is actual Rust/DB, never a fake private DTO.
    await page.route(
      (url) =>
        url.pathname === "/api/v1/me/task-timer" || url.pathname.startsWith(fixture.timerUrl),
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const capture = new URL(route.request().url()).searchParams;
        expect(capture.get("expectedActorId")).toBe(fixture.actor.userId);
        expect(capture.get("expectedSessionId")).toBe(fixture.actor.sessionId);
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", status: 503, title: "측정 연결 실패" }),
        });
      },
    );
    await page.context().addCookies(await context.cookies());
    const entriesPath = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}/time-entries`;
    const ordinary = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === entriesPath && response.request().method() === "GET",
    );
    await page.getByTestId(`my-task-${fixture.task.id}`).click();
    const response = await ordinary;
    const parsed = z
      .object({ items: z.array(z.object({ id: z.string(), note: z.string().nullable() })) })
      .safeParse(await response.json());
    if (
      response.status() === 200 &&
      parsed.success &&
      parsed.data.items.some((item) => item.id === row.id)
    )
      await expect(page.locator(`[data-time-entry-id="${row.id}"]`)).toBeVisible();
    await testInfo.attach("ordinary-real-private-overlay-stale-capture", {
      body: JSON.stringify({
        status: response.status(),
        staleActor: fixture.actor.userId,
        authenticatedActor: identity.userId,
        expectedActorParameter: new URL(response.url()).searchParams.get("expectedActorId"),
        expectedSessionParameter: new URL(response.url()).searchParams.get("expectedSessionId"),
        returnedOtherPrivateCorrection:
          parsed.success &&
          parsed.data.items.some((item) => item.note === "다른 작성자만 볼 사적인 수정"),
        privateResponseMocked: false,
        identityDeliveryWithheld: true,
      }),
      contentType: "application/json",
    });
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(before);
    // Original privacy oracle, unchanged after any granted captured GET fix.
    await expect(page.getByTestId("task-time-entries")).not.toContainText(
      "다른 작성자만 볼 사적인 수정",
    );
  } finally {
    releaseMe();
    await context.close();
  }
});

test("a late R1 response cannot clear the genuine pending stop of canonical R2", async ({
  page,
  browser,
}) => {
  const fixture = await ordinaryTimerTask(page, "TPEND");
  const run1 = await startPausedTimer(page, fixture.timerUrl, fixture.actor, "First pending owner");
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  const owner = page.getByTestId("timer-owner");
  const button = owner.getByRole("button", { name: "현재 측정 종료", exact: true });
  await expect(button).toBeEnabled();
  await observeOwnerCompletion(page);
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  const held = [run1.runId, ""].map(() => {
    let release = () => {};
    let committed = () => {};
    const delivery = new Promise<void>((resolve) => {
      release = resolve;
    });
    const commit = new Promise<void>((resolve) => {
      committed = resolve;
    });
    const operation: {
      delivery: Promise<void>;
      commit: Promise<void>;
      release: () => void;
      committed: () => void;
      body: unknown;
    } = { delivery, commit, release, committed, body: undefined };
    return operation;
  });
  const [first, second] = held;
  if (!first || !second) throw new Error("missing held response fixture");
  let index = 0;
  let releaseOwner = () => {};
  const ownerDelivery = new Promise<void>((resolve) => {
    releaseOwner = resolve;
  });
  await page.route(
    (url) => url.pathname === "/api/v1/me/task-timer/stop",
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const operation = held[index++];
      if (!operation) throw new Error("unexpected third owner command");
      operation.body = route.request().postDataJSON();
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      operation.committed();
      await operation.delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  try {
    await button.click();
    await first.commit;
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    const run2 = await startPausedTimer(fresh, fixture.timerUrl, identity, "Second pending owner");
    const successor = page.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer" &&
        response.status() === 200 &&
        z.object({ runId: z.string().nullable() }).parse(await response.json()).runId ===
          run2.runId,
    );
    await page.bringToFront();
    await successor;
    await expect(button).toBeEnabled();
    // Keep the actual canonical R2 read visible until its own completion; do
    // not let a later owner-null poll remove the control before this oracle.
    await page.route(
      (url) => url.pathname === "/api/v1/me/task-timer",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const response = await route.fetch();
        await ownerDelivery;
        if (!page.isClosed()) await route.fulfill({ response });
      },
    );
    await button.click();
    await second.commit;
    expect(z.object({ runId: z.string() }).parse(second.body).runId).toBe(run2.runId);
    const beforeDelivery = timerDatabaseEffects(identity.userId, fixture.task.id);
    await expect(button).toBeDisabled();
    const firstResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer/stop" &&
        z.object({ runId: z.string() }).parse(response.request().postDataJSON()).runId ===
          run1.runId,
    );
    first.release();
    await firstResponse;
    await ownerCompletion(page, run1.runId);
    await expect(button).toBeDisabled();
    await button.evaluate((element) => {
      if (!(element instanceof HTMLButtonElement))
        throw new Error("owner control is not a native button");
      element.click();
    });
    const attempts = z
      .object({ requested: z.array(z.string()) })
      .parse(
        JSON.parse((await page.getByTestId("owner-delivery-witness").textContent()) ?? "null"),
      );
    expect(attempts.requested).toEqual([run1.runId, run2.runId]);
    expect(index).toBe(2);
    await expect(button).toBeDisabled();
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeDelivery);
    second.release();
    releaseOwner();
    await expect(owner).toHaveCount(0);
    expect(
      timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run,
    ).toBeNull();
  } finally {
    for (const operation of held) operation.release();
    releaseOwner();
    await context.close();
  }
});

test("ordinary research plan persists document origins explicit minutes and fresh-client stopwatch", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TPLAN", true);
  const documents = [];
  for (const title of ["연구 목표·노트", "읽을 자료"]) {
    const response = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/documents`,
      {
        data: { parentId: null, title },
      },
    );
    expect(response.status(), await response.text()).toBe(201);
    documents.push(z.object({ id: z.string(), number: z.number() }).parse(await response.json()));
  }
  const [notes, material] = documents;
  if (!notes || !material) throw new Error("missing ordinary plan source documents");
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  const planner = page.getByTestId("study-plan-builder");
  await planner.getByText("학습·연구·업무 계획 만들기", { exact: true }).click();
  await planner.getByLabel("계획 틀", { exact: true }).selectOption("research");
  await planner.getByRole("button", { name: "틀 적용", exact: true }).click();
  await planner.getByLabel("목표", { exact: true }).fill("자료를 읽고 가설을 검토하기");
  await planner.getByLabel("목표 예상 시간(분, 선택)", { exact: true }).fill("0");
  await planner
    .getByLabel("목표·연구 노트 문서 링크", { exact: true })
    .fill(`WIKI-${String(notes.number)}`);
  await planner
    .getByLabel("읽을 자료 문서 링크", { exact: true })
    .fill(`WIKI-${String(material.number)}`);
  await planner.getByRole("button", { name: "연결할 문서 확인", exact: true }).click();
  const destination = planner.getByLabel("저장할 프로젝트", { exact: true });
  await expect(destination.locator("option", { hasText: "TPLAN" })).toHaveCount(1);
  const project = z
    .object({ projectId: z.string() })
    .parse(
      await (
        await page.request.get(`/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`)
      ).json(),
    );
  await destination.selectOption(project.projectId);
  await planner.getByLabel("단계 이름", { exact: true }).nth(0).fill("자료 읽기");
  await planner.getByLabel("단계 예상 시간(분, 선택)", { exact: true }).nth(0).fill("30");
  await planner.getByLabel("단계 예상 시간(분, 선택)", { exact: true }).nth(1).fill("20");
  await planner.getByLabel("단계 예상 시간(분, 선택)", { exact: true }).nth(2).fill("10");
  const accepted: Array<{ taskId: string; number: number; projectKey: string; document: string }> =
    [];
  page.on("response", async (response) => {
    const pathname = new URL(response.url()).pathname;
    if (
      response.request().method() !== "POST" ||
      !pathname.endsWith("/study-plan/task") ||
      response.status() !== 200
    )
      return;
    const result = z
      .object({ taskId: z.string(), number: z.number(), projectKey: z.string() })
      .parse(await response.json());
    accepted.push({ ...result, document: pathname.split("/").at(-3) ?? "" });
  });
  await planner.getByRole("button", { name: "계획 저장", exact: true }).click();
  await expect(planner.getByRole("status").filter({ hasText: "저장된 목표와 단계" })).toContainText(
    "4개 저장됨",
  );
  await expect.poll(() => accepted.length).toBe(4);
  const [goal, reading] = accepted;
  if (!goal || !reading) throw new Error("missing real ordinary plan receipts");
  expect(goal.document).toBe(notes.id);
  expect(accepted.slice(1).map((row) => row.document)).toEqual([
    material.id,
    material.id,
    material.id,
  ]);
  for (const row of accepted) {
    expect(/^[0-9a-f-]{36}$/.test(row.taskId)).toBe(true);
    const raw = z
      .object({ id: z.string(), type: z.string(), parentId: z.string().nullable() })
      .parse(
        await (
          await page.request.get(`/api/v1/workspaces/${fixture.workspaceId}/tasks/${row.taskId}`)
        ).json(),
      );
    expect(raw.id).toBe(row.taskId);
    expect(raw.type).toBe(row === goal ? "epic" : "task");
    expect(raw.parentId).toBe(row === goal ? null : goal.taskId);
  }
  const rawOrigins = diagnosticSql(
    `SELECT jsonb_agg(jsonb_build_object('task',t.id,'document',o.document_id,'estimate',t.estimate,'unit',t.estimate_unit) ORDER BY t.number) FROM fvoci.tasks t JOIN fvoci.task_origins o ON o.workspace_id=t.workspace_id AND o.task_id=t.id WHERE t.workspace_id='${fixture.workspaceId}'`,
  );
  const origins = z
    .array(
      z.object({ task: z.string(), document: z.string(), estimate: z.number(), unit: z.string() }),
    )
    .parse(JSON.parse(rawOrigins));
  expect(origins.map((row) => row.task)).toEqual(accepted.map((row) => row.taskId));
  expect(origins.map((row) => row.estimate)).toEqual([0, 30, 20, 10]);
  expect(origins.every((row) => row.unit === "minutes")).toBe(true);
  const widget = planner.getByTestId(`task-stopwatch-${reading.taskId}`);
  await expect(widget.getByTestId("timer-estimate")).toHaveText("예상 30분");
  await widget.getByTestId("timer-start").click();
  await expect(widget.getByTestId("timer-state")).toHaveText("측정 중");
  const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${reading.taskId}/timer`;
  const committed = timerShape.parse(await (await page.request.get(timerUrl)).json());
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    await fresh.goto(`/w/${fixture.slug}/${reading.projectKey}-${String(reading.number)}`);
    const current = fresh.getByTestId(`task-stopwatch-${reading.taskId}`);
    await expect(current.getByTestId("timer-state")).toHaveText("측정 중");
    expect(timerShape.parse(await (await fresh.request.get(timerUrl)).json()).run?.id).toBe(
      committed.run?.id,
    );
    const beforeTask = taskShape.parse(
      await (
        await fresh.request.get(`/api/v1/workspaces/${fixture.workspaceId}/tasks/${reading.taskId}`)
      ).json(),
    );
    await current.getByTestId("timer-pause").click();
    await expect(current.getByTestId("timer-state")).toHaveText("일시정지");
    const paused = timerShape.parse(await (await fresh.request.get(timerUrl)).json());
    await fresh.reload();
    await expect(current.getByTestId("timer-state")).toHaveText("일시정지");
    expect(
      timerShape.parse(await (await fresh.request.get(timerUrl)).json()).run?.elapsedMilliseconds,
    ).toBe(paused.run?.elapsedMilliseconds);
    await current.getByTestId("timer-resume").click();
    await expect(current.getByTestId("timer-state")).toHaveText("측정 중");
    await current.getByTestId("timer-stop").click();
    await expect(current.getByTestId("timer-start")).toBeEnabled();
    const stopped = timerShape.parse(await (await fresh.request.get(timerUrl)).json());
    expect(stopped.run).toBeNull();
    expect(stopped.actualMilliseconds).toBeGreaterThan(0);
    expect(
      taskShape.parse(
        await (
          await fresh.request.get(
            `/api/v1/workspaces/${fixture.workspaceId}/tasks/${reading.taskId}`,
          )
        ).json(),
      ).statusId,
    ).toBe(beforeTask.statusId);
    const editor = current.getByTestId("task-estimate-editor");
    await editor.getByText("예상 시간 설정", { exact: true }).click();
    await editor.getByLabel("예상 시간(분)", { exact: true }).fill("45");
    await editor.getByLabel("예상 시간 변경 사유", { exact: true }).fill("자료 분량을 다시 확인함");
    await editor.getByRole("button", { name: "예상 시간 저장", exact: true }).click();
    await expect(current.getByTestId("timer-estimate")).toHaveText("예상 45분");
    expect(
      diagnosticSql(
        `SELECT reason FROM fvoci.task_timer_audit WHERE user_id='${fixture.actor.userId}' AND task_id='${reading.taskId}' AND verb='task.estimate.minutes' ORDER BY created_at DESC LIMIT 1`,
      ),
    ).toBe("자료 분량을 다시 확인함");
    await fresh.reload();
    await expect(current.getByTestId("timer-estimate")).toHaveText("예상 45분");
    await testInfo.attach("ordinary-plan-native-model", {
      body: JSON.stringify({
        accepted,
        origins,
        runId: committed.run?.id,
        pausedElapsed: paused.run?.elapsedMilliseconds,
        stoppedActual: stopped.actualMilliseconds,
        statusUnchanged: true,
      }),
      contentType: "application/json",
    });
  } finally {
    await context.close();
  }
  for (const percent of [100, 200]) {
    await page.setViewportSize({ width: 320, height: 900 });
    await page.evaluate((value) => {
      document.documentElement.style.fontSize = `${String(value)}%`;
    }, percent);
    await expect
      .poll(() => planner.evaluate((element) => element.scrollWidth <= element.clientWidth + 1))
      .toBe(true);
    await planner.screenshot({
      path: testInfo.outputPath(`planner-320-text-${String(percent)}.png`),
    });
  }
});

// A real response.json completion witness: it observes delivery after native
// fetch/Vue continuations without injecting a DTO, query cache or command.
async function observeTaskTimerRead(page: import("@playwright/test").Page, pathname: string) {
  await page.evaluate((pathname) => {
    const witness = document.createElement("script");
    witness.type = "application/json";
    witness.dataset.testid = "task-timer-read-witness";
    document.body.append(witness);
    const completed: string[] = [];
    witness.textContent = JSON.stringify(completed);
    const nativeFetch = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input, init) => {
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      const url = input instanceof Request ? input.url : String(input);
      const response = await nativeFetch(input, init);
      if (
        method !== "GET" ||
        new URL(url, location.href).pathname !== pathname ||
        response.status !== 200
      )
        return response;
      const nativeJson = response.json.bind(response);
      response.json = async () => {
        const result: unknown = await nativeJson();
        if (
          typeof result === "object" &&
          result !== null &&
          "run" in result &&
          typeof result.run === "object" &&
          result.run !== null &&
          "id" in result.run &&
          typeof result.run.id === "string"
        ) {
          const runId = result.run.id;
          const delivery = new MessageChannel();
          delivery.port1.onmessage = () => {
            completed.push(runId);
            witness.textContent = JSON.stringify(completed);
            delivery.port1.close();
            delivery.port2.close();
          };
          delivery.port2.postMessage(runId);
        }
        return result;
      };
      return response;
    };
  }, pathname);
}

test("task widget retires a held R1 command when a genuine current R2 read arrives", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TMAIN", true);
  const run1 = await startPausedTimer(
    page,
    fixture.timerUrl,
    fixture.actor,
    "First task widget command",
  );
  await page.goto(fixture.detail);
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  const stop = widget.getByTestId("timer-stop");
  await expect(stop).toBeEnabled();
  await observeTaskTimerRead(page, fixture.timerUrl);
  let release = () => {};
  let committed = () => {};
  const delivery = new Promise<void>((resolve) => {
    release = resolve;
  });
  const commit = new Promise<void>((resolve) => {
    committed = resolve;
  });
  await page.route(
    (url) => url.pathname === fixture.timerUrl,
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const body = z
        .object({ runId: z.string(), operation: z.string() })
        .parse(route.request().postDataJSON());
      expect(body.runId).toBe(run1.runId);
      expect(body.operation).toBe("stop");
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      committed();
      await delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    await stop.click();
    await commit;
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    const run2 = await startPausedTimer(
      fresh,
      fixture.timerUrl,
      identity,
      "Second task widget command",
    );
    expect(run2.runId).not.toBe(run1.runId);
    const successor = page.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === fixture.timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 200 &&
        timerShape.parse(await response.json()).run?.id === run2.runId,
    );
    await page.bringToFront();
    await successor;
    await expect
      .poll(async () =>
        z
          .array(z.string())
          .parse(
            JSON.parse((await page.getByTestId("task-timer-read-witness").textContent()) ?? "null"),
          ),
      )
      .toContain(run2.runId);
    const beforeDelivery = timerDatabaseEffects(identity.userId, fixture.task.id);
    await testInfo.attach("main-widget-genuine-successor-before-late-delivery", {
      body: JSON.stringify({
        run1,
        run2,
        realReadConsumed: true,
        r1DeliveryHeld: true,
        databaseEffects: JSON.parse(beforeDelivery) as unknown,
      }),
      contentType: "application/json",
    });
    // Original negative oracle: canonical R2 must be usable before R1 arrives.
    await expect(widget.getByTestId("timer-resume")).toBeEnabled();
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeDelivery);
    release();
    await expect(widget.getByTestId("timer-resume")).toBeEnabled();
  } finally {
    release();
    await context.close();
  }
});

async function observeTaskTimerCompletion(
  page: import("@playwright/test").Page,
  pathname: string,
): Promise<void> {
  await page.evaluate((pathname) => {
    const witness = document.createElement("script");
    witness.type = "application/json";
    witness.dataset.testid = "task-timer-command-witness";
    document.body.append(witness);
    const requested: string[] = [];
    const completed: string[] = [];
    const publish = () => {
      witness.textContent = JSON.stringify({ requested, completed });
    };
    publish();
    const nativeFetch = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input, init) => {
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      const url = input instanceof Request ? input.url : String(input);
      if (method !== "POST" || new URL(url, location.href).pathname !== pathname)
        return nativeFetch(input, init);
      let body: unknown;
      if (input instanceof Request) body = await input.clone().json();
      else {
        const requestBody = init?.body;
        if (typeof requestBody !== "string")
          throw new Error("task timer request witness requires the actual JSON request");
        body = JSON.parse(requestBody);
      }
      if (
        typeof body !== "object" ||
        body === null ||
        !("runId" in body) ||
        typeof body.runId !== "string"
      )
        throw new Error("task timer request witness requires the actual run identity");
      const runId = body.runId;
      requested.push(runId);
      publish();
      const response = await nativeFetch(input, init);
      const nativeJson = response.json.bind(response);
      response.json = async () => {
        const result: unknown = await nativeJson();
        const delivery = new MessageChannel();
        delivery.port1.onmessage = () => {
          completed.push(runId);
          publish();
          delivery.port1.close();
          delivery.port2.close();
        };
        delivery.port2.postMessage(runId);
        return result;
      };
      return response;
    };
  }, pathname);
}

test("a late task-widget R1 response cannot clear a genuine pending R2 resume", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TPENDMAIN", true);
  const run1 = await startPausedTimer(page, fixture.timerUrl, fixture.actor, "First held widget");
  await page.goto(fixture.detail);
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  await expect(widget.getByTestId("timer-stop")).toBeEnabled();
  await observeTaskTimerCompletion(page, fixture.timerUrl);
  await observeTaskTimerRead(page, fixture.timerUrl);
  const held = [0, 1].map(() => {
    let release = () => {};
    let committed = () => {};
    const delivery = new Promise<void>((resolve) => {
      release = resolve;
    });
    const commit = new Promise<void>((resolve) => {
      committed = resolve;
    });
    const operation: {
      delivery: Promise<void>;
      commit: Promise<void>;
      release: () => void;
      committed: () => void;
      body: unknown;
    } = { delivery, commit, release, committed, body: undefined };
    return operation;
  });
  const [first, second] = held;
  if (!first || !second) throw new Error("missing widget held responses");
  let index = 0;
  let releaseRead = () => {};
  const readDelivery = new Promise<void>((resolve) => {
    releaseRead = resolve;
  });
  await page.route(
    (url) => url.pathname === fixture.timerUrl,
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const operation = held[index++];
      if (!operation) throw new Error("unexpected third widget command");
      operation.body = route.request().postDataJSON();
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      operation.committed();
      await operation.delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    await widget.getByTestId("timer-stop").click();
    await first.commit;
    expect(
      z.object({ runId: z.string(), operation: z.literal("stop") }).parse(first.body).runId,
    ).toBe(run1.runId);
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    const run2 = await startPausedTimer(fresh, fixture.timerUrl, identity, "Second held widget");
    expect(run2.runId).not.toBe(run1.runId);
    await page.bringToFront();
    await expect
      .poll(async () =>
        z
          .array(z.string())
          .parse(
            JSON.parse((await page.getByTestId("task-timer-read-witness").textContent()) ?? "null"),
          ),
      )
      .toContain(run2.runId);
    const resume = widget.getByTestId("timer-resume");
    await expect(resume).toBeEnabled();
    // Keep the already-consumed real R2 snapshot until its pending response;
    // later real GETs are held, never replaced with a fabricated DTO.
    await page.route(
      (url) => url.pathname === fixture.timerUrl,
      async (route) => {
        if (route.request().method() !== "GET") return route.fallback();
        const response = await route.fetch();
        await readDelivery;
        if (!page.isClosed()) await route.fulfill({ response });
      },
    );
    await resume.click();
    await second.commit;
    expect(
      z.object({ runId: z.string(), operation: z.literal("resume") }).parse(second.body).runId,
    ).toBe(run2.runId);
    const beforeDelivery = timerDatabaseEffects(identity.userId, fixture.task.id);
    await expect(resume).toBeDisabled();
    first.release();
    await expect
      .poll(
        async () =>
          z
            .object({ completed: z.array(z.string()) })
            .parse(
              JSON.parse(
                (await page.getByTestId("task-timer-command-witness").textContent()) ?? "null",
              ),
            ).completed,
      )
      .toContain(run1.runId);
    await expect(resume).toBeDisabled();
    await resume.evaluate((element) => {
      if (!(element instanceof HTMLButtonElement)) throw new Error("resume is not a native button");
      element.click();
    });
    expect(
      z
        .object({ requested: z.array(z.string()) })
        .parse(
          JSON.parse(
            (await page.getByTestId("task-timer-command-witness").textContent()) ?? "null",
          ),
        ).requested,
    ).toEqual([run1.runId, run2.runId]);
    expect(index).toBe(2);
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeDelivery);
    await testInfo.attach("main-widget-old-completion-current-pending", {
      body: JSON.stringify({
        run1,
        run2,
        originalConsumed: true,
        currentPending: true,
        requests: 2,
        databaseEffects: JSON.parse(beforeDelivery) as unknown,
      }),
      contentType: "application/json",
    });
    second.release();
    releaseRead();
    await expect(widget.getByTestId("timer-state")).toHaveText("측정 중");
    await expect(widget.getByTestId("timer-pause")).toBeEnabled();
    expect(timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run?.id).toBe(
      run2.runId,
    );
  } finally {
    for (const operation of held) operation.release();
    releaseRead();
    await context.close();
  }
});

test("owner releases opaque legacy reservations after task permission loss without rewriting history", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await permissionFixture(page, "TLEGACY", "member");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const person = await context.newPage();
    await login(person, fixture.email, credentials.password);
    let identity = identityShape.parse(await (await person.request.get("/api/v1/auth/me")).json());
    expect(
      z
        .object({ isInstanceAdmin: z.literal(false) })
        .parse(await (await person.request.get("/api/v1/auth/me")).json()).isInstanceAdmin,
    ).toBe(false);
    const membersUrl = `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`;
    const granted = await page.request.post(membersUrl, {
      data: { userId: identity.userId, role: "member" },
    });
    expect(granted.ok(), await granted.text()).toBe(true);
    const created = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      { data: { title: "접근 회수된 기존 기록의 숨겨진 제목" } },
    );
    expect(created.status(), await created.text()).toBe(201);
    const task = taskShape.parse(await created.json());
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    const entryResponse = await person.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/time-entries`,
      {
        data: {
          startedAt: "2026-09-29T01:02:03.123456Z",
          note: "기존 비공개 미종료 메모",
        },
      },
    );
    expect(entryResponse.status(), await entryResponse.text()).toBe(201);
    const entry = z.object({ id: z.string() }).parse(await entryResponse.json());
    const originalRaw = diagnosticSql(
      `SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE e.id='${entry.id}'`,
    );
    const nextSlug = `legacy-next-${crypto.randomUUID().slice(0, 8)}`;
    const nextWorkspaceResponse = await page.request.post("/api/v1/workspaces", {
      data: { name: "다음 작업 공간", slug: nextSlug },
    });
    expect(nextWorkspaceResponse.status(), await nextWorkspaceResponse.text()).toBe(201);
    const nextWorkspace = z.object({ id: z.string() }).parse(await nextWorkspaceResponse.json());
    const invitation = await page.request.post(
      `/api/v1/workspaces/${nextWorkspace.id}/invitations`,
      { data: { email: fixture.email, role: "member" } },
    );
    expect(invitation.status(), await invitation.text()).toBe(201);
    const acceptUrl = z.object({ acceptUrl: z.string() }).parse(await invitation.json()).acceptUrl;
    const token = new URL(acceptUrl).pathname.split("/invite/")[1];
    if (!token) throw new Error("existing member invitation token missing");
    const accepted = await person.request.post(`/api/v1/invitations/${token}/accept`, {
      data: { password: credentials.password },
    });
    expect(accepted.status(), await accepted.text()).toBe(200);
    const acceptedIdentity = identityShape.parse(
      await (await person.request.get("/api/v1/auth/me")).json(),
    );
    expect(acceptedIdentity.userId).toBe(identity.userId);
    identity = acceptedIdentity;
    const nextProjectResponse = await page.request.post(
      `/api/v1/workspaces/${nextWorkspace.id}/projects`,
      {
        data: { key: "NEXT", name: "다음 정상 작업", visibility: "workspace" },
      },
    );
    expect(nextProjectResponse.status(), await nextProjectResponse.text()).toBe(201);
    const nextProject = z.object({ id: z.string() }).parse(await nextProjectResponse.json());
    const nextTaskResponse = await page.request.post(
      `/api/v1/workspaces/${nextWorkspace.id}/projects/${nextProject.id}/tasks`,
      { data: { title: "제한 해제 뒤 새 측정" } },
    );
    expect(nextTaskResponse.status(), await nextTaskResponse.text()).toBe(201);
    const nextTask = taskShape.parse(await nextTaskResponse.json());
    const assigned = await page.request.patch(
      `/api/v1/workspaces/${nextWorkspace.id}/tasks/${nextTask.id}`,
      { data: { assigneeIds: [identity.userId] } },
    );
    expect(assigned.ok(), await assigned.text()).toBe(true);
    const nextTimerUrl = `/api/v1/workspaces/${nextWorkspace.id}/tasks/${nextTask.id}/timer`;
    const revoke = await page.request.delete(`${membersUrl}/${identity.userId}`);
    expect(revoke.ok(), await revoke.text()).toBe(true);
    expect((await person.request.get(timerUrl)).status()).toBe(404);
    const blocked = await person.request.post(nextTimerUrl, {
      data: {
        expectedActorId: identity.userId,
        expectedSessionId: identity.sessionId,
        requestId: crypto.randomUUID(),
        operation: "start",
        expectedVersion: 0,
        runId: null,
        note: null,
      },
    });
    expect(blocked.status(), await blocked.text()).toBe(409);
    const ownerShape = z.object({
      runId: z.null(),
      visibleRun: z.null(),
      legacyOpenIds: z.array(z.string()),
    });
    const state = ownerShape.parse(
      await (await person.request.get("/api/v1/me/task-timer")).json(),
    );
    expect(state.legacyOpenIds).toEqual([entry.id]);
    await person.goto(`/w/${nextSlug}/my-tasks`);
    const visible = person.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer" &&
        response.status() === 200 &&
        ownerShape.parse(await response.json()).legacyOpenIds.includes(entry.id),
    );
    await person.bringToFront();
    await visible;
    await testInfo.attach("legacy-only-native-owner-original-precondition", {
      body: JSON.stringify({
        actor: identity.userId,
        entryId: entry.id,
        originalRaw: JSON.parse(originalRaw) as unknown,
        hiddenTask404: true,
        otherWorkspaceStart409: true,
        canonicalOwner: state,
      }),
      contentType: "application/json",
    });
    const owner = person.getByTestId("timer-legacy-owner");
    // Literal original negative: the global owner must offer opaque cleanup
    // even when there is no modern run and the legacy task is inaccessible.
    await expect(owner).toBeVisible();
    await expect(owner).not.toContainText("접근 회수된 기존 기록의 숨겨진 제목");
    await expect(owner).not.toContainText("기존 비공개 미종료 메모");
    const releaseResult = person.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer/legacy-release" &&
        response.request().method() === "POST",
    );
    await owner.getByRole("button", { name: "미종료 기록의 측정 제한 해제", exact: true }).click();
    const released = await releaseResult;
    expect(released.status(), await released.text()).toBe(200);
    const command = z
      .object({
        expectedActorId: z.string(),
        expectedSessionId: z.string(),
        requestId: z.string(),
        timeEntryId: z.string(),
      })
      .parse(released.request().postDataJSON());
    expect(command).toMatchObject({
      expectedActorId: identity.userId,
      expectedSessionId: identity.sessionId,
      timeEntryId: entry.id,
    });
    await expect(owner).toHaveCount(0);
    expect(
      diagnosticSql(`SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE e.id='${entry.id}'`),
    ).toBe(originalRaw);
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE user_id='${identity.userId}' AND time_entry_id='${entry.id}' AND verb='legacy.release' AND reason='explicit_release_original_range_unresolved'`,
      ),
    ).toBe("1");
    const beforeReplay = timerDatabaseEffects(identity.userId, task.id);
    const replay = await person.request.post("/api/v1/me/task-timer/legacy-release", {
      data: command,
    });
    expect(replay.status(), await replay.text()).toBe(200);
    expect(await replay.json()).toEqual(await released.json());
    const changed = await person.request.post("/api/v1/me/task-timer/legacy-release", {
      data: { ...command, timeEntryId: nextTask.id },
    });
    expect(changed.status(), await changed.text()).toBe(409);
    expect(timerDatabaseEffects(identity.userId, task.id)).toBe(beforeReplay);
    await person.reload();
    expect(
      ownerShape.parse(await (await person.request.get("/api/v1/me/task-timer")).json())
        .legacyOpenIds,
    ).toEqual([]);
    const next = person.getByTestId(`task-stopwatch-${nextTask.id}`);
    await next.getByTestId("timer-start").click();
    await expect(next.getByTestId("timer-state")).toHaveText("측정 중");
    await next.getByTestId("timer-stop").click();
    await expect(next.getByTestId("timer-start")).toBeEnabled();
    expect(
      diagnosticSql(`SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE e.id='${entry.id}'`),
    ).toBe(originalRaw);
  } finally {
    await context.close();
  }
});

test("a late legacy release cannot clear a genuine successor release pending", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TLEGSTALE", true);
  const entriesUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}/time-entries`;
  const createLegacy = async (
    client: import("@playwright/test").Page,
    note: string,
    target = entriesUrl,
  ) => {
    const response = await client.request.post(target, {
      data: { startedAt: "2026-09-29T01:02:03.123Z", note },
    });
    expect(response.status(), await response.text()).toBe(201);
    return z.object({ id: z.string() }).parse(await response.json()).id;
  };
  const firstEntry = await createLegacy(page, "첫 미종료 기록 보존");
  const rawEntry = (id: string) =>
    diagnosticSql(`SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE e.id='${id}'`);
  const firstRaw = rawEntry(firstEntry);
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  const owner = page.getByTestId("timer-legacy-owner");
  const button = owner.getByRole("button", { name: "미종료 기록의 측정 제한 해제", exact: true });
  await expect(button).toBeEnabled();
  await page.setViewportSize({ width: 320, height: 900 });
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "200%";
  });
  await button.focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(button).toBeFocused();
  await testInfo.attach("legacy-owner-320-text200-keyboard", {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "";
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => {
    const witness = document.createElement("script");
    witness.type = "application/json";
    witness.dataset.testid = "legacy-delivery-witness";
    document.body.append(witness);
    const requested: string[] = [];
    const completed: string[] = [];
    let canonical: string[] = [];
    const publish = () => {
      witness.textContent = JSON.stringify({ requested, completed, canonical });
    };
    publish();
    const nativeFetch = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input, init) => {
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, location.href).pathname;
      const releasing = method === "POST" && pathname === "/api/v1/me/task-timer/legacy-release";
      const reading = method === "GET" && pathname === "/api/v1/me/task-timer";
      if (!releasing && !reading) return nativeFetch(input, init);
      let entryId = "";
      if (releasing) {
        let body: unknown;
        if (input instanceof Request) body = await input.clone().json();
        else {
          if (typeof init?.body !== "string") throw new Error("actual release JSON missing");
          body = JSON.parse(init.body);
        }
        if (
          typeof body !== "object" ||
          body === null ||
          !("timeEntryId" in body) ||
          typeof body.timeEntryId !== "string"
        )
          throw new Error("actual release entry identity missing");
        entryId = body.timeEntryId;
        requested.push(entryId);
        publish();
      }
      const response = await nativeFetch(input, init);
      const nativeJson = response.json.bind(response);
      response.json = async () => {
        const result: unknown = await nativeJson();
        const delivery = new MessageChannel();
        delivery.port1.onmessage = () => {
          if (releasing) completed.push(entryId);
          else if (
            typeof result === "object" &&
            result !== null &&
            "legacyOpenIds" in result &&
            Array.isArray(result.legacyOpenIds) &&
            result.legacyOpenIds.every((id): id is string => typeof id === "string")
          )
            canonical = result.legacyOpenIds;
          publish();
          delivery.port1.close();
          delivery.port2.close();
        };
        delivery.port2.postMessage(entryId);
        return result;
      };
      return response;
    };
  });
  const held = Array.from({ length: 2 }, () => {
    let release = () => {};
    let committed = () => {};
    const delivery = new Promise<void>((resolve) => {
      release = resolve;
    });
    const commit = new Promise<void>((resolve) => {
      committed = resolve;
    });
    const command: {
      release: () => void;
      committed: () => void;
      delivery: Promise<void>;
      commit: Promise<void>;
      body: unknown;
    } = { release, committed, delivery, commit, body: undefined };
    return command;
  });
  const [first, second] = held;
  if (!first || !second) throw new Error("missing two actual release gates");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  const preparationContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let releaseOwner = () => {};
  const ownerDelivery = new Promise<void>((resolve) => {
    releaseOwner = resolve;
  });
  let count = 0;
  await page.route(
    (url) => url.pathname === "/api/v1/me/task-timer/legacy-release",
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const command = held[count++];
      if (!command) throw new Error("unexpected third release request");
      command.body = route.request().postDataJSON();
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      command.committed();
      await command.delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  const observations = async () =>
    z
      .object({
        requested: z.array(z.string()),
        completed: z.array(z.string()),
        canonical: z.array(z.string()),
      })
      .parse(
        JSON.parse((await page.getByTestId("legacy-delivery-witness").textContent()) ?? "null"),
      );
  try {
    // Releasing the global reservation preserves the old raw open entry and
    // its immutable workspace-local unique index. Prepare the successor in
    // another ordinary workspace instead of changing that historical row.
    const setupEmail = "timer-legacy-successor-setup@example.com";
    createE2eUser(setupEmail, credentials.password, "후속 작업 공간 준비", {
      workspaceSlug: fixture.slug,
      membershipRole: "member",
    });
    expect(
      diagnosticSql(
        `UPDATE fvoci.users SET is_instance_admin=true WHERE email='${setupEmail}' AND NOT is_instance_admin RETURNING id`,
      ),
    ).toMatch(/^[0-9a-f-]{36}$/);
    const preparation = await preparationContext.newPage();
    await login(preparation, setupEmail, credentials.password);
    const nextWorkspaceResponse = await preparation.request.post("/api/v1/workspaces", {
      data: { name: "후속 미종료 기록 검증", slug: "w5-legacy-successor" },
    });
    expect(nextWorkspaceResponse.status(), await nextWorkspaceResponse.text()).toBe(201);
    const nextWorkspace = z.object({ id: z.string() }).parse(await nextWorkspaceResponse.json());
    expect(nextWorkspace.id).not.toBe(fixture.workspaceId);
    const invitation = await preparation.request.post(
      `/api/v1/workspaces/${nextWorkspace.id}/invitations`,
      { data: { email: fixture.email, role: "member" } },
    );
    expect(invitation.status(), await invitation.text()).toBe(201);
    const acceptUrl = z.object({ acceptUrl: z.string() }).parse(await invitation.json()).acceptUrl;
    const token = new URL(acceptUrl).pathname.split("/invite/")[1];
    if (!token) throw new Error("successor workspace invitation token missing");
    await button.click();
    await first.commit;
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const freshActor = identityShape.parse(
      await (await fresh.request.get("/api/v1/auth/me")).json(),
    );
    expect(freshActor.userId).toBe(fixture.actor.userId);
    expect(freshActor.sessionId).not.toBe(fixture.actor.sessionId);
    const accepted = await fresh.request.post(`/api/v1/invitations/${token}/accept`, {
      data: { password: credentials.password },
    });
    expect(accepted.status(), await accepted.text()).toBe(200);
    const acceptedActorResponse: unknown = await (
      await fresh.request.get("/api/v1/auth/me")
    ).json();
    expect(identityShape.parse(acceptedActorResponse).userId).toBe(fixture.actor.userId);
    z.object({ isInstanceAdmin: z.literal(false) }).parse(acceptedActorResponse);
    const nextProjectResponse = await fresh.request.post(
      `/api/v1/workspaces/${nextWorkspace.id}/projects`,
      { data: { key: "TLEGNEXT", name: "일반 후속 작업", visibility: "workspace" } },
    );
    expect(nextProjectResponse.status(), await nextProjectResponse.text()).toBe(201);
    const nextProject = z.object({ id: z.string() }).parse(await nextProjectResponse.json());
    const nextTaskResponse = await fresh.request.post(
      `/api/v1/workspaces/${nextWorkspace.id}/projects/${nextProject.id}/tasks`,
      { data: { title: "다른 작업 공간의 실제 후속 미종료 기록" } },
    );
    expect(nextTaskResponse.status(), await nextTaskResponse.text()).toBe(201);
    const nextTask = taskShape.parse(await nextTaskResponse.json());
    const secondEntry = await createLegacy(
      fresh,
      "후속 미종료 기록 보존",
      `/api/v1/workspaces/${nextWorkspace.id}/tasks/${nextTask.id}/time-entries`,
    );
    expect(secondEntry).not.toBe(firstEntry);
    const secondRaw = rawEntry(secondEntry);
    const successorTaskRaw = () =>
      diagnosticSql(`SELECT to_jsonb(t) FROM fvoci.tasks t WHERE t.id='${nextTask.id}'`);
    const originalSuccessorTask = successorTaskRaw();
    await page.bringToFront();
    await expect.poll(async () => (await observations()).canonical).toEqual([secondEntry]);
    await expect(button).toBeEnabled();
    await page.route(
      (url) => url.pathname === "/api/v1/me/task-timer",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const response = await route.fetch();
        await ownerDelivery;
        if (!page.isClosed()) await route.fulfill({ response });
      },
    );
    await button.click();
    await second.commit;
    expect(
      z
        .object({
          timeEntryId: z.string(),
          expectedActorId: z.string(),
          expectedSessionId: z.string(),
        })
        .parse(second.body),
    ).toMatchObject({
      timeEntryId: secondEntry,
      expectedActorId: fixture.actor.userId,
      expectedSessionId: fixture.actor.sessionId,
    });
    await expect(button).toBeDisabled();
    const beforeDelivery = timerDatabaseEffects(fixture.actor.userId, fixture.task.id);
    first.release();
    await expect.poll(async () => (await observations()).completed).toContain(firstEntry);
    await expect(button).toBeDisabled();
    await button.evaluate((element) => {
      if (!(element instanceof HTMLButtonElement)) throw new Error("native release button missing");
      element.click();
    });
    expect((await observations()).requested).toEqual([firstEntry, secondEntry]);
    expect(count).toBe(2);
    expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(beforeDelivery);
    expect(rawEntry(firstEntry)).toBe(firstRaw);
    expect(rawEntry(secondEntry)).toBe(secondRaw);
    expect(successorTaskRaw()).toBe(originalSuccessorTask);
    await testInfo.attach("legacy-successor-pending-old-completion-native", {
      body: JSON.stringify({
        firstEntry,
        secondEntry,
        observations: await observations(),
        commands: held.map((command) => command.body),
        count,
        rawEntriesPreserved: true,
      }),
      contentType: "application/json",
    });
    second.release();
    releaseOwner();
    await expect(owner).toHaveCount(0);
    const freshOwner = z
      .object({ legacyOpenIds: z.array(z.string()) })
      .parse(await (await fresh.request.get("/api/v1/me/task-timer")).json());
    expect(freshOwner.legacyOpenIds).toEqual([]);
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE user_id='${fixture.actor.userId}' AND verb='legacy.release' AND reason='explicit_release_original_range_unresolved'`,
      ),
    ).toBe("2");
    expect(rawEntry(firstEntry)).toBe(firstRaw);
    expect(rawEntry(secondEntry)).toBe(secondRaw);
    expect(successorTaskRaw()).toBe(originalSuccessorTask);
  } finally {
    for (const command of held) command.release();
    releaseOwner();
    await preparationContext.close();
    await context.close();
  }
});

test("real browser offline start preserves one command and recovers one native outcome", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TOFF", true);
  await page.goto(fixture.detail);
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  await expect(widget.getByTestId("timer-start")).toBeEnabled();
  const before = timerDatabaseEffects(fixture.actor.userId, fixture.task.id);
  const attempts: unknown[] = [];
  page.on("request", (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === fixture.timerUrl)
      attempts.push(request.postDataJSON());
  });
  const retry = widget.getByRole("button", { name: "같은 요청 다시 보내기", exact: true });
  const freshContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    await page.context().setOffline(true);
    await widget.getByTestId("timer-start").click();
    await expect(retry).toBeEnabled();
    expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(before);
    expect(attempts).toHaveLength(1);
    await page.context().setOffline(false);
    await retry.click();
    await expect(widget.getByTestId("timer-state")).toHaveText("측정 중");
    expect(attempts).toHaveLength(2);
    expect(attempts[1]).toEqual(attempts[0]);
    const capture = z
      .object({ requestId: z.string(), expectedActorId: z.string(), expectedSessionId: z.string() })
      .parse(attempts[0]);
    expect(capture.expectedActorId).toBe(fixture.actor.userId);
    expect(capture.expectedSessionId).toBe(fixture.actor.sessionId);
    const native = timerShape.parse(await (await page.request.get(fixture.timerUrl)).json());
    expect(native.run?.status).toBe("running");
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_commands WHERE user_id='${fixture.actor.userId}' AND request_id='${capture.requestId}'`,
      ),
    ).toBe("1");
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE user_id='${fixture.actor.userId}' AND request_id='${capture.requestId}'`,
      ),
    ).toBe("1");
    const fresh = await freshContext.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    expect(identity.sessionId).not.toBe(fixture.actor.sessionId);
    await fresh.goto(`/w/${fixture.slug}/my-tasks`);
    const current = fresh.getByTestId(`task-stopwatch-${fixture.task.id}`);
    await expect(current.getByTestId("timer-state")).toHaveText("측정 중");
    expect(timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run?.id).toBe(
      native.run?.id,
    );
    await current.getByTestId("timer-stop").click();
    await expect(current.getByTestId("timer-start")).toBeEnabled();
    expect(
      taskShape.parse(
        await (
          await fresh.request.get(
            `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`,
          )
        ).json(),
      ).statusId,
    ).toBe(fixture.task.statusId);
    await testInfo.attach("native-offline-command-recovery", {
      body: JSON.stringify({
        requestId: capture.requestId,
        attempts: 2,
        offlineDatabaseUnchanged: true,
        replayBodyIdentical: true,
        runId: native.run?.id,
        nativeReceiptCount: 1,
        nativeAuditCount: 1,
        freshSessionRead: true,
        stopLeavesTaskIncomplete: true,
      }),
      contentType: "application/json",
    });
  } finally {
    await page.context().setOffline(false);
    await freshContext.close();
  }
});

test("a native committed pause with its response lost replays one unchanged receipt", async ({
  page,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TLOST", true);
  await page.goto(fixture.detail);
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  await expect(widget.getByTestId("timer-start")).toBeEnabled();
  await widget.getByTestId("timer-start").click();
  await expect(widget.getByTestId("timer-state")).toHaveText("측정 중");
  let release = () => {};
  const readDelivery = new Promise<void>((resolve) => {
    release = resolve;
  });
  const attempts: unknown[] = [];
  const outcomes: unknown[] = [];
  await page.route(
    (url) => url.pathname === fixture.timerUrl || url.pathname === "/api/v1/me/task-timer",
    async (route) => {
      if (route.request().method() === "GET") {
        const response = await route.fetch();
        await readDelivery;
        if (!page.isClosed()) await route.fulfill({ response });
        return;
      }
      const body: unknown = route.request().postDataJSON();
      if (z.object({ operation: z.string() }).parse(body).operation !== "pause")
        return route.continue();
      attempts.push(body);
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      outcomes.push(await response.json());
      if (attempts.length === 1) await route.abort("failed");
      else await route.fulfill({ response });
    },
  );
  try {
    await widget.getByTestId("timer-pause").click();
    const retry = widget.getByRole("button", { name: "같은 요청 다시 보내기", exact: true });
    await expect(retry).toBeEnabled();
    expect(attempts).toHaveLength(1);
    const paused = timerShape.parse(await (await page.request.get(fixture.timerUrl)).json());
    expect(paused.run?.status).toBe("paused");
    const committed = timerDatabaseEffects(fixture.actor.userId, fixture.task.id);
    const replay = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === fixture.timerUrl &&
        response.request().method() === "POST" &&
        z.object({ operation: z.string() }).parse(response.request().postDataJSON()).operation ===
          "pause",
    );
    await retry.click();
    expect((await replay).status()).toBe(200);
    await expect(retry).toHaveCount(0);
    expect(attempts).toHaveLength(2);
    expect(attempts[1]).toEqual(attempts[0]);
    expect(outcomes).toHaveLength(2);
    expect(outcomes[1]).toEqual(outcomes[0]);
    expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(committed);
    const capture = z
      .object({ requestId: z.string(), expectedActorId: z.string(), expectedSessionId: z.string() })
      .passthrough()
      .parse(attempts[0]);
    const changed = await page.request.post(fixture.timerUrl, {
      data: { ...capture, note: "different replay payload" },
    });
    expect(changed.status(), await changed.text()).toBe(409);
    expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(committed);
    release();
    await expect(widget.getByTestId("timer-state")).toHaveText("일시정지");
    expect(timerShape.parse(await (await page.request.get(fixture.timerUrl)).json()).run).toEqual(
      paused.run,
    );
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_commands WHERE user_id='${fixture.actor.userId}' AND request_id='${capture.requestId}'`,
      ),
    ).toBe("1");
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE user_id='${fixture.actor.userId}' AND request_id='${capture.requestId}'`,
      ),
    ).toBe("1");
    await widget.getByTestId("timer-stop").click();
    await expect(widget.getByTestId("timer-start")).toBeEnabled();
    await testInfo.attach("native-lost-success-identical-replay", {
      body: JSON.stringify({
        requestId: capture.requestId,
        firstNativeCommit: 200,
        firstBrowserDelivery: "net::ERR_FAILED after actual commit",
        replayNativeCommit: 200,
        sameBodyAndOutcome: true,
        changedPayloadStatus: 409,
        receiptAndAuditCount: 1,
        replayAndChangedPayloadDatabaseUnchanged: true,
        pausedElapsed: paused.run?.elapsedMilliseconds,
      }),
      contentType: "application/json",
    });
  } finally {
    release();
  }
});

// Observe actual planning/estimate JSON consumption after SPA target changes.
// This never substitutes a DTO, query value, route or mutation result.
async function observePlanningCompletion(page: import("@playwright/test").Page) {
  await page.evaluate(() => {
    const witness = document.createElement("script");
    witness.type = "application/json";
    witness.dataset.testid = "planning-delivery-witness";
    document.body.append(witness);
    const completed: string[] = [];
    witness.textContent = JSON.stringify(completed);
    const nativeFetch = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input, init) => {
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, location.href).pathname;
      if (
        method !== "POST" ||
        !(pathname.endsWith("/study-plan/task") || pathname.endsWith("/timer/estimate"))
      )
        return nativeFetch(input, init);
      const body: unknown =
        input instanceof Request
          ? await input.clone().json()
          : typeof init?.body === "string"
            ? JSON.parse(init.body)
            : null;
      if (
        typeof body !== "object" ||
        body === null ||
        !("requestId" in body) ||
        typeof body.requestId !== "string"
      )
        throw new Error("planning witness requires the actual request identity");
      const requestId = body.requestId;
      const response = await nativeFetch(input, init);
      const nativeJson = response.json.bind(response);
      response.json = async () => {
        const result: unknown = await nativeJson();
        const delivery = new MessageChannel();
        delivery.port1.onmessage = () => {
          completed.push(requestId);
          witness.textContent = JSON.stringify(completed);
          delivery.port1.close();
          delivery.port2.close();
        };
        delivery.port2.postMessage(requestId);
        return result;
      };
      return response;
    };
  });
}

async function planningCompletion(page: import("@playwright/test").Page, requestId: string) {
  await expect
    .poll(async () =>
      z
        .array(z.string())
        .parse(
          JSON.parse((await page.getByTestId("planning-delivery-witness").textContent()) ?? "null"),
        ),
    )
    .toContain(requestId);
}

async function fillPlanningDraft(
  page: import("@playwright/test").Page,
  fixture: Awaited<ReturnType<typeof ordinaryTimerTask>>,
  documents: readonly { number: number }[],
  title: string,
) {
  const [notes, material] = documents;
  if (!notes || !material) throw new Error("planning fixture documents missing");
  const planner = page.getByTestId("study-plan-builder");
  await planner.getByText("학습·연구·업무 계획 만들기", { exact: true }).click();
  await planner.getByLabel("목표", { exact: true }).fill(title);
  await planner
    .getByLabel("목표·연구 노트 문서 링크", { exact: true })
    .fill(`WIKI-${String(notes.number)}`);
  await planner
    .getByLabel("읽을 자료 문서 링크", { exact: true })
    .fill(`WIKI-${String(material.number)}`);
  await planner.getByRole("button", { name: "연결할 문서 확인", exact: true }).click();
  const project = z
    .object({ projectId: z.string() })
    .parse(
      await (
        await page.request.get(`/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`)
      ).json(),
    );
  const destination = planner.getByLabel("저장할 프로젝트", { exact: true });
  await expect(destination.locator(`option[value="${project.projectId}"]`)).toHaveCount(1);
  await destination.selectOption(project.projectId);
  return planner;
}

test("a planner A-B-A target change cannot accept a retired goal into the new pending plan", async ({
  page,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TPABA", true);
  const documents: Array<{ id: string; number: number }> = [];
  for (const title of ["이전과 현재의 실제 목표 노트", "이전과 현재의 실제 읽기 자료"]) {
    const response = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/documents`,
      {
        data: { parentId: null, title },
      },
    );
    expect(response.status(), await response.text()).toBe(201);
    documents.push(z.object({ id: z.string(), number: z.number() }).parse(await response.json()));
  }
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  await observePlanningCompletion(page);
  const planner = await fillPlanningDraft(page, fixture, documents, "퇴역한 첫 목표");
  const held: Array<{
    body: { requestId: string };
    result: { taskId: string };
    release: () => void;
  }> = [];
  let requests = 0;
  await page.route(
    (url) => url.pathname.endsWith("/study-plan/task"),
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      requests++;
      if (requests > 2) return route.continue();
      const body = z.object({ requestId: z.string() }).parse(route.request().postDataJSON());
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      const result = z.object({ taskId: z.string() }).parse(await response.json());
      let release = () => {};
      const delivery = new Promise<void>((resolve) => {
        release = resolve;
      });
      held.push({ body, result, release });
      await delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  try {
    await planner.getByRole("button", { name: "계획 저장", exact: true }).click();
    await expect.poll(() => held.length).toBe(1);
    const first = held[0];
    if (!first) throw new Error("first native goal commit missing");
    await page.getByTestId(`my-task-${fixture.task.id}`).click();
    await expect(page).toHaveURL(new RegExp(`${fixture.detail}$`));
    await page.goBack();
    await expect(page.getByTestId("my-tasks")).toBeVisible();
    const returning = await fillPlanningDraft(page, fixture, documents, "반환한 현재 목표");
    await returning.getByRole("button", { name: "계획 저장", exact: true }).click();
    await expect.poll(() => held.length).toBe(2);
    const second = held[1];
    if (!second) throw new Error("returning native goal commit missing");
    expect(second.result.taskId).not.toBe(first.result.taskId);
    expect(second.body.requestId).not.toBe(first.body.requestId);
    const beforeDelivery = timerDatabaseEffects(fixture.actor.userId, fixture.task.id);
    first.release();
    await planningCompletion(page, first.body.requestId);
    await expect(returning.getByRole("button", { name: "계획 저장", exact: true })).toBeDisabled();
    await expect(returning.getByLabel("목표", { exact: true })).toHaveValue("반환한 현재 목표");
    await expect(
      returning.getByRole("status").filter({ hasText: "저장된 목표와 단계" }),
    ).toHaveCount(0);
    expect(requests).toBe(2);
    expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(beforeDelivery);
    expect(
      diagnosticSql(`SELECT count(*) FROM fvoci.tasks WHERE parent_id='${first.result.taskId}'`),
    ).toBe("0");
    second.release();
    await expect(
      returning.getByRole("status").filter({ hasText: "저장된 목표와 단계" }),
    ).toContainText("4개 저장됨");
    expect(requests).toBe(5);
    expect(
      diagnosticSql(`SELECT count(*) FROM fvoci.tasks WHERE parent_id='${second.result.taskId}'`),
    ).toBe("3");
    expect(
      diagnosticSql(`SELECT count(*) FROM fvoci.tasks WHERE parent_id='${first.result.taskId}'`),
    ).toBe("0");
    await expect(
      returning.getByRole("status").filter({ hasText: "저장된 목표와 단계" }),
    ).not.toContainText("퇴역한 첫 목표");
    await testInfo.attach("native-planner-target-ABA", {
      body: JSON.stringify({
        firstGoal: first.result.taskId,
        returningGoal: second.result.taskId,
        firstNativeCommit: 200,
        firstRealJsonConsumedWhileReturningPending: true,
        currentDraftAndPendingPreserved: true,
        originalGoalChildCount: 0,
        returningGoalChildCount: 3,
        actualRequests: 5,
      }),
      contentType: "application/json",
    });
  } finally {
    for (const command of held) command.release();
  }
});

test("an estimate A-B-A target change cannot clear the current native mutation pending state", async ({
  page,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TEABA", true);
  const project = z
    .object({ projectId: z.string() })
    .parse(
      await (
        await page.request.get(`/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`)
      ).json(),
    );
  const created = await page.request.post(
    `/api/v1/workspaces/${fixture.workspaceId}/projects/${project.projectId}/tasks`,
    { data: { title: "변경하지 않을 다른 예상 시간 대상" } },
  );
  expect(created.status(), await created.text()).toBe(201);
  const other = taskShape.parse(await created.json());
  const assigned = await page.request.patch(
    `/api/v1/workspaces/${fixture.workspaceId}/tasks/${other.id}`,
    { data: { assigneeIds: [fixture.actor.userId] } },
  );
  expect(assigned.ok(), await assigned.text()).toBe(true);
  const otherRaw = diagnosticSql(`SELECT to_jsonb(t) FROM fvoci.tasks t WHERE id='${other.id}'`);
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  await observePlanningCompletion(page);
  await page.getByTestId(`my-task-${fixture.task.id}`).click();
  await expect(page).toHaveURL(new RegExp(`${fixture.detail}$`));
  await expect(page.locator("h1.task-detail__title")).toHaveText("TEABA 기록 검증");
  await expect(page.getByTestId("my-tasks")).toHaveCount(0);
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  const editor = widget.getByTestId("task-estimate-editor");
  await editor.getByText("예상 시간 설정", { exact: true }).click();
  await editor.getByLabel("예상 시간(분)", { exact: true }).fill("45");
  await editor.getByLabel("예상 시간 변경 사유", { exact: true }).fill("첫 대상의 원래 요청");
  const held: Array<{ requestId: string; release: () => void }> = [];
  await page.route(
    (url) => url.pathname === `${fixture.timerUrl}/estimate`,
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const body = z.object({ requestId: z.string() }).parse(route.request().postDataJSON());
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      let release = () => {};
      const delivery = new Promise<void>((resolve) => {
        release = resolve;
      });
      held.push({ requestId: body.requestId, release });
      await delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  try {
    await editor.getByRole("button", { name: "예상 시간 저장", exact: true }).click();
    await expect.poll(() => held.length).toBe(1);
    await page.goBack();
    await expect(page.getByTestId("my-tasks")).toBeVisible();
    await page.getByTestId(`my-task-${other.id}`).click();
    await expect(page).toHaveURL(new RegExp(`/w/${fixture.slug}/TEABA-${String(other.number)}$`));
    await expect(page.locator("h1.task-detail__title")).toHaveText(
      "변경하지 않을 다른 예상 시간 대상",
    );
    await expect(page.getByTestId("my-tasks")).toHaveCount(0);
    await expect(page.getByTestId(`task-stopwatch-${other.id}`)).toBeVisible();
    await page.goBack();
    await expect(page.getByTestId("my-tasks")).toBeVisible();
    await page.getByTestId(`my-task-${fixture.task.id}`).click();
    await expect(page).toHaveURL(new RegExp(`${fixture.detail}$`));
    await expect(page.locator("h1.task-detail__title")).toHaveText("TEABA 기록 검증");
    await expect(page.getByTestId("my-tasks")).toHaveCount(0);
    await expect(widget.getByTestId("timer-estimate")).toHaveText("예상 45분");
    await editor.getByText("예상 시간 설정", { exact: true }).click();
    await editor.getByLabel("예상 시간(분)", { exact: true }).fill("60");
    await editor
      .getByLabel("예상 시간 변경 사유", { exact: true })
      .fill("반환한 현재 대상의 새 요청");
    await editor.getByRole("button", { name: "예상 시간 저장", exact: true }).click();
    await expect.poll(() => held.length).toBe(2);
    const [first, second] = held;
    if (!first || !second) throw new Error("two real estimate commits missing");
    expect(first.requestId).not.toBe(second.requestId);
    const beforeDelivery = timerDatabaseEffects(fixture.actor.userId, fixture.task.id);
    first.release();
    await planningCompletion(page, first.requestId);
    await expect(
      editor.getByRole("button", { name: "예상 시간 저장", exact: true }),
    ).toBeDisabled();
    await expect(editor.getByLabel("예상 시간(분)", { exact: true })).toHaveValue("60");
    await expect(editor.getByLabel("예상 시간 변경 사유", { exact: true })).toHaveValue(
      "반환한 현재 대상의 새 요청",
    );
    expect(held).toHaveLength(2);
    expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(beforeDelivery);
    expect(diagnosticSql(`SELECT to_jsonb(t) FROM fvoci.tasks t WHERE id='${other.id}'`)).toBe(
      otherRaw,
    );
    second.release();
    await planningCompletion(page, second.requestId);
    await expect(editor.getByRole("button", { name: "예상 시간 저장", exact: true })).toBeEnabled();
    await expect(widget.getByTestId("timer-estimate")).toHaveText("예상 60분");
    expect(held).toHaveLength(2);
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE task_id='${fixture.task.id}' AND verb='task.estimate.minutes'`,
      ),
    ).toBe("2");
    await testInfo.attach("native-estimate-target-ABA", {
      body: JSON.stringify({
        retiredRequest: first.requestId,
        currentRequest: second.requestId,
        bothNativeCommits: 200,
        retiredRealJsonConsumedWhileCurrentPending: true,
        currentDraftAndPendingPreserved: true,
        otherTaskFullRowUnchanged: true,
        finalExplicitMinutes: 60,
        auditCount: 2,
      }),
      contentType: "application/json",
    });
  } finally {
    for (const command of held) command.release();
  }
});

test("a genuine new session retires held planning and estimate successes without touching current pending drafts", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TPSSESSION", true);
  const documents: Array<{ id: string; number: number }> = [];
  for (const title of ["세션 경계의 실제 연구 노트", "세션 경계의 실제 읽기 자료"]) {
    const response = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/documents`,
      {
        data: { parentId: null, title },
      },
    );
    expect(response.status(), await response.text()).toBe(201);
    documents.push(z.object({ id: z.string(), number: z.number() }).parse(await response.json()));
  }
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  await observePlanningCompletion(page);
  const planner = await fillPlanningDraft(page, fixture, documents, "이전 세션의 퇴역 목표");
  await planner.getByLabel("목표", { exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(planner.getByLabel("목표 예상 시간(분, 선택)", { exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(planner.getByLabel("목표·연구 노트 문서 링크", { exact: true })).toBeFocused();
  await planner.screenshot({ path: testInfo.outputPath("planner-real-keyboard-focus.png") });
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  const editor = widget.getByTestId("task-estimate-editor");
  await editor.getByText("예상 시간 설정", { exact: true }).click();
  await editor.getByLabel("예상 시간(분)", { exact: true }).fill("45");
  await editor
    .getByLabel("예상 시간 변경 사유", { exact: true })
    .fill("이전 세션의 실제 예상 요청");
  const captureShape = z.object({
    requestId: z.string(),
    expectedActorId: z.string(),
    expectedSessionId: z.string(),
  });
  type Held = { body: z.infer<typeof captureShape>; result: unknown; release: () => void };
  const plans: Held[] = [];
  const estimates: Held[] = [];
  const planRequests: Array<z.infer<typeof captureShape>> = [];
  await page.route(
    (url) =>
      url.pathname.endsWith("/study-plan/task") || url.pathname === `${fixture.timerUrl}/estimate`,
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const planning = new URL(route.request().url()).pathname.endsWith("/study-plan/task");
      const body = captureShape.parse(route.request().postDataJSON());
      if (planning) planRequests.push(body);
      const commands = planning ? plans : estimates;
      if (commands.length >= 2) return route.continue();
      const native = await route.fetch();
      expect(native.status(), await native.text()).toBe(200);
      const result: unknown = await native.json();
      let release = () => {};
      const delivery = new Promise<void>((resolve) => {
        release = resolve;
      });
      commands.push({ body, result, release });
      await delivery;
      if (!page.isClosed()) await route.fulfill({ response: native });
    },
  );
  const successorContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    await planner.getByRole("button", { name: "계획 저장", exact: true }).click();
    await editor.getByRole("button", { name: "예상 시간 저장", exact: true }).click();
    await expect.poll(() => plans.length).toBe(1);
    await expect.poll(() => estimates.length).toBe(1);
    const oldPlan = plans[0];
    const oldEstimate = estimates[0];
    if (!oldPlan || !oldEstimate) throw new Error("two original native commits missing");
    expect(oldPlan.body.expectedSessionId).toBe(fixture.actor.sessionId);
    expect(oldEstimate.body.expectedSessionId).toBe(fixture.actor.sessionId);
    const successorPage = await successorContext.newPage();
    await login(successorPage, fixture.email, credentials.password);
    const successor = identityShape.parse(
      await (await successorPage.request.get("/api/v1/auth/me")).json(),
    );
    expect(successor.userId).toBe(fixture.actor.userId);
    expect(successor.sessionId).not.toBe(fixture.actor.sessionId);
    const actualMe = page.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === "/api/v1/auth/me" &&
        response.status() === 200 &&
        identityShape.parse(await response.json()).sessionId === successor.sessionId,
    );
    const actualTimer = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === fixture.timerUrl &&
        new URL(response.url()).searchParams.get("expectedSessionId") === successor.sessionId &&
        response.status() === 200,
    );
    await page.context().addCookies(await successorContext.cookies());
    await page.bringToFront();
    await actualMe;
    await actualTimer;
    await expect(page.getByTestId("my-tasks")).toBeVisible();
    await expect(page.getByTestId("planning-delivery-witness")).toHaveCount(1);
    await expect(planner.getByLabel("목표", { exact: true })).toHaveValue("");
    await expect(planner.getByRole("status").filter({ hasText: "저장된 목표와 단계" })).toHaveCount(
      0,
    );
    const [notes, material] = documents;
    if (!notes || !material) throw new Error("actual two planning documents missing");
    // Read the actual current forms after real me/query session refresh.
    if ((await planner.getAttribute("open")) === null)
      await planner.getByText("학습·연구·업무 계획 만들기", { exact: true }).click();
    if ((await editor.getAttribute("open")) === null)
      await editor.getByText("예상 시간 설정", { exact: true }).click();
    await planner.getByLabel("목표", { exact: true }).fill("새 세션의 현재 목표");
    await planner
      .getByLabel("목표·연구 노트 문서 링크", { exact: true })
      .fill(`WIKI-${String(notes.number)}`);
    await planner
      .getByLabel("읽을 자료 문서 링크", { exact: true })
      .fill(`WIKI-${String(material.number)}`);
    await planner.getByRole("button", { name: "연결할 문서 확인", exact: true }).click();
    const project = z
      .object({ projectId: z.string() })
      .parse(
        await (
          await page.request.get(
            `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`,
          )
        ).json(),
      );
    await expect(
      planner
        .getByLabel("저장할 프로젝트", { exact: true })
        .locator(`option[value="${project.projectId}"]`),
    ).toHaveCount(1);
    await planner.getByLabel("저장할 프로젝트", { exact: true }).selectOption(project.projectId);
    await expect(editor.getByLabel("예상 시간(분)", { exact: true })).toHaveValue("45");
    await editor.getByLabel("예상 시간(분)", { exact: true }).fill("60");
    await editor
      .getByLabel("예상 시간 변경 사유", { exact: true })
      .fill("새 세션의 현재 예상 요청");
    await planner.getByRole("button", { name: "계획 저장", exact: true }).click();
    await editor.getByRole("button", { name: "예상 시간 저장", exact: true }).click();
    await expect.poll(() => plans.length).toBe(2);
    await expect.poll(() => estimates.length).toBe(2);
    const currentPlan = plans[1];
    const currentEstimate = estimates[1];
    if (!currentPlan || !currentEstimate) throw new Error("two current native commits missing");
    expect(currentPlan.body.expectedSessionId).toBe(successor.sessionId);
    expect(currentEstimate.body.expectedSessionId).toBe(successor.sessionId);
    expect(currentPlan.body.requestId).not.toBe(oldPlan.body.requestId);
    expect(currentEstimate.body.requestId).not.toBe(oldEstimate.body.requestId);
    const beforeDelivery = timerDatabaseEffects(fixture.actor.userId, fixture.task.id);
    oldPlan.release();
    oldEstimate.release();
    await planningCompletion(page, oldPlan.body.requestId);
    await planningCompletion(page, oldEstimate.body.requestId);
    await expect(planner.getByRole("button", { name: "계획 저장", exact: true })).toBeDisabled();
    await expect(
      editor.getByRole("button", { name: "예상 시간 저장", exact: true }),
    ).toBeDisabled();
    await expect(planner.getByLabel("목표", { exact: true })).toHaveValue("새 세션의 현재 목표");
    await expect(editor.getByLabel("예상 시간(분)", { exact: true })).toHaveValue("60");
    await expect(editor.getByLabel("예상 시간 변경 사유", { exact: true })).toHaveValue(
      "새 세션의 현재 예상 요청",
    );
    await expect(planner.getByRole("status").filter({ hasText: "저장된 목표와 단계" })).toHaveCount(
      0,
    );
    expect(planRequests).toHaveLength(2);
    expect(estimates).toHaveLength(2);
    expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(beforeDelivery);
    const oldGoal = z.object({ taskId: z.string() }).parse(oldPlan.result);
    expect(
      diagnosticSql(`SELECT count(*) FROM fvoci.tasks WHERE parent_id='${oldGoal.taskId}'`),
    ).toBe("0");
    currentPlan.release();
    currentEstimate.release();
    await planningCompletion(page, currentPlan.body.requestId);
    await planningCompletion(page, currentEstimate.body.requestId);
    await expect(
      planner.getByRole("status").filter({ hasText: "저장된 목표와 단계" }),
    ).toContainText("4개 저장됨");
    await expect(widget.getByTestId("timer-estimate")).toHaveText("예상 60분");
    await expect(editor.getByRole("button", { name: "예상 시간 저장", exact: true })).toBeEnabled();
    expect(planRequests).toHaveLength(5);
    expect(
      planRequests.slice(1).every((request) => request.expectedSessionId === successor.sessionId),
    ).toBe(true);
    expect(estimates).toHaveLength(2);
    const currentGoal = z.object({ taskId: z.string() }).parse(currentPlan.result);
    expect(
      diagnosticSql(`SELECT count(*) FROM fvoci.tasks WHERE parent_id='${currentGoal.taskId}'`),
    ).toBe("3");
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE task_id='${fixture.task.id}' AND verb='task.estimate.minutes'`,
      ),
    ).toBe("2");
    await testInfo.attach("native-planner-estimate-session-retirement", {
      body: JSON.stringify({
        initial: fixture.actor,
        successor,
        oldRealJsonConsumedWhileNewPending: true,
        currentDraftAndPendingPreserved: true,
        oldGoalChildren: 0,
        currentGoalChildren: 3,
        planRequests: 5,
        estimateCommands: 2,
      }),
      contentType: "application/json",
    });
  } finally {
    for (const held of [...plans, ...estimates]) held.release();
    await successorContext.close();
  }
});

// Browser transport fault injection after a REAL native commit. This is not
// a claim that the native limiter emitted429 or that a real server outage503 occurred.
test("transient browser 429 and 503 deliveries preserve server anchors and replay one native outcome", async ({
  page,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TSTATUS", true);
  await page.goto(fixture.detail);
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  await expect(widget.getByTestId("timer-start")).toBeEnabled();
  await widget.getByTestId("timer-start").click();
  await expect(widget.getByTestId("timer-state")).toHaveText("측정 중");
  let operation: "pause" | "resume" = "pause";
  let browserStatus = 429;
  let release = () => {};
  let readDelivery = Promise.resolve();
  let attempts: unknown[] = [];
  let outcomes: unknown[] = [];
  const evidence: unknown[] = [];
  await page.route(
    (url) => url.pathname === fixture.timerUrl || url.pathname === "/api/v1/me/task-timer",
    async (route) => {
      if (route.request().method() === "GET") {
        const delivery = readDelivery;
        const native = await route.fetch();
        await delivery;
        if (!page.isClosed()) await route.fulfill({ response: native });
        return;
      }
      const body: unknown = route.request().postDataJSON();
      if (z.object({ operation: z.string() }).parse(body).operation !== operation)
        return route.continue();
      attempts.push(body);
      const native = await route.fetch();
      expect(native.status(), await native.text()).toBe(200);
      outcomes.push(await native.json());
      if (attempts.length === 1)
        await route.fulfill({
          status: browserStatus,
          contentType: "application/problem+json",
          body: JSON.stringify({
            type: "about:blank",
            title: "Temporary transport failure",
            status: browserStatus,
          }),
        });
      else await route.fulfill({ response: native });
    },
  );
  try {
    for (const phase of [
      { operation: "pause", status: 429, previous: "측정 중", next: "일시정지" },
      { operation: "resume", status: 503, previous: "일시정지", next: "측정 중" },
    ] as const) {
      operation = phase.operation;
      browserStatus = phase.status;
      attempts = [];
      outcomes = [];
      readDelivery = new Promise<void>((resolve) => {
        release = resolve;
      });
      const failedResponse = page.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === fixture.timerUrl &&
          response.request().method() === "POST" &&
          response.status() === phase.status,
      );
      await widget.getByTestId(`timer-${phase.operation}`).click();
      expect((await failedResponse).status()).toBe(phase.status);
      const retry = widget.getByRole("button", { name: "같은 요청 다시 보내기", exact: true });
      await expect(retry).toBeEnabled();
      await expect(widget.getByTestId("timer-state")).toHaveText(phase.previous);
      await expect(page).toHaveURL(new RegExp(`${fixture.detail}$`));
      expect(attempts).toHaveLength(1);
      const capture = z
        .object({
          requestId: z.string(),
          expectedActorId: z.string(),
          expectedSessionId: z.string(),
        })
        .parse(attempts[0]);
      expect(capture.expectedActorId).toBe(fixture.actor.userId);
      expect(capture.expectedSessionId).toBe(fixture.actor.sessionId);
      const committed = timerDatabaseEffects(fixture.actor.userId, fixture.task.id);
      const realReplay = page.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === fixture.timerUrl &&
          response.request().method() === "POST" &&
          response.status() === 200,
      );
      await retry.click();
      expect((await realReplay).status()).toBe(200);
      expect(attempts).toHaveLength(2);
      expect(attempts[1]).toEqual(attempts[0]);
      expect(outcomes).toHaveLength(2);
      expect(outcomes[1]).toEqual(outcomes[0]);
      expect(timerDatabaseEffects(fixture.actor.userId, fixture.task.id)).toBe(committed);
      expect(
        diagnosticSql(
          `SELECT count(*) FROM fvoci.task_timer_commands WHERE user_id='${fixture.actor.userId}' AND request_id='${capture.requestId}'`,
        ),
      ).toBe("1");
      expect(
        diagnosticSql(
          `SELECT count(*) FROM fvoci.task_timer_audit WHERE user_id='${fixture.actor.userId}' AND request_id='${capture.requestId}'`,
        ),
      ).toBe("1");
      release();
      await expect(widget.getByTestId("timer-state")).toHaveText(phase.next);
      await expect(retry).toHaveCount(0);
      evidence.push({
        browserTransportStatus: phase.status,
        operation: phase.operation,
        nativeStatuses: [200, 200],
        requestId: capture.requestId,
        replayBodyAndNativeOutcomeIdentical: true,
        committedEffectsUnchanged: true,
        retainedPreviousServerAnchor: true,
        canonicalRecovered: phase.next,
      });
    }
    await widget.getByTestId("timer-stop").click();
    await expect(widget.getByTestId("timer-start")).toBeEnabled();
    expect(
      taskShape.parse(
        await (
          await page.request.get(
            `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`,
          )
        ).json(),
      ).statusId,
    ).toBe(fixture.task.statusId);
    await testInfo.attach("native-commit-transient-browser-status-replay", {
      body: JSON.stringify({
        scope:
          "injected browser 429/503 deliveries after actual native200 commits, not native limiter/outage evidence",
        phases: evidence,
        stopLeavesTaskIncomplete: true,
      }),
      contentType: "application/json",
    });
  } finally {
    release();
  }
});

// Keep this last: it replaces only this isolated group's supervised native.
test("native same-database restart preserves paused and running anchors for genuine new clients", async ({
  page,
  browser,
  restartTimerServer,
}, testInfo) => {
  const restartShape = timerShape.extend({ serverNow: z.string().datetime({ offset: true }) });
  const effectiveElapsed = (view: z.infer<typeof restartShape>) =>
    view.run
      ? view.run.elapsedMilliseconds +
        (view.run.runningSince
          ? Math.max(0, Date.parse(view.serverNow) - Date.parse(view.run.runningSince))
          : 0)
      : 0;
  const fixture = await ordinaryTimerTask(page, "TRESTART", true);
  // Preparation takes ownership of the group's process without rewriting data.
  // Both measured restart exits below belong to our fixture-owned children.
  const initialOrigin = await restartTimerServer();
  await page.goto(`${initialOrigin}${fixture.detail}`);
  const widget = page.getByTestId(`task-stopwatch-${fixture.task.id}`);
  await expect(widget.getByTestId("timer-start")).toBeEnabled();
  await widget.getByTestId("timer-start").click();
  await expect(widget.getByTestId("timer-state")).toHaveText("측정 중");
  // Existing034 projections require a positive whole second; await the real
  // server anchor, without sleeping or weakening the two-entry assertion.
  await expect
    .poll(async () =>
      effectiveElapsed(
        restartShape.parse(
          await (await page.request.get(`${initialOrigin}${fixture.timerUrl}`)).json(),
        ),
      ),
    )
    .toBeGreaterThanOrEqual(1000);
  await widget.getByTestId("timer-pause").click();
  await expect(widget.getByTestId("timer-state")).toHaveText("일시정지");
  const paused = restartShape.parse(
    await (await page.request.get(`${initialOrigin}${fixture.timerUrl}`)).json(),
  );
  expect(paused.run?.status).toBe("paused");
  expect(paused.run?.elapsedMilliseconds).toBeGreaterThan(0);
  const pausedOrigin = await restartTimerServer();
  const firstContext = await browser.newContext({ baseURL: pausedOrigin });
  let resumed: z.infer<typeof restartShape> | undefined;
  let firstFreshIdentity: z.infer<typeof identityShape> | undefined;
  let pausedAfterRestart: z.infer<typeof restartShape> | undefined;
  try {
    const fresh = await firstContext.newPage();
    await login(fresh, fixture.email, credentials.password);
    const actor = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    firstFreshIdentity = actor;
    expect(actor.userId).toBe(fixture.actor.userId);
    expect(actor.sessionId).not.toBe(fixture.actor.sessionId);
    await fresh.goto(`/w/${fixture.slug}/my-tasks`);
    const current = fresh.getByTestId(`task-stopwatch-${fixture.task.id}`);
    await expect(current.getByTestId("timer-state")).toHaveText("일시정지");
    pausedAfterRestart = restartShape.parse(
      await (await fresh.request.get(fixture.timerUrl)).json(),
    );
    expect(pausedAfterRestart.run).toEqual(paused.run);
    await current.getByTestId("timer-resume").click();
    await expect(current.getByTestId("timer-state")).toHaveText("측정 중");
    resumed = restartShape.parse(await (await fresh.request.get(fixture.timerUrl)).json());
    expect(resumed.run?.id).toBe(paused.run?.id);
    expect(resumed.run?.runningSince).toBeTruthy();
    expect(resumed.run?.version).toBe((paused.run?.version ?? 0) + 1);
  } finally {
    await firstContext.close();
  }
  if (!resumed.run) throw new Error("actual resumed server anchor missing");
  const runningOrigin = await restartTimerServer();
  const secondContext = await browser.newContext({ baseURL: runningOrigin });
  try {
    const fresh = await secondContext.newPage();
    await login(fresh, fixture.email, credentials.password);
    const secondFreshIdentity = identityShape.parse(
      await (await fresh.request.get("/api/v1/auth/me")).json(),
    );
    expect(secondFreshIdentity.userId).toBe(fixture.actor.userId);
    expect(secondFreshIdentity.sessionId).not.toBe(fixture.actor.sessionId);
    expect(secondFreshIdentity.sessionId).not.toBe(firstFreshIdentity.sessionId);
    expect(firstFreshIdentity).toBeDefined();
    await fresh.goto(fixture.detail);
    const current = fresh.getByTestId(`task-stopwatch-${fixture.task.id}`);
    await expect(current.getByTestId("timer-state")).toHaveText("측정 중");
    const running = restartShape.parse(await (await fresh.request.get(fixture.timerUrl)).json());
    expect(running.run?.id).toBe(resumed.run.id);
    expect(running.run?.runningSince).toBe(resumed.run.runningSince);
    expect(running.run?.version).toBe(resumed.run.version);
    expect(running.run?.elapsedMilliseconds).toBe(resumed.run.elapsedMilliseconds);
    expect(effectiveElapsed(running)).toBeGreaterThan(effectiveElapsed(resumed));
    const displayedSeconds = (text: string) => {
      const parsed = z
        .string()
        .regex(/^\d+:\d{2}:\d{2}$/)
        .parse(text)
        .split(":")
        .map(Number);
      const [hours, minutes, seconds] = parsed;
      if (hours === undefined || minutes === undefined || seconds === undefined)
        throw new Error("genuine timer output missing hours/minutes/seconds");
      return hours * 3600 + minutes * 60 + seconds;
    };
    const freshDisplayBefore = await current.getByTestId("timer-elapsed").innerText();
    expect(displayedSeconds(freshDisplayBefore)).toBeGreaterThanOrEqual(
      Math.floor(resumed.run.elapsedMilliseconds / 1000),
    );
    await expect
      .poll(async () => displayedSeconds(await current.getByTestId("timer-elapsed").innerText()))
      .toBeGreaterThan(displayedSeconds(freshDisplayBefore));
    const freshDisplayAfter = await current.getByTestId("timer-elapsed").innerText();
    await expect
      .poll(async () =>
        effectiveElapsed(
          restartShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()),
        ),
      )
      .toBeGreaterThanOrEqual((paused.run?.elapsedMilliseconds ?? 0) + 1000);
    await current.getByTestId("timer-stop").click();
    await expect(current.getByTestId("timer-start")).toBeEnabled();
    const stopped = restartShape.parse(await (await fresh.request.get(fixture.timerUrl)).json());
    expect(stopped.run).toBeNull();
    expect(stopped.actualMilliseconds).toBeGreaterThan(effectiveElapsed(running));
    const task = taskShape.parse(
      await (
        await fresh.request.get(
          `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`,
        )
      ).json(),
    );
    expect(task.statusId).toBe(fixture.task.statusId);
    const referenceShape = z.object({
      id: z.string(),
      workspaceId: z.string(),
      taskId: z.string(),
      userId: z.string(),
    });
    const graph = z
      .object({
        runs: z.array(referenceShape.extend({ status: z.literal("stopped") })),
        segments: z.array(
          referenceShape.extend({
            runId: z.string(),
            timeEntryId: z.string(),
            endedAt: z.string(),
          }),
        ),
        entries: z.array(referenceShape.extend({ endedAt: z.string() })),
      })
      .parse(
        JSON.parse(
          diagnosticSql(`SELECT jsonb_build_object(
      'runs',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'workspaceId',workspace_id,'taskId',task_id,'userId',user_id,'status',status)),'[]'::jsonb) FROM fvoci.task_timer_runs),
      'segments',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'workspaceId',workspace_id,'taskId',task_id,'userId',user_id,'runId',run_id,'timeEntryId',time_entry_id,'endedAt',ended_at)),'[]'::jsonb) FROM fvoci.task_timer_segments),
      'entries',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'workspaceId',workspace_id,'taskId',task_id,'userId',user_id,'endedAt',ended_at)),'[]'::jsonb) FROM fvoci.time_entries))`),
        ),
      );
    const rawGraph: unknown = JSON.parse(
      diagnosticSql(`SELECT jsonb_build_object(
      'runs',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM fvoci.task_timer_runs t),
      'segments',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM fvoci.task_timer_segments t),
      'entries',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM fvoci.time_entries t))`),
    );
    // Keep actual source data before the graph oracles, including on failure.
    await testInfo.attach("native-restart-actual-responses-and-full-graph", {
      body: JSON.stringify({
        paused,
        pausedAfterRestart,
        resumed,
        running,
        stopped,
        task,
        rawGraph,
        freshDisplayBefore,
        freshDisplayAfter,
        firstFreshIdentity,
        secondFreshIdentity,
      }),
      contentType: "application/json",
    });
    expect(graph.runs).toHaveLength(1);
    expect(graph.segments).toHaveLength(2);
    expect(graph.entries).toHaveLength(2);
    for (const row of [...graph.runs, ...graph.segments, ...graph.entries]) {
      expect(row.workspaceId).toBe(fixture.workspaceId);
      expect(row.taskId).toBe(fixture.task.id);
      expect(row.userId).toBe(fixture.actor.userId);
    }
    for (const segment of graph.segments) expect(segment.runId).toBe(resumed.run.id);
    expect(graph.runs.map((run) => run.id)).toEqual([resumed.run.id]);
    expect(graph.segments.map((segment) => segment.timeEntryId).sort()).toEqual(
      graph.entries.map((entry) => entry.id).sort(),
    );
    await testInfo.attach("native-same-db-persisted-run-new-clients", {
      body: JSON.stringify({
        initialOrigin,
        pausedOrigin,
        runningOrigin,
        runId: resumed.run.id,
        pausedElapsed: paused.run?.elapsedMilliseconds,
        samePausedElapsedAfterRestart: true,
        sameResumedAnchorAfterRestart: true,
        storedClosedElapsedAfterRestart: running.run?.elapsedMilliseconds,
        effectiveServerElapsedAfterRestart: effectiveElapsed(running),
        firstFreshIdentity,
        secondFreshIdentity,
        allGraphRowsClosedAndReferencesExact: true,
        actualResponses: { paused, pausedAfterRestart, resumed, running, stopped, task },
        rawGraph,
        freshDisplayBefore,
        freshDisplayAfter,
        finalActual: stopped.actualMilliseconds,
        actualFreshLogins: 2,
        measuredPhysicalRestarts: 2,
        taskStatusUnchanged: true,
        runCount: 1,
        segmentAndClosedEntryCount: 2,
      }),
      contentType: "application/json",
    });
  } finally {
    await secondContext.close();
  }
});

type JointTimerEstimateState = {
  timerRows: string;
  estimate: string | null;
  unit: string | null;
  identity: unknown;
};
function assertJointTimerEstimatePreserved(
  before: JointTimerEstimateState,
  after: JointTimerEstimateState,
) {
  expect(after.timerRows, "restore/refused estimate must preserve all six raw timer tables").toBe(
    before.timerRows,
  );
  expect(after.estimate).toBe("15");
  expect(after.unit).toBe("minutes");
  expect(after.identity, "same planned task/project/document origin").toEqual(before.identity);
}

type JointCollabReceipt = {
  seq: string;
  op_id: string;
  actor_user_id: string;
  payload_len: number;
  payload_sha256: string;
};
function assertJointRestoreAppend(
  before: { tail: string; receipts: JointCollabReceipt[] },
  after: { tail: string; receipts: JointCollabReceipt[] },
  actorId: string,
) {
  expect(BigInt(after.tail)).toBe(BigInt(before.tail) + 1n);
  // Persist compacts update rows, but durable operation receipts survive it.
  expect(after.receipts).toHaveLength(before.receipts.length + 1);
  expect(after.receipts.slice(0, -1)).toEqual(before.receipts);
  const receipt = after.receipts.at(-1);
  expect(receipt).toBeDefined();
  if (!receipt) throw new Error("restore forward receipt missing");
  expect(receipt.seq).toBe(after.tail);
  expect(receipt.actor_user_id).toBe(actorId);
  expect(before.receipts.some((old) => old.op_id === receipt.op_id)).toBe(false);
  expect(receipt.payload_len).toBeGreaterThan(0);
  expect(receipt.payload_sha256).toMatch(/^\\x[0-9a-f]{64}$/);
}

test("one ordinary task restore preserves paused timer and explicit estimate while its stale estimate intent conflicts", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "JOINT", true);
  const notesResponse = await page.request.post(
    `/api/v1/workspaces/${fixture.workspaceId}/documents`,
    { data: { parentId: null, title: "공동 복원·측정 연구 노트" } },
  );
  expect(notesResponse.status(), await notesResponse.text()).toBe(201);
  const notes = z
    .object({ id: z.string().uuid(), number: z.number() })
    .parse(await notesResponse.json());
  const project = z
    .object({ projectId: z.string().uuid() })
    .parse(
      await (
        await page.request.get(`/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`)
      ).json(),
    );
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  const planner = page.getByTestId("study-plan-builder");
  await planner.getByText("학습·연구·업무 계획 만들기", { exact: true }).click();
  await planner.getByLabel("목표", { exact: true }).fill("복원과 측정을 같은 연구 목표에서 확인");
  await planner.getByLabel("목표 예상 시간(분, 선택)", { exact: true }).fill("15");
  for (const label of ["목표·연구 노트 문서 링크", "읽을 자료 문서 링크"])
    await planner.getByLabel(label, { exact: true }).fill(`WIKI-${String(notes.number)}`);
  await planner.getByRole("button", { name: "연결할 문서 확인", exact: true }).click();
  await expect(
    planner.getByLabel("저장할 프로젝트", { exact: true }).locator("option", { hasText: "JOINT" }),
  ).toHaveCount(1);
  await planner.getByLabel("저장할 프로젝트", { exact: true }).selectOption(project.projectId);
  const createdGoal = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().endsWith(`/documents/${notes.id}/study-plan/task`) &&
      response.status() === 200,
  );
  await planner.getByRole("button", { name: "계획 저장", exact: true }).click();
  const goal = z
    .object({ taskId: z.string().uuid(), number: z.number(), projectKey: z.string() })
    .parse(await (await createdGoal).json());
  await expect(planner.getByRole("status").filter({ hasText: "저장된 목표와 단계" })).toContainText(
    "4개 저장됨",
  );
  const base = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${goal.taskId}`;
  const detail = `/w/${fixture.slug}/${goal.projectKey}-${String(goal.number)}`;
  const timerUrl = base + "/timer";
  const estimateShape = z.object({
    value: z.string().nullable(),
    unit: z.string().nullable(),
    updatedAt: z.string(),
  });
  const readState = () => ({
    timerRows: timerDatabaseEffectsForRestart(),
    ...z
      .object({
        estimate: z.string().nullable(),
        unit: z.string().nullable(),
        updatedAt: z.string(),
        tail: z.string(),
        updates: z.number(),
        receipts: z.array(
          z
            .object({
              seq: z.string(),
              op_id: z.string().uuid(),
              actor_user_id: z.string().uuid(),
              payload_len: z.number(),
              payload_sha256: z.string(),
            })
            .passthrough(),
        ),
        identity: z.object({
          task: z.string().uuid(),
          project: z.string().uuid(),
          origins: z.array(
            z.object({ document_id: z.string().uuid(), task_id: z.string().uuid() }).passthrough(),
          ),
        }),
      })
      .parse(
        JSON.parse(
          diagnosticSql(
            `SELECT jsonb_build_object('estimate',t.estimate::text,'unit',t.estimate_unit,'updatedAt',to_char(t.updated_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"+00:00"'),'tail',(SELECT tail_seq::text FROM fvoci.task_states WHERE workspace_id=t.workspace_id AND task_id=t.id),'updates',(SELECT count(*) FROM fvoci.task_collab_updates WHERE workspace_id=t.workspace_id AND task_id=t.id),'receipts',(SELECT coalesce(jsonb_agg(to_jsonb(r)||jsonb_build_object('seq',r.seq::text) ORDER BY r.seq,r.op_id),'[]'::jsonb) FROM fvoci.task_collab_op_receipts r WHERE r.workspace_id=t.workspace_id AND r.task_id=t.id),'identity',jsonb_build_object('task',t.id,'project',t.project_id,'origins',(SELECT jsonb_agg(to_jsonb(o) ORDER BY o.workspace_id,o.task_id) FROM fvoci.task_origins o WHERE o.workspace_id=t.workspace_id AND o.task_id=t.id))) FROM fvoci.tasks t WHERE t.workspace_id='${fixture.workspaceId}' AND t.id='${goal.taskId}'`,
          ),
        ),
      ),
  });
  const history = async () => {
    const response = await page.request.get(base + "/revisions");
    expect(response.ok()).toBe(true);
    return z
      .object({ items: z.array(z.object({ id: z.string().uuid() }).passthrough()) })
      .parse(await response.json()).items;
  };
  const body = async () => {
    const response = await page.request.get(base);
    expect(response.ok()).toBe(true);
    return z.object({ contentJson: z.unknown() }).parse(await response.json()).contentJson;
  };
  await page.goto(detail);
  const widget = page.getByTestId(`task-stopwatch-${goal.taskId}`);
  await expect(widget.getByTestId("timer-estimate")).toHaveText("예상 15분");
  await widget.getByTestId("timer-start").click();
  await expect(widget.getByTestId("timer-state")).toHaveText("측정 중");
  await widget.getByTestId("timer-pause").click();
  await expect(widget.getByTestId("timer-state")).toHaveText("일시정지");
  const paused = timerShape.parse(await (await page.request.get(timerUrl)).json());
  expect(paused.run?.status).toBe("paused");
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.type("공동 연구 원본 본문");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible();
  await page.getByTestId("revision-history").click();
  const saved = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" && response.url().endsWith(base + "/revisions"),
  );
  await page.getByTestId("revision-save").click();
  const savedResponse = await saved;
  expect(savedResponse.status()).toBe(201);
  const source = z.object({ id: z.string().uuid() }).parse(await savedResponse.json());
  const sourceDetailResponse = await page.request.get(base + "/revisions/" + source.id);
  expect(sourceDetailResponse.ok()).toBe(true);
  const sourceDetail: unknown = await sourceDetailResponse.json();
  const sourceBody = z.object({ contentJson: z.unknown() }).parse(sourceDetail).contentJson;
  await page.getByTestId("revision-history").click();
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.type("공동 연구 현재 본문");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible();
  await page.getByTestId("revision-history").click();
  const sourceIndex = (await history()).findIndex((item) => item.id === source.id);
  expect(sourceIndex).toBeGreaterThanOrEqual(0);
  let restoreButton = page
    .getByTestId("revision-item")
    .nth(sourceIndex)
    .getByTestId("revision-restore");
  const beforeDraftBody = await body();
  const beforeDraftHistory = await history();
  const draftRequests: string[] = [];
  page.on("request", (request) => {
    const pathname = new URL(request.url()).pathname;
    if (
      (request.method() === "POST" && pathname === base + "/revisions") ||
      (pathname.startsWith(base + "/revisions/") && /restore(?:-preview)?$/.test(pathname))
    )
      draftRequests.push(request.url());
  });
  await page.locator('[data-editor-mode="markdown"]').click();
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.focus();
  await page.keyboard.press("ControlOrMeta+a");
  const cdp = await page.context().newCDPSession(page);
  try {
    await cdp.send("Input.imeSetComposition", { text: "ㅎ", selectionStart: 1, selectionEnd: 1 });
    await expect(field).toHaveValue("ㅎ");
    await expect(restoreButton).toBeDisabled();
    await expect(page.getByTestId("revision-save")).toBeDisabled();
    expect(draftRequests).toHaveLength(0);
    expect(await body()).toEqual(beforeDraftBody);
    expect(await history()).toEqual(beforeDraftHistory);
    const dirtyDraft = "공동 연구 현재 본문 · 보류 중인 초안";
    await cdp.send("Input.insertText", { text: dirtyDraft });
    await expect(field).toHaveValue(dirtyDraft);
    await expect(restoreButton).toBeDisabled();
    await expect(page.getByTestId("revision-save")).toBeDisabled();
    expect(draftRequests).toHaveLength(0);
    expect(await body()).toEqual(beforeDraftBody);
    expect(await history()).toEqual(beforeDraftHistory);
    await page.getByRole("button", { name: "적용", exact: true }).click();
  } finally {
    await cdp.detach();
  }
  await page.getByTestId("revision-history").click();
  await page.locator('[data-editor-mode="rich"]').click();
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible();
  await page.getByTestId("revision-history").click();
  restoreButton = page
    .getByTestId("revision-item")
    .nth((await history()).findIndex((item) => item.id === source.id))
    .getByTestId("revision-restore");
  await restoreButton.click();
  await expect(page.getByTestId("revision-restore-current")).toContainText("현재 본문");
  await expect(page.getByTestId("revision-restore-source")).toContainText("원본 본문");
  const peerContext = await browser.newContext({
    baseURL: new URL(page.url()).origin,
    storageState: await page.context().storageState(),
  });
  try {
    const peer = await peerContext.newPage();
    await peer.goto(detail);
    await expect(peer.locator('[data-collab-status="connected"]')).toBeVisible();
    await peer.locator(".fvoci-editor .ProseMirror").click();
    await peer.keyboard.press("ControlOrMeta+a");
    await peer.keyboard.type("공동 연구 동료 최신 본문");
    await peer.getByRole("button", { name: "저장", exact: true }).click();
    await expect(peer.locator('[data-collab-persisted="true"]')).toBeVisible();
    await expect(editor).toContainText("동료 최신 본문");
    const beforeConflict = readState();
    const beforeConflictHistory = await history();
    const beforeConflictBody = await body();
    const conflict = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        response.url().endsWith(`/revisions/${source.id}/restore`),
    );
    await page.getByTestId("revision-restore-confirm").click();
    const conflictResponse = await conflict;
    expect(conflictResponse.status()).toBe(409);
    z.object({ code: z.literal("revision_restore_conflict") }).parse(await conflictResponse.json());
    const afterConflict = readState();
    assertJointTimerEstimatePreserved(beforeConflict, afterConflict);
    expect(afterConflict.tail).toBe(beforeConflict.tail);
    expect(afterConflict.updates).toBe(beforeConflict.updates);
    expect(await history()).toEqual(beforeConflictHistory);
    expect(await body()).toEqual(beforeConflictBody);
    await page.getByTestId("revision-restore-refresh").click();
    await expect(page.getByTestId("revision-restore-current")).toContainText("동료 최신 본문");
    await page.getByTestId("revision-restore-cancel").click();
  } finally {
    await peerContext.close();
  }
  // Capture E0 after the peer commit. The restore alone must retire its exact
  // microsecond timestamp; no-op persist must not manufacture that conflict.
  await page.reload();
  await expect(widget.getByTestId("timer-state")).toHaveText("일시정지");
  const current = await page.request.get(timerUrl);
  expect(current.ok()).toBe(true);
  const e0 = z.object({ estimate: estimateShape }).parse(await current.json()).estimate;
  const beforeRestore = readState();
  expect(e0.value).toBe("15");
  expect(e0.unit).toBe("minutes");
  expect(e0.updatedAt).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|\+00:00)$/);
  expect(
    diagnosticSql(
      `SELECT (updated_at='${e0.updatedAt}'::timestamptz)::text FROM fvoci.tasks WHERE workspace_id='${fixture.workspaceId}' AND id='${goal.taskId}'`,
    ),
    "E0 exact microsecond timestamp matches real task row",
  ).toBe("true");
  expect(beforeRestore.identity).toMatchObject({
    task: goal.taskId,
    project: project.projectId,
    origins: [{ document_id: notes.id, task_id: goal.taskId }],
  });
  const estimateEditor = widget.getByTestId("task-estimate-editor");
  await estimateEditor.getByText("예상 시간 설정", { exact: true }).click();
  await estimateEditor.getByLabel("예상 시간(분)", { exact: true }).fill("25");
  await estimateEditor.getByLabel("예상 시간 변경 사유", { exact: true }).fill("복원 전 예상 의도");
  let release = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const commandShape = z.object({
    requestId: z.string().uuid(),
    expectedActorId: z.string().uuid(),
    expectedSessionId: z.string().uuid(),
    expected: estimateShape,
    minutes: z.literal(25),
    reason: z.literal("복원 전 예상 의도"),
  });
  let heldBody: z.infer<typeof commandShape> | undefined;
  const estimateRoute = (url: URL) => url.pathname === timerUrl + "/estimate";
  let heldContinuation: Promise<void> | undefined;
  let primaryFailure: { error: unknown } | undefined;
  const cleanupFailures: unknown[] = [];
  await page.route(estimateRoute, async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    heldContinuation = (async () => {
      heldBody = commandShape.parse(route.request().postDataJSON());
      await gate;
      await route.continue();
    })();
    await heldContinuation;
  });
  try {
    await estimateEditor.getByRole("button", { name: "예상 시간 저장", exact: true }).click();
    await expect.poll(() => heldBody !== undefined).toBe(true);
    if (!heldBody) throw new Error("real mounted estimate intent missing");
    expect(heldBody.expected).toEqual(e0);
    expect(heldBody.expectedActorId).toBe(fixture.actor.userId);
    expect(heldBody.expectedSessionId).toBe(fixture.actor.sessionId);
    await expect(
      estimateEditor.getByRole("button", { name: "예상 시간 저장", exact: true }),
    ).toBeDisabled();
    await page.getByTestId("revision-history").click();
    const revisionsBefore = await history();
    await page
      .getByTestId("revision-item")
      .nth(revisionsBefore.findIndex((item) => item.id === source.id))
      .getByTestId("revision-restore")
      .click();
    await expect(page.getByTestId("revision-restore-preview")).toHaveAttribute(
      "data-source-revision",
      source.id,
    );
    expect(readState().updatedAt, "preview's no-op persist keeps exact E0").toBe(
      beforeRestore.updatedAt,
    );
    const restoredResponse = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        response.url().endsWith(`/revisions/${source.id}/restore`),
    );
    await page.getByTestId("revision-restore-confirm").click();
    const restoredReply = await restoredResponse;
    expect(restoredReply.status()).toBe(200);
    const restored = z.object({ revisionId: z.string().uuid() }).parse(await restoredReply.json());
    expect(restored.revisionId).not.toBe(source.id);
    await expect(editor).toContainText("원본 본문");
    expect(await body()).toEqual(sourceBody);
    const afterRestore = readState();
    assertJointTimerEstimatePreserved(beforeRestore, afterRestore);
    expect(afterRestore.updatedAt).not.toBe(beforeRestore.updatedAt);
    assertJointRestoreAppend(beforeRestore, afterRestore, fixture.actor.userId);
    const restoreDetail = await page.request.get(base + "/revisions/" + restored.revisionId);
    expect(restoreDetail.ok()).toBe(true);
    z.object({
      reason: z.literal("restore"),
      restoredFromId: z.literal(source.id),
      contentJson: z.unknown(),
    }).parse(await restoreDetail.json());
    expect(await (await page.request.get(base + "/revisions/" + source.id)).json()).toEqual(
      sourceDetail,
    );
    expect((await history()).filter((item) => item.id === restored.revisionId)).toHaveLength(1);
    const staleReply = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" && response.url().endsWith(timerUrl + "/estimate"),
    );
    release();
    const stale = await staleReply;
    expect(stale.status()).toBe(409);
    z.object({ params: z.object({ code: z.literal("estimate_changed") }) }).parse(
      await stale.json(),
    );
    await expect(
      estimateEditor.getByRole("button", { name: "예상 시간 저장", exact: true }),
    ).toBeEnabled();
    await expect(estimateEditor.getByLabel("예상 시간(분)", { exact: true })).toHaveValue("25");
    await expect(estimateEditor.getByLabel("예상 시간 변경 사유", { exact: true })).toHaveValue(
      "복원 전 예상 의도",
    );
    await expect(estimateEditor).toContainText("예상 시간이 다른 곳에서 변경되었습니다");
    const afterStale = readState();
    assertJointTimerEstimatePreserved(beforeRestore, afterStale);
    expect(afterStale.updatedAt).toBe(afterRestore.updatedAt);
    const freshContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
    try {
      const fresh = await freshContext.newPage();
      await login(fresh, fixture.email, credentials.password);
      const freshActor = identityShape.parse(
        await (await fresh.request.get("/api/v1/auth/me")).json(),
      );
      expect(freshActor.userId).toBe(fixture.actor.userId);
      expect(freshActor.sessionId).not.toBe(fixture.actor.sessionId);
      await fresh.goto(detail);
      await expect(fresh.locator(".fvoci-editor .ProseMirror")).toContainText("원본 본문");
      const freshWidget = fresh.getByTestId(`task-stopwatch-${goal.taskId}`);
      await expect(freshWidget.getByTestId("timer-state")).toHaveText("일시정지");
      await expect(freshWidget.getByTestId("timer-estimate")).toHaveText("예상 15분");
      expect(timerShape.parse(await (await fresh.request.get(timerUrl)).json()).run).toEqual(
        paused.run,
      );
      const freshBody = await fresh.request.get(base);
      expect(
        z.object({ contentJson: z.unknown() }).parse(await freshBody.json()).contentJson,
      ).toEqual(sourceBody);
      const freshHistory = await fresh.request.get(base + "/revisions");
      expect(
        z
          .object({ items: z.array(z.object({ id: z.string() })) })
          .parse(await freshHistory.json())
          .items.some((item) => item.id === restored.revisionId),
      ).toBe(true);
      assertJointTimerEstimatePreserved(beforeRestore, readState());
    } finally {
      await freshContext.close();
    }
    await testInfo.attach("same-task-restore-timer-estimate", {
      body: JSON.stringify({
        taskId: goal.taskId,
        source: source.id,
        restored: restored.revisionId,
        pausedRun: paused.run,
        expected: e0,
        afterRestoreUpdatedAt: afterRestore.updatedAt,
        estimateRequestId: heldBody.requestId,
        before: beforeRestore,
        afterRestore,
        afterStale,
        staleStatus: stale.status(),
      }),
      contentType: "application/json",
    });
  } catch (error) {
    primaryFailure = { error };
  } finally {
    release();
    try {
      await heldContinuation;
    } catch (error) {
      cleanupFailures.push(error);
    }
    try {
      await page.unroute(estimateRoute);
    } catch (error) {
      cleanupFailures.push(error);
    }
    if (cleanupFailures.length) {
      testInfo.annotations.push({
        type: "estimate-route-cleanup-error",
        description: cleanupFailures
          .map((error) =>
            error instanceof Error ? `${error.name}: ${error.message}` : String(error),
          )
          .join("\n"),
      });
    }
  }
  if (primaryFailure) throw primaryFailure.error;
  if (cleanupFailures.length)
    throw new AggregateError(cleanupFailures, "held estimate route cleanup failed");
});
