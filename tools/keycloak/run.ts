// Real Keycloak consumer. Build/source/browser admission is owned by the
// maintained recorder; process ownership is owned by UiProcesses.
import { deepEquals } from "bun";
import { randomBytes } from "node:crypto";
import {
  appendFileSync,
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { join, resolve } from "node:path";
import process from "node:process";
import { databaseUrls } from "../web-e2e/database-urls.ts";
import { below, root, sha } from "../selected-backend-ci/io.ts";
import { localKeycloakAllocation } from "../selected-backend-ci/admission.ts";
import { pgImage } from "../selected-backend-ci/drivers/postgres.ts";
import { assertNoEnvFile } from "../selected-backend-ci/drivers/common.ts";
import { cleanEnv, bunIsolation } from "../turso/ui-common.ts";
import {
  communicate,
  now,
  pause,
  poll,
  procIdentity,
  UiProcesses,
  withProcesses,
  type Child,
} from "../turso/ui-processes.ts";
import {
  currentBuild,
  freeze,
  loadLocalLease,
  recordBefore,
  type LocalLease,
  type Manifest,
} from "../turso/ui-record.ts";
import { awaitListening, portClosed, requireSetup } from "../turso/ui-server.ts";
import { events, ready, verify } from "./admin.ts";
import {
  CLIENT_ID,
  LABEL,
  REALM,
  USERS,
  SECRET_ENV,
  groupSummary,
  realmValues,
  registerEnvSecrets,
  renderTemplate,
  requirePassedCargoGroup,
  requirePassedGroup,
  specConfig,
  ssoConfig,
  ssoRealmValues,
} from "./realm.ts";
import {
  HelperError,
  addSecret,
  errorName,
  knownSecrets,
  redactLine,
  selftestFailures,
} from "./redact.ts";
import { startLocalLibsql } from "./local-libsql.ts";
import { digest, systemSudo, type WitnessInput } from "./process-witness.ts";
import {
  validatePreparedInstance,
  validatePreparedActors,
  type PreparedActors,
  type PreparedBinding,
  type PreparedInstance,
} from "../../apps/web/e2e-keycloak/config.ts";
import { assertPreparedDelta, type PreparedDelta, type PreparedSnapshot } from "../../apps/web/e2e-keycloak/observer.ts";

export const MODES = ["flows", "failures", "wrong-secret"] as const;
export type Backend = "postgres" | "sqlite" | "libsql-remote";
export const lease: { load: LocalLease } = { load: localKeycloakAllocation };
class RunInterrupted extends HelperError {
  constructor(readonly exitCode: number) {
    super("Keycloak run interrupted");
  }
}

const privateWrite = (path: string, text: string) => {
  writeFileSync(path, text, { mode: 0o600, flag: "wx" });
};
const require = (condition: unknown, message: string): void => {
  if (!condition) throw new HelperError(message);
};
export function safeText(text: string): string {
  return text
    .split(/(?<=\n)/)
    .map((line) => redactLine(line, knownSecrets()))
    .join("")
    .replace(/(?:https?|libsql):\/\/[^\s"'\\]+/g, "<redacted-url>")
    .replace(/\b(?:localhost|127\.0\.0\.1)(?::[0-9]+)?\b/g, "<redacted-host>");
}
const emit = (text: string) => {
  try {
    writeSync(2, safeText(text) + "\n");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EPIPE") throw error;
    process.emit("SIGPIPE");
  }
};

export interface CommandOptions {
  env?: Record<string, string>;
  cwd?: string;
  input?: string;
  signal?: AbortSignal;
  seconds?: number;
  output?: (stdout: string, stderr: string) => void;
}
/** One owned child, output drained before retirement; failures never quote argv. */
export async function runCommand(scope: UiProcesses, args: string[], options: CommandOptions = {}) {
  options.signal?.throwIfAborted();
  const child = scope.spawn(args, "keycloak-command", {
    cwd: options.cwd ?? root,
    env: options.env ?? cleanEnv(),
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  const interrupted = () => {
    scope.snapshot();
    const allocation = scope.allocations.findIndex((a) => a.process === child);
    for (const entry of scope.entries.values())
      if (entry.allocation === allocation) scope.send(entry, 15);
  };
  options.signal?.addEventListener("abort", interrupted, { once: true });
  const stderr =
    child.stderr instanceof ReadableStream
      ? new Response(child.stderr).text()
      : Promise.resolve("");
  try {
    const stdout = await communicate(child, options.input ?? "", options.seconds ?? 240);
    const errors = await stderr;
    const code = await scope.finish(child);
    const text = new TextDecoder().decode(stdout);
    options.output?.(text, errors);
    options.signal?.throwIfAborted();
    return { code, stdout: text, stderr: errors };
  } finally {
    options.signal?.removeEventListener("abort", interrupted);
    if (poll(child) === null) await scope.finish(child, true);
    await stderr;
  }
}

/** This is the runner's verdict boundary, also exercised by reporter fixtures. */
export function groupVerdict(
  directory: string,
  mode: string,
  code: number,
  output: (line: string) => void = emit,
): number {
  let report: unknown, log: string | undefined;
  try {
    report = JSON.parse(readFileSync(join(directory, `playwright-${mode}.json`), "utf8"));
  } catch {
    /* missing is failure */
  }
  try {
    log = readFileSync(join(directory, `${mode}.log`), "utf8");
  } catch {
    /* missing is failure */
  }
  if (code === 0) {
    try {
      if (mode === "workspace-sso")
        requirePassedCargoGroup(log ?? "", "keycloak_workspace_sso_with_a_test_entitlement");
      else requirePassedGroup(report, mode);
    } catch {
      code = 1;
    }
  }
  output(`group ${mode}: exit ${String(code)} (${groupSummary(report, log)})`);
  return code === 0 ? 0 : 1;
}

export interface ProcessObserver {
  directory: string;
  bun: { path: string; sha256: string };
  module: { path: string; sha256: string };
  config: { path: string; sha256: string };
}
export async function assertServerEnvironment(
  scope: UiProcesses,
  pid: number,
  app: Record<string, string>,
  manifest: Manifest,
  observer: ProcessObserver,
  negatives: boolean,
) {
  require(realpathSync(observer.directory) ===
    observer.directory, "privileged observer directory is not physical");
  const directory = lstatSync(observer.directory);
  require(directory.isDirectory() &&
    directory.uid === 0 &&
    directory.gid === 0 &&
    (directory.mode & 0o022) === 0, "privileged observer directory is not root owned");
  for (const [name, reference] of [
    ["bun", observer.bun],
    ["process-witness.ts", observer.module],
    ["bunfig.toml", observer.config],
  ] as const) {
    const file = lstatSync(reference.path);
    require(reference.path === join(observer.directory, name) &&
      file.isFile() &&
      !file.isSymbolicLink() &&
      file.uid === 0 &&
      file.gid === 0 &&
      (file.mode & 0o022) === 0, "privileged observer input is not immutable to the worker");
  }
  require(observer.module.sha256 === sha(join(root, "tools/keycloak/process-witness.ts")) &&
    sha(observer.module.path) === observer.module.sha256 &&
    sha(observer.bun.path) === observer.bun.sha256 &&
    sha(observer.config.path) === observer.config.sha256 &&
    observer.bun.sha256 === manifest.bun.sha256 &&
    observer.config.sha256 === digest(""), "reviewed privileged observer inputs changed");
  const parent = procIdentity(process.pid),
    server = procIdentity(pid);
  require(server.parentPid === parent.pid, "normal server is not the consumer child");
  let unprivilegedEnvironmentDenied = false;
  try {
    readFileSync(`/proc/${String(pid)}/environ`);
  } catch (error) {
    if (["EACCES", "EPERM"].includes(String((error as NodeJS.ErrnoException).code)))
      unprivilegedEnvironmentDenied = true;
    else throw error;
  }
  require(unprivilegedEnvironmentDenied, "normal server environment is readable by the unprivileged actor");
  const input: WitnessInput = {
    pid,
    startTicks: server.startTicks,
    parentPid: parent.pid,
    parentStartTicks: parent.startTicks,
    executableSha256: manifest.binaries["fvoci-server"]?.sha256 ?? "",
    environmentDigests: Object.fromEntries(
      Object.entries({
        DATABASE_APP_URL: app.DATABASE_APP_URL,
        FVOCI_DATABASE_BACKEND: app.FVOCI_DATABASE_BACKEND,
        FVOCI_SQLITE_PATH: app.FVOCI_SQLITE_PATH,
        FVOCI_LIBSQL_URL: app.FVOCI_LIBSQL_URL,
        FVOCI_LIBSQL_AUTH_TOKEN: app.FVOCI_LIBSQL_AUTH_TOKEN,
        SSL_CERT_FILE: app.SSL_CERT_FILE,
        OIDC_GENERIC_ISSUER: app.OIDC_GENERIC_ISSUER,
        OIDC_GENERIC_CLIENT_ID: app.OIDC_GENERIC_CLIENT_ID,
        OIDC_GENERIC_CLIENT_SECRET: app.OIDC_GENERIC_CLIENT_SECRET,
      }).flatMap(([key, value]) => (value === undefined ? [] : [[key, digest(value)]])),
    ),
  };
  const sudo = systemSudo();
  const argv = [
    sudo,
    "-n",
    "/usr/bin/env",
    "-i",
    "-C",
    observer.directory,
    observer.bun.path,
    "--no-env-file",
    "--config=" + observer.config.path,
    observer.module.path,
  ];
  const observed = await runCommand(scope, argv, { input: JSON.stringify(input), seconds: 30 });
  require(observed.code === 0, "actual protected server environment witness failed");
  const facts = JSON.parse(observed.stdout) as Record<string, unknown>;
  require(Object.keys(facts).length === 7 &&
    facts.accepted === true &&
    Object.values(facts).every(
      (value) => value === true,
    ), "actual protected server environment facts failed");
  if (negatives)
    for (const refused of [
      { ...input, startTicks: "0" },
      { ...input, parentStartTicks: "0" },
      { ...input, executableSha256: "0".repeat(64) },
      {
        ...input,
        environmentDigests: {
          ...input.environmentDigests,
          OIDC_GENERIC_CLIENT_SECRET: "0".repeat(64),
        },
      },
      { ...input, path: "arbitrary-path-refused" },
    ]) {
      const rejected = await runCommand(scope, argv, {
        input: JSON.stringify(refused),
        seconds: 30,
      });
      require(rejected.code !== 0 &&
        (JSON.parse(rejected.stdout) as { accepted: unknown }).accepted ===
          false, "privileged process observer negative control was accepted");
    }
  return { ...facts, unprivilegedEnvironmentDenied, negativeControls: negatives ? 5 : 0 };
}

export interface PreparedBackend {
  backend: Backend;
  environment: Record<string, string>;
  observer: Record<string, string>;
  close(): Promise<void>;
  /** Exact current admitted target/group, never a credential or grant. */
  prepared?: PreparedBinding;
}
export interface PreparedPrimary {
  source: string;
  tree: string;
  target: string;
  groups: Record<(typeof MODES)[number], PreparedActors>;
  /** The admitted caller binds native reads to its existing target and full original baseline. */
  observe(binding: PreparedBinding): Promise<PreparedSnapshot>;
  /** Reuse the maintained serial native whole-primary/horizon auditor, with unfiltered inputs. */
  assertWholePrimary(before: PreparedSnapshot, after: PreparedSnapshot, delta: PreparedDelta): Promise<void>;
}
export interface ConsumerInputs {
  manifest: Manifest;
  directory: string;
  evidence?: string;
  signal?: AbortSignal;
  // The admitted producer owns backend allocation and TLS/target binding.
  prepare(mode: string, directory: string, binding?: PreparedBinding): Promise<PreparedBackend>;
  checkInputs(): void;
  heavy?: string;
  processObserver: ProcessObserver;
  workspaceSso?: { binary: string; environment: Record<string, string> };
  /** In-memory caller contract only: deliberately absent from CLI/config loaders. */
  prepared?: PreparedPrimary;
}

export function validatePreparedInputs(inputs: ConsumerInputs): void {
  const p = inputs.prepared;
  if (p === undefined) return;
  require(p.source === inputs.manifest.sourceInputs.source &&
    p.tree === inputs.manifest.sourceInputs.tree && typeof p.target === "string" && p.target.length > 0 &&
    typeof p.observe === "function" && typeof p.assertWholePrimary === "function" &&
    inputs.workspaceSso === undefined, "Keycloak prepared source or target contract differs");
  const identities = new Set<string>();
  for (const mode of MODES) {
    const actors = p.groups?.[mode];
    require(actors !== undefined, "Keycloak prepared group allocation missing");
    validatePreparedActors(actors);
    for (const identity of [actors.owner.id, actors.workspace.id, actors.workspace.slug,
      actors.owner.email, actors.member.email, actors.mismatchEmail, actors.crossOriginEmail,
      ...Object.values(actors.users).flatMap((user) => [user.username, user.email])]) {
      require(!identities.has(identity), "Keycloak prepared group identities overlap");
      identities.add(identity);
    }
  }
}
export function assertPreparedBinding(expected: PreparedBinding, actual: PreparedBinding | undefined): void {
  require(deepEquals(expected, actual, true), "Keycloak prepared current source, target, group or issuer differs");
}

/** One backend, a fresh official Keycloak, the unchanged 8/7/1 browser oracle. */
export async function consumeKeycloak(scope: UiProcesses, inputs: ConsumerInputs): Promise<void> {
  inputs.checkInputs();
  validatePreparedInputs(inputs);
  require(sha(process.execPath) ===
    inputs.manifest.bun.sha256, "current Bun differs from the frozen runtime");
  const nativeVersion = await runCommand(
    scope,
    [inputs.manifest.binaries["fvoci-server"]?.path ?? "", "--version"],
    { signal: inputs.signal, seconds: 30 },
  );
  require(nativeVersion.code === 0 &&
    nativeVersion.stdout.includes(
      "(" + inputs.manifest.sourceInputs.source + ")",
    ), "normal server source marker differs from the frozen compiler cohort");
  require(selftestFailures().length === 0, "redaction selftest failed");
  const work = mkdtempSync(join(inputs.directory, "keycloak-"));
  const realms = join(work, "realm"),
    out = join(work, "out");
  mkdirSync(realms, { mode: 0o755 });
  mkdirSync(out, { mode: 0o700 });
  const secrets = Object.fromEntries(
    SECRET_ENV.map((name) => [name, randomBytes(32).toString("hex")]),
  );
  registerEnvSecrets(secrets);
  const project = "fvoci-kc-e2e-" + randomBytes(6).toString("hex");
  const projectResources = [
    ["containers", ["ps", "-aq"]],
    ["volumes", ["volume", "ls", "-q"]],
    ["networks", ["network", "ls", "-q"]],
  ] as const;
  const composeArgs = [
    "docker",
    "compose",
    "-p",
    project,
    "-f",
    join(root, "scripts/keycloak/compose.yml"),
  ];
  const composeEnv = {
    ...cleanEnv(),
    KC_BOOTSTRAP_ADMIN_PASSWORD: secrets.KC_BOOTSTRAP_ADMIN_PASSWORD as string,
    FVOCI_KC_REALM_DIR: realms,
  };
  const command = async (
    args: string[],
    env = cleanEnv(),
    signal: AbortSignal | null = inputs.signal ?? null,
    seconds = 240,
  ) => {
    const result = await runCommand(scope, args, { env, signal: signal ?? undefined, seconds });
    if (result.code !== 0) {
      emit(result.stderr);
      throw new HelperError("Keycloak command failed");
    }
    return result.stdout.trim();
  };
  const compose = (args: string[], signal: AbortSignal | null = inputs.signal ?? null) =>
    command([...composeArgs, ...args], composeEnv, signal);
  let original: unknown;
  const cleanupErrors: string[] = [];
  const retain = (name: string, text: string) => {
    if (inputs.evidence)
      writeFileSync(join(inputs.evidence, name), safeText(text), { mode: 0o600 });
  };
  let created = false;
  try {
    writeFileSync(
      join(realms, "fvoci-e2e-realm.json"),
      renderTemplate(
        readFileSync(join(root, "scripts/keycloak/realm.template.json"), "utf8"),
        realmValues(secrets),
      ),
      { mode: 0o644 },
    );
    if (inputs.workspaceSso)
      for (const [name, values] of ssoRealmValues(secrets))
        writeFileSync(
          join(realms, name),
          renderTemplate(
            readFileSync(join(root, "scripts/keycloak/workspace-sso-realm.template.json"), "utf8"),
            values,
          ),
          { mode: 0o644 },
        );
    for (const [, args] of projectResources)
      require((await command([
        "docker",
        ...args,
        "--filter",
        "label=com.docker.compose.project=" + project,
      ])) === "", "Keycloak project namespace is already allocated");
    created = true;
    await compose(["up", "-d", "--quiet-pull"]);
    const published = await compose(["port", "keycloak", "8080"]);
    require(/^127\.0\.0\.1:[0-9]+$/.test(published), "Keycloak must bind loopback only");
    const origin = "http://" + published,
      issuer = origin + "/realms/" + REALM;
    addSecret(origin, "Keycloak origin");
    addSecret(published, "Keycloak publication");
    const config = specConfig(issuer, secrets);
    const configPath = join(work, "config.json");
    privateWrite(configPath, JSON.stringify(config));
    const sso = inputs.workspaceSso ? ssoConfig(origin, secrets) : undefined;
    const ssoPath = join(work, "sso.json");
    if (sso) privateWrite(ssoPath, JSON.stringify(sso));
    const issuers = [
      issuer,
      ...(sso ? [origin + "/realms/fvoci-e2e-ws-a", origin + "/realms/fvoci-e2e-ws-b"] : []),
    ];
    const deadline = now() + 240;
    for (;;) {
      inputs.signal?.throwIfAborted();
      try {
        for (const item of issuers) await ready(item);
        break;
      } catch {
        require(now() < deadline, "Keycloak discovery/JWKS readiness failed");
        require((await compose(["ps", "--status", "running", "--quiet", "keycloak"])) !==
          "", "Keycloak exited before readiness");
        await pause(1000);
      }
    }
    const setup = await verify(config, sso);
    require(setup.problems.length === 0, "Keycloak realm settings differ");

    retain("keycloak-setup.json", JSON.stringify(setup.report, null, 2));
    const image = await compose(["config", "--images"]);
    retain(
      "versions.json",
      JSON.stringify(
        {
          source: inputs.manifest.sourceInputs.source,
          tree: inputs.manifest.sourceInputs.tree,
          binaries: inputs.manifest.binaries,
          bun: inputs.manifest.bun,
          chromiumSha256: sha(inputs.manifest.chromium),
          playwright: (
            JSON.parse(
              readFileSync(join(root, "node_modules/@playwright/test/package.json"), "utf8"),
            ) as { version: string }
          ).version,
          keycloakImage: image,
          keycloakRepoDigests: JSON.parse(
            await command([
              "docker",
              "image",
              "inspect",
              "--format",
              "{{json .RepoDigests}}",
              image,
            ]),
          ) as unknown,
          profile: "dev opt0/debug_assertions; api-schema,db-tests; functional witness",
          fvociServer: nativeVersion.stdout.trim(),
        },
        null,
        2,
      ),
    );
    const codes: number[] = [];
    for (const mode of MODES) {
      inputs.signal?.throwIfAborted();
      const group = join(work, mode);
      mkdirSync(group, { mode: 0o700 });
      inputs.checkInputs();
      const binding: PreparedBinding | undefined = inputs.prepared ? {
        source: inputs.prepared.source, tree: inputs.prepared.tree,
        target: inputs.prepared.target, group: mode, issuer,
      } : undefined;
      const database = await inputs.prepare(mode, group, binding);
      const logPath = join(group, "server.log");
      const fd = openSync(logPath, "wx", 0o600);
      let server: Child | undefined, base: string | undefined;
      try {
        let before: PreparedSnapshot | undefined;
        let groupConfigPath = configPath;
        if (inputs.prepared && binding) {
          assertPreparedBinding(binding, database.prepared);
          before = structuredClone(await inputs.prepared.observe(binding));
          assertPreparedBinding(binding, before.binding);
          const actors = inputs.prepared.groups[mode];
          const prepared: PreparedInstance = {
            binding, actors, baseline: { users: before.users, links: before.links },
            idpBefore: MODES.slice(0, MODES.indexOf(mode)).flatMap((prior) =>
              Object.entries(inputs.prepared?.groups[prior].users ?? {}).map(([key, user]) =>
                [user.username, user.email, USERS[key]?.[1] ?? false,
                  prior === "failures" && key === "tina" ? [] : [...(USERS[key]?.[2] ?? [])]] as [string, string, boolean, string[]],
              )),
          };
          validatePreparedInstance(prepared);
          require(before.memberships.filter((row) => row.workspaceId === actors.workspace.id &&
            row.userId === actors.owner.id && row.role === "owner").length === 1,
          "Keycloak prepared owner membership differs");
          addSecret(actors.owner.password, "prepared owner password");
          addSecret(actors.member.password, "prepared member password");
          groupConfigPath = join(group, "config.json");
          privateWrite(groupConfigPath, JSON.stringify({
            ...config, prepared,
            users: Object.fromEntries(Object.entries(config.users as Record<string, Record<string, string>>).map(
              ([key, user]) => [key, { ...user, ...actors.users[key as keyof typeof actors.users] }],
            )),
            fvoci: { ownerPassword: actors.owner.password, memberPassword: actors.member.password },
          }));
        } else require(database.prepared === undefined, "Keycloak prepared backend requires an admitted caller");
        const app = {
          ...database.environment,
          OIDC_GENERIC_ISSUER: issuer,
          OIDC_GENERIC_CLIENT_ID: CLIENT_ID,
          OIDC_GENERIC_CLIENT_SECRET: secrets[
            mode === "wrong-secret" ? "KC_E2E_WRONG_SECRET" : "KC_E2E_CLIENT_SECRET"
          ] as string,
          OIDC_GENERIC_LABEL: LABEL,
          OIDC_ALLOW_INSECURE: "1",
          FVOCI_BIND: "127.0.0.1:0",
          FVOCI_PUBLIC_ORIGIN: "http://127.0.0.1:0",
          FVOCI_STATIC_DIR: join(root, "apps/web/dist"),
          FVOCI_STORAGE_DIR: join(group, "storage"),
          RUST_LOG: "warn,tower_http=debug",
        };
        mkdirSync(app.FVOCI_STORAGE_DIR, { mode: 0o700 });
        server = scope.spawn(
          [inputs.manifest.binaries["fvoci-migrate"]?.path ?? "", "--start"],
          "normal-server",
          { env: app, stdin: "ignore", stdout: fd, stderr: fd },
        );
        base = await awaitListening(logPath, server, now() + 30);
        await requireSetup(base, binding === undefined);
        retain(
          `process-environment-${mode}.json`,
          JSON.stringify(
            await assertServerEnvironment(
              scope,
              server.pid,
              app,
              inputs.manifest,
              inputs.processObserver,
              mode === "flows",
            ),
          ),
        );
        const native = inputs.manifest.binaries["fvoci-e2e-fixture"]?.path ?? "";
        const observed = await runCommand(scope, [native, "oidc-observe"], {
          env: database.observer,
          input: JSON.stringify({ operation: "users" }),
          signal: inputs.signal,
        });
        require(observed.code === 0, "native observer could not read the initialized backend");
        const witness = JSON.parse(observed.stdout) as {
          value: unknown;
          rollback: unknown;
          lifecycleDrain: unknown;
          leases: unknown;
        };
        require(witness.value === (before ? before.users.length : 0) &&
          witness.rollback === "confirmed" &&
          witness.lifecycleDrain === "confirmed" &&
          witness.leases === 0, "native observer baseline or closure changed");
        if (database.backend === "libsql-remote" && binding === undefined) {
          for (const wrong of [
            { ...database.observer, SSL_CERT_FILE: "/etc/ssl/certs/ca-certificates.crt" },
            {
              ...database.observer,
              FVOCI_LIBSQL_URL: (database.observer.FVOCI_LIBSQL_URL as string).replace(
                "127.0.0.1",
                "localhost",
              ),
            },
            { ...database.observer, FVOCI_LIBSQL_AUTH_TOKEN: randomBytes(32).toString("hex") },
          ]) {
            const refused = await runCommand(scope, [native, "oidc-observe"], {
              env: wrong,
              input: JSON.stringify({ operation: "users" }),
              signal: inputs.signal,
            });
            require(refused.code !== 0, "native libSQL TLS or token negative control was accepted");
          }
          // OpenSSL connects to the exact listener while checking an unrelated
          // DNS name, distinguishing SAN rejection from a DNS/connect failure.
          const endpoint = new URL(database.observer.FVOCI_LIBSQL_URL as string);
          const hostname = await runCommand(
            scope,
            [
              "openssl",
              "s_client",
              "-connect",
              endpoint.host,
              "-servername",
              "wrong.example.com",
              "-verify_hostname",
              "wrong.example.com",
              "-verify_return_error",
              "-CAfile",
              database.observer.SSL_CERT_FILE as string,
            ],
            { signal: inputs.signal },
          );
          require(hostname.code !== 0 &&
            hostname.stderr.includes(
              "hostname mismatch",
            ), "TLS hostname rejection was not observed");
          retain(
            `libsql-controls-${mode}.json`,
            JSON.stringify({
              nativeRead: "PASS",
              nativeWrongCa: "PASS",
              nativeWrongHostname: "PASS",
              nativeWrongToken: "PASS",
              opensslExactListenerSanRejection: "PASS",
              certificateSha256: sha(database.observer.SSL_CERT_FILE as string),
            }),
          );
        }
        const observer = join(group, "observer.json");
        privateWrite(observer, JSON.stringify(database.observer));
        const env = {
          ...cleanEnv(),
          PLAYWRIGHT_BASE_URL: base,
          FVOCI_STATIC_DIR: app.FVOCI_STATIC_DIR,
          FVOCI_KC_E2E_CONFIG: groupConfigPath,
          FVOCI_KC_E2E_OUT: out,
          FVOCI_KC_E2E_MODE: mode,
          FVOCI_KC_OBSERVER_BIN: inputs.manifest.binaries["fvoci-e2e-fixture"]?.path ?? "",
          FVOCI_KC_OBSERVER_ENV: observer,
          SERVER_LOG: logPath,
          OIDC_GENERIC_ISSUER: issuer,
          OIDC_GENERIC_CLIENT_SECRET: app.OIDC_GENERIC_CLIENT_SECRET,
          OIDC_ALLOW_INSECURE: "1",
          TMPDIR: process.env.TMPDIR ?? "",
          FVOCI_KC_CHROMIUM_TMPDIR: process.env.FVOCI_KC_CHROMIUM_TMPDIR ?? "",
          FVOCI_KC_CHROMIUM_BIN: inputs.manifest.chromium,
        };
        const args = [
          process.execPath,
          ...bunIsolation(),
          join(root, "node_modules/@playwright/test/cli.js"),
          "test",
          "e2e-keycloak/oidc-keycloak-flow.spec.ts",
          "--config=e2e-keycloak/keycloak.config.ts",
          "--output=" + join(group, "playwright-output"),
        ];
        const result = await runCommand(scope, inputs.heavy ? [inputs.heavy, ...args] : args, {
          env,
          cwd: join(root, "apps/web"),
          signal: inputs.signal,
          seconds: 1800,
        });
        if ([-1, -2, -13, -15].includes(result.code)) throw new RunInterrupted(128 - result.code);
        emit(result.stdout);
        emit(result.stderr);
        privateWrite(join(out, `${mode}.log`), safeText(result.stdout + result.stderr));
        codes.push(groupVerdict(out, mode, result.code));
        if (inputs.prepared && before && binding) {
          require(codes.at(-1) === 0, "Keycloak prepared group failed before serial continuation");
          const delta = JSON.parse(readFileSync(join(out, `prepared-${mode}.private.json`), "utf8")) as PreparedDelta;
          const actors = inputs.prepared.groups[mode];
          const emails = mode === "flows" ? [actors.users.carol.email, actors.mismatchEmail, actors.users.erin.email]
            : mode === "failures" ? [actors.member.email] : [];
          require(deepEquals([...delta.emails].sort(), emails.sort(), true) &&
            delta.links.length === (mode === "flows" ? 4 : mode === "failures" ? 2 : 0),
          "Keycloak prepared browser delta composition differs");
          const tuples = mode === "flows" ? [
            [actors.owner.email, actors.users.alice.email],
            [actors.users.carol.email, actors.users.carol.email],
            [actors.mismatchEmail, actors.users.mallory.email],
            [actors.users.erin.email, actors.users.erin.email],
          ] : mode === "failures" ? [[actors.owner.email, actors.users.alice.email], [actors.member.email, actors.users.tina.email]] : [];
          require(deepEquals(delta.links.map((row) => [row.user, row.email]).sort(), tuples.sort(), true) &&
            delta.links.every((row) => row.provider === "generic" && row.issuer === issuer &&
              /^[a-f0-9-]{36}$/.test(row.subject)) &&
            new Set(delta.links.map((row) => row.subject)).size === delta.links.length,
          "Keycloak prepared IdP or actor link delta differs");
          const closed = await scope.finish(server, true);
          server = undefined;
          require(closed === 0 && await portClosed(base), "Keycloak prepared server closure failed");
          const after = structuredClone(await inputs.prepared.observe(binding));
          assertPreparedDelta(before, after, delta);
          for (const email of delta.emails) {
            const user = after.users.find((row) => row.email === email);
            require(user !== undefined && after.memberships.filter((row) =>
              row.workspaceId === actors.workspace.id && row.userId === user.id && row.role === "member").length === 1,
            "Keycloak prepared member membership differs");
          }
          await inputs.prepared.assertWholePrimary(before, after, delta);
          inputs.checkInputs();
        }
        if (result.code !== 0 && inputs.evidence) {
          for (const entry of readdirSync(group, { recursive: true, withFileTypes: true }))
            if (entry.isFile() && /\.(?:md|log|txt)$/.test(entry.name)) {
              const path = join(entry.parentPath, entry.name);
              retain(
                `failure-${mode}-${path.slice(group.length + 1).replaceAll("/", "_")}`,
                readFileSync(path, "utf8"),
              );
            }
        }
      } finally {
        if (server)
          await scope
            .finish(server, true)
            .then((code) => {
              if (code !== 0) cleanupErrors.push("normal server exit was nonzero");
            })
            .catch(() => {
              cleanupErrors.push("server retirement failed");
            });
        closeSync(fd);
        if (inputs.evidence) {
          try {
            retain(`server-${mode}.log`, readFileSync(logPath, "utf8"));
          } catch {
            cleanupErrors.push("server log collection failed");
          }
        }
        if (base && !(await portClosed(base))) cleanupErrors.push("server port remains open");
        await database.close().catch(() => {
          cleanupErrors.push("backend retirement failed");
        });
      }
    }
    inputs.signal?.throwIfAborted();
    if (inputs.workspaceSso) {
      const test = inputs.workspaceSso;
      const result = await runCommand(
        scope,
        [test.binary, "keycloak_workspace_sso_with_a_test_entitlement", "--ignored", "--nocapture"],
        {
          env: { ...test.environment, FVOCI_KC_SSO_E2E_CONFIG: ssoPath },
          signal: inputs.signal,
          seconds: 240,
        },
      );
      privateWrite(join(out, "workspace-sso.log"), safeText(result.stdout + result.stderr));
      codes.push(groupVerdict(out, "workspace-sso", result.code));
    }
    retain(
      "keycloak-events.json",
      JSON.stringify(
        await events(config, [REALM, ...(sso ? ["fvoci-e2e-ws-a", "fvoci-e2e-ws-b"] : [])]),
        null,
        2,
      ),
    );
    require(codes.every((code) => code === 0), "Keycloak acceptance group failed");
  } catch (error) {
    original = error;
  } finally {
    if (inputs.evidence) {
      try {
        for (const entry of readdirSync(work, { recursive: true, withFileTypes: true }))
          if (entry.isFile() && /\.(?:md|log|txt)$/.test(entry.name)) {
            const file = join(entry.parentPath, entry.name);
            retain(
              "output-" + file.slice(work.length + 1).replaceAll("/", "_"),
              readFileSync(file, "utf8"),
            );
          }
        for (const name of readdirSync(out)) {
          if (/^prepared-.*\.private\.json$/.test(name)) continue;
          retain(name, readFileSync(join(out, name), "utf8"));
        }
      } catch {
        cleanupErrors.push("redacted output collection failed");
      }
    }
    if (created) {
      if (inputs.evidence) {
        try {
          const logs = await compose(["logs", "--no-color", "keycloak"], null);
          writeFileSync(join(inputs.evidence, "keycloak-container.log"), safeText(logs), {
            mode: 0o600,
          });
        } catch {
          cleanupErrors.push("Keycloak log collection failed");
        }
      }
      try {
        await compose(["down", "-v", "--remove-orphans"], null);
      } catch {
        cleanupErrors.push("Keycloak compose cleanup failed");
      }
      for (const [kind, args] of projectResources) {
        try {
          require((await command(
            ["docker", ...args, "--filter", "label=com.docker.compose.project=" + project],
            cleanEnv(),
            null,
          )) === "", "owned resources remain");
        } catch {
          cleanupErrors.push("Keycloak " + kind + " cleanup unconfirmed");
        }
      }
    }
    try {
      inputs.checkInputs();
    } catch {
      cleanupErrors.push("source or artifact inputs changed");
    }
    try {
      rmSync(work, { recursive: true });
    } catch {
      cleanupErrors.push("private run directory removal failed");
    }
    if (cleanupErrors.length) emit(cleanupErrors.join("\n"));
  }
  if (original !== undefined)
    throw original instanceof Error ? original : new HelperError("Keycloak consumer failed");
  require(cleanupErrors.length === 0, "Keycloak cleanup failed");
}

export interface PostgresFixture {
  container: string;
  admin: string;
  close(): Promise<void>;
}

export async function startPostgres(
  scope: UiProcesses,
  owner: string,
  signal?: AbortSignal,
): Promise<PostgresFixture> {
  const container = "fvoci-kc-pg-" + randomBytes(6).toString("hex");
  const password = randomBytes(32).toString("hex");
  addSecret(password, "PostgreSQL fixture password");
  const run = async (args: string[], input?: string, interrupt = signal) =>
    runCommand(scope, args, { input, signal: interrupt });
  let created = false;
  let base: string | undefined;
  const close = async () => {
    if (!created) return;
    const owned = await runCommand(scope, [
      "docker",
      "ps",
      "-aq",
      "--filter",
      "name=^/" + container + "$",
      "--filter",
      "label=fvoci.owner=" + owner,
      "--filter",
      "label=fvoci.keycloak-fixture=" + container,
    ]);
    require(owned.code === 0, "owned PostgreSQL identity could not be observed");
    if (owned.stdout.trim() !== "") {
      require(/^[0-9a-f]{12,64}$/.test(
        owned.stdout.trim(),
      ), "owned PostgreSQL identity is malformed");
      const removed = await runCommand(scope, ["docker", "rm", "-f", "-v", owned.stdout.trim()]);
      require(removed.code === 0, "owned PostgreSQL removal failed");
    }
    const absent = await runCommand(scope, [
      "docker",
      "ps",
      "-aq",
      "--filter",
      "name=^/" + container + "$",
    ]);
    require(absent.code === 0 &&
      absent.stdout.trim() === "", "owned PostgreSQL cleanup unconfirmed");
    if (base) require(await portClosed(base), "owned PostgreSQL port remains open");
  };
  try {
    created = true;
    const started = await runCommand(
      scope,
      [
        "docker",
        "run",
        "-d",
        "--name",
        container,
        "--label",
        "fvoci.owner=" + owner,
        "--label",
        "fvoci.keycloak-fixture=" + container,
        "-e",
        "POSTGRES_PASSWORD",
        "-p",
        "127.0.0.1::5432",
        pgImage,
        "postgres",
        "-c",
        "max_connections=150",
      ],
      {
        env: { ...cleanEnv(), POSTGRES_PASSWORD: password },
        signal,
      },
    );
    require(started.code === 0, "owned PostgreSQL start failed");
    const published = await run(["docker", "port", container, "5432/tcp"]);
    require(published.code === 0 &&
      /^127\.0\.0\.1:[0-9]+$/.test(published.stdout.trim()), "PostgreSQL must bind loopback only");
    base = "http://" + published.stdout.trim();
    const admin = "postgres://postgres:" + password + "@" + published.stdout.trim() + "/postgres";
    addSecret(admin, "PostgreSQL observer URL");
    const deadline = now() + 30;
    for (;;) {
      const probe = await run(["docker", "exec", container, "pg_isready", "-U", "postgres"]);
      if (probe.code === 0) break;
      require(now() < deadline, "owned PostgreSQL readiness failed");
      await pause(1000);
    }
    const version = await run([
      "docker",
      "exec",
      container,
      "psql",
      "-U",
      "postgres",
      "-tAc",
      "SHOW server_version_num",
    ]);
    require(version.code === 0 &&
      /^18[0-9]{4}$/.test(version.stdout.trim()), "owned PostgreSQL major version changed");
    return { container, admin, close };
  } catch (error) {
    try {
      await close();
    } catch (cleanup) {
      throw new AggregateError([error, cleanup], "owned fixture preparation and cleanup failed", {
        cause: cleanup,
      });
    }
    throw error;
  }
}

/** Explicit database preparation; normal servers inherit only app inputs. */
export async function prepareBackend(
  scope: UiProcesses,
  manifest: Manifest,
  backend: Backend,
  directory: string,
  postgres?: PostgresFixture,
  sqld?: string,
  libsqlRoot?: string,
): Promise<PreparedBackend> {
  const common = {
    ...cleanEnv(),
    FVOCI_DATABASE_BACKEND: backend,
    PASSWORD_PEPPER_KEYS: JSON.stringify({ keycloak: randomBytes(32).toString("hex") }),
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "keycloak",
    ENCRYPTION_KEYS: JSON.stringify({ keycloak: randomBytes(32).toString("hex") }),
    ENCRYPTION_ACTIVE_KEY_ID: "keycloak",
    FVOCI_COOKIE_SECURE: "0",
    STORAGE_DRIVER: "local",
  };
  for (const name of ["PASSWORD_PEPPER_KEYS", "ENCRYPTION_KEYS"] as const)
    addSecret(common[name], "native keyring");
  const observer = {
    ...cleanEnv(),
    FVOCI_KC_OBSERVER_SELECTED: "1",
    E2E_DATABASE_BACKEND: backend,
  };
  if (backend === "sqlite") {
    const path = join(directory, "app.sqlite");
    require(!existsSync(path), "SQLite fixture database already exists");
    return {
      backend,
      environment: { ...common, FVOCI_SQLITE_PATH: path },
      observer: { ...observer, FVOCI_SQLITE_PATH: path },
      close: async () => {},
    };
  }
  if (backend === "libsql-remote") {
    require(sqld !== undefined, "pinned official sqld is required");
    const localDirectory = join(directory, "libsql");
    require(libsqlRoot !== undefined &&
      below(localDirectory, libsqlRoot), "libSQL fixture is outside the allocated namespace");
    const local = await startLocalLibsql(scope, sqld as string, localDirectory);
    addSecret(local.environment.FVOCI_LIBSQL_AUTH_TOKEN as string, "local libSQL token");
    return {
      backend,
      environment: { ...common, ...local.environment },
      observer: { ...observer, ...local.environment },
      close: () => local.close(),
    };
  }
  require(postgres !== undefined, "owned PostgreSQL fixture is required");
  const pg = postgres as PostgresFixture;
  const suffix = randomBytes(8).toString("hex"),
    db = "fvoci_kc_" + suffix,
    role = "fvoci_kc_app_" + suffix;
  const password = randomBytes(32).toString("hex");
  addSecret(password, "PostgreSQL app password");
  const preparationLog = join(directory, "postgres-preparation.log");
  privateWrite(preparationLog, "");
  const collect = (stdout: string, stderr: string) => {
    appendFileSync(preparationLog, safeText(stdout + stderr));
  };
  const urls = databaseUrls(pg.admin, db, role, password);
  addSecret(urls.admin, "PostgreSQL observer URL");
  addSecret(urls.app, "PostgreSQL app URL");
  const psql = async (database: string, input: string) => {
    const result = await runCommand(
      scope,
      [
        "docker",
        "exec",
        "-i",
        pg.container,
        "psql",
        "-U",
        "postgres",
        "-d",
        database,
        "-v",
        "ON_ERROR_STOP=1",
        "-tA",
      ],
      { input, output: collect },
    );
    require(result.code === 0, "owned PostgreSQL preparation failed");
    return result.stdout.trim();
  };
  const close = async () => {
    await psql(
      "postgres",
      `DROP DATABASE IF EXISTS "${db}" WITH (FORCE); DROP ROLE IF EXISTS "${role}";`,
    );
  };
  try {
    await psql("postgres", `CREATE DATABASE "${db}";`);
    const migrated = await runCommand(scope, [manifest.binaries["fvoci-migrate"]?.path ?? ""], {
      env: { ...common, DATABASE_URL: urls.admin },
      output: collect,
    });
    require(migrated.code === 0, "owned PostgreSQL migration failed");
    await psql(db, `CREATE ROLE "${role}" LOGIN PASSWORD '${password}' NOSUPERUSER NOBYPASSRLS;`);
    const granted = await runCommand(
      scope,
      [manifest.binaries["fvoci-migrate"]?.path ?? "", "--grant-app-role", role],
      { env: { ...common, DATABASE_URL: urls.admin }, output: collect },
    );
    require(granted.code === 0, "owned PostgreSQL app grant failed");
    require((await psql(
      db,
      `SELECT (NOT rolsuper AND NOT rolbypassrls)::text FROM pg_roles WHERE rolname='${role}';`,
    )) === "true", "normal app role is privileged");
    return {
      backend,
      environment: { ...common, DATABASE_APP_URL: urls.app },
      observer: { ...observer, FVOCI_KC_OBSERVER_DATABASE_URL: urls.admin },
      close,
    };
  } catch (error) {
    await close();
    throw error;
  }
}

// CLI admission is deliberately the same maintained lease boundary as the
// current native/UI recorder. Runtime remains refused until ROOT wires its
// purpose-specific loader; a caller cannot make a hosted environment local.
export async function main(argv: string[], load = lease.load): Promise<number> {
  try {
    assertNoEnvFile();
    const mode = argv.join("\0");
    if (mode === "--record-before") recordBefore(load);
    else if (mode === "--freeze") freeze(load);
    else {
      const selected = argv.find((arg) => arg.startsWith("--backend="))?.slice("--backend=".length);
      require(["postgres", "sqlite", "libsql-remote"].includes(selected ?? "") &&
        argv.every(
          (arg) => arg === "--backend=" + String(selected) || arg === "--workspace-sso",
        ), "usage: run.ts --backend=postgres|sqlite|libsql-remote [--workspace-sso], --record-before, or --freeze");
      require(!argv.includes("--workspace-sso") ||
        selected === "postgres", "workspace SSO entitlement witness is PostgreSQL only");
      const grant = loadLocalLease("keycloak", load);
      require(deepEquals(
        [process.execPath, ...process.execArgv, import.meta.path, ...argv],
        grant.stageCommands.run,
      ), "runtime arguments differ from the allocated exact stage");
      const configuration = grant.keycloak as
        | {
            backends?: Backend[];
            sqld?: { path: string; sha256: string };
            heavy?: string;
            chromiumTmp?: string;
            sso?: { path: string; sha256: string };
            processObserver?: ProcessObserver;
          }
        | undefined;
      require(configuration?.backends?.includes(
        selected as Backend,
      ), "backend is outside the local allocation");
      require(configuration?.processObserver !==
        undefined, "reviewed privileged process observer is required");
      const manifest = currentBuild(load);
      const directory = resolve(grant.outputRoot, "runtime");
      mkdirSync(directory, { recursive: true, mode: 0o700 });
      const evidence = join(directory, "keycloak-" + String(selected) + "-evidence");
      mkdirSync(evidence, { mode: 0o700 });
      require(typeof configuration?.heavy ===
        "string", "allocated heavy command wrapper is required");
      require(typeof configuration?.chromiumTmp === "string" &&
        configuration.chromiumTmp.length > 0, "allocated Chromium socket directory is required");
      process.env.FVOCI_KC_CHROMIUM_TMPDIR = configuration?.chromiumTmp;
      const controller = new AbortController();
      const handlers = (
        [
          ["SIGINT", 130],
          ["SIGTERM", 143],
          ["SIGHUP", 129],
          ["SIGPIPE", 141],
        ] as const
      ).map(([signal, code]) => {
        const handle = () => {
          if (!controller.signal.aborted) controller.abort(new RunInterrupted(code));
        };
        process.on(signal, handle);
        return { signal, handle };
      });
      try {
        await withProcesses(
          async (scope) => {
            const pg =
              selected === "postgres"
                ? await startPostgres(scope, grant.owner, controller.signal)
                : undefined;
            try {
              const sso = argv.includes("--workspace-sso") ? configuration?.sso : undefined;
              if (argv.includes("--workspace-sso"))
                require(sso !== undefined &&
                  sha(sso.path) ===
                    sso.sha256, "current workspace SSO test executable is required");
              if (selected === "libsql-remote")
                require(configuration?.sqld !== undefined &&
                  sha(configuration.sqld.path) ===
                    configuration.sqld.sha256, "allocated sqld input changed");
              await consumeKeycloak(scope, {
                manifest,
                directory,
                evidence,
                signal: controller.signal,
                heavy: configuration?.heavy,
                processObserver: configuration?.processObserver as ProcessObserver,
                prepare: (_mode, path) =>
                  prepareBackend(
                    scope,
                    manifest,
                    selected as Backend,
                    path,
                    pg,
                    configuration?.sqld?.path,
                    (grant.localLibsql as { directory: string } | undefined)?.directory,
                  ),
                checkInputs: () => {
                  require(deepEquals(currentBuild(load), manifest, true), "current inputs changed");
                },
                workspaceSso:
                  sso && pg
                    ? {
                        binary: sso.path,
                        environment: { ...cleanEnv(), TEST_DATABASE_URL: pg.admin },
                      }
                    : undefined,
              });
            } finally {
              await pg?.close();
            }
          },
          new UiProcesses(undefined, {
            root: () => directory,
            write: (path, value) => {
              privateWrite(path, safeText(JSON.stringify(value)));
            },
            output: { out: emit, err: emit },
          }),
        );
      } finally {
        for (const { signal, handle } of handlers) process.off(signal, handle);
      }
    }
    return 0;
  } catch (error) {
    emit(
      error instanceof HelperError ? error.message : `Keycloak runner failed (${errorName(error)})`,
    );
    return error instanceof RunInterrupted ? error.exitCode : 1;
  }
}

if (import.meta.main) process.exitCode = await main(process.argv.slice(2));
