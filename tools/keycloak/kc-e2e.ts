// Helpers of scripts/keycloak-oidc-e2e.sh (local, opt-in Keycloak OIDC check).
//
// Secrets come from the environment or the mode-600 config file and are never
// printed; `redact` replaces them and every code/state/token-shaped value.
import { closeSync, openSync, readFileSync, writeFileSync, writeSync } from "node:fs";
import { machine, release, type } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { parseArgs } from "node:util";
import { events, ready, verify } from "./admin.ts";
import {
  groupSummary,
  pyDumps,
  pyDumpsIndented,
  realmValues,
  registerEnvSecrets,
  renderTemplate,
  specConfig,
  ssoConfig,
  ssoRealmValues,
  type Json,
} from "./realm.ts";
import {
  HelperError,
  LineSplitter,
  errorName,
  REDACTION_CASES,
  redactLine,
  knownSecrets,
  registerConfigSecrets,
  scrubText,
  selftestFailures,
} from "./redact.ts";

const USAGE = `usage: kc-e2e.ts <command> [<args>]
  render <template> <out>               realm file from the per-run secrets
                                        in the environment
  render-sso <template> <out-dir>       the two workspace SSO realm files
  config <issuer> <out>                 the spec's mode-600 config
  sso-config <keycloak-origin> <out>    the Rust workspace SSO test's config
  ready <issuer>...                     exit 0 once discovery and JWKS answer
  verify <config> [<sso-config>]        imported realms/clients read back via
                                        the admin API and token endpoint
  events <config> <realm>...            Keycloak event summary (no ids/tokens)
  redact [<config>]                     stdin to stdout without secrets
  selftest                              the redaction cases
  versions --evidence=<dir> --playwright-files=<json> --source-sha=<sha>
           --root=<dir> --target-dir=<dir> --keycloak-image=<ref>
           --repo-digests=<json> --compose-project=<name> --published=<host:port>
           --issuer=<url>               toolchain and image versions (JSON)
  summary <playwright.json> <log>       one group's result line`;

/** Creates `path` (never overwrites) with `mode` and writes `text`; errors name it by `label`. */
function createFile(path: string, text: string, mode: number, label: string): void {
  let fd: number;
  try {
    fd = openSync(path, "wx", mode);
  } catch (error) {
    throw new HelperError(`cannot create ${label} (${errorName(error)})`);
  }
  try {
    writeSync(fd, text);
  } finally {
    closeSync(fd);
  }
}

/** A text file; errors name it by `label` only (paths are input). */
function readText(path: string, label: string): string {
  try {
    return readFileSync(path, "utf8");
  } catch (error) {
    throw new HelperError(`cannot read ${label} (${errorName(error)})`);
  }
}

/** A JSON value; errors name it by `label` only (the content may hold secrets). */
function parseJson(text: string, label: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    throw new HelperError(`${label} is not JSON`);
  }
}

/** A JSON file; errors name it by `label` only (paths are input, the content holds secrets). */
function loadJson(path: string, label: string): Json {
  const text = readText(path, label);
  try {
    return JSON.parse(text) as Json;
  } catch {
    throw new HelperError(`${label} is not JSON`);
  }
}

/** A spec config, its secrets registered before anything else is read or written. */
function loadConfig(path: string): Json {
  const config = loadJson(path, "the config");
  registerConfigSecrets(config);
  return config;
}

// The output boundary: every byte this helper writes to stdout or stderr,
// error paths included, goes through scrubText.
function out(text: string): void {
  process.stdout.write(scrubText(text));
}
function err(text: string): void {
  process.stderr.write(scrubText(text));
}

/**
 * Copies stdin to stdout, redacted. A given config must load before stdin is
 * read: the runner takes exit 0 as "its secrets were applied". Without one,
 * only the built-in rules apply.
 */
async function redactStdin(configPath: string | undefined): Promise<void> {
  if (configPath !== undefined) loadConfig(configPath);
  const secrets = knownSecrets();
  // Invalid UTF-8 becomes U+FFFD instead of ending the stream; a BOM is kept.
  const decoder = new TextDecoder("utf-8", { ignoreBOM: true });
  const splitter = new LineSplitter();
  // Each complete line is written as soon as it is read: the run log follows
  // a group live.
  const emit = async (lines: string[]) => {
    if (lines.length === 0) return;
    const text = lines.map((line) => redactLine(line, secrets)).join("");
    await Bun.write(Bun.stdout, scrubText(text));
  };
  const reader = Bun.stdin.stream().getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    await emit(splitter.push(decoder.decode(value, { stream: true })));
  }
  await emit(splitter.push(decoder.decode()));
  await emit(splitter.end());
}

/** Trimmed stdout of a command, or null when it cannot start or does not finish in 30 s. */
function run(...cmd: string[]): string | null {
  try {
    const child = Bun.spawnSync(cmd, {
      stdin: "ignore",
      stdout: "pipe",
      stderr: "ignore",
      timeout: 30_000,
    });
    if (child.exitedDueToTimeout === true || child.signalCode !== undefined) return null;
    return child.stdout.toString().trim();
  } catch {
    return null;
  }
}

/** `platform.platform()`: system, release, machine and, on glibc, `with-glibc<version>`. */
function osName(): string {
  const base = `${type()}-${release()}-${machine()}`;
  const glibc = /^glibc (\S+)$/.exec(run("getconf", "GNU_LIBC_VERSION") ?? "")?.[1];
  return glibc === undefined ? base : `${base}-with-glibc${glibc}`;
}

/** A member the evidence must carry; a missing one fails instead of vanishing from the JSON. */
function required(record: Json, key: string, label: string): unknown {
  if (!Object.hasOwn(record, key)) throw new HelperError(`${label} has no ${key}`);
  return record[key];
}

function versions(argv: string[]): void {
  const { values } = parseArgs({
    args: argv,
    strict: true,
    options: Object.fromEntries(
      [
        "evidence",
        "playwright-files",
        "source-sha",
        "root",
        "target-dir",
        "keycloak-image",
        "repo-digests",
        "compose-project",
        "published",
        "issuer",
      ].map((name) => [name, { type: "string" as const }]),
    ),
  });
  const value = (name: string) => {
    const given = values[name];
    if (typeof given !== "string") throw new HelperError(`versions: --${name} is required`);
    return given;
  };
  const files = parseJson(value("playwright-files"), "--playwright-files") as {
    test: string;
    browsers: string;
  };
  const browsers = loadJson(files.browsers, "browsers.json").browsers as Json[];
  // Headless runs use Playwright's chromium-headless-shell build.
  const shell = browsers.find((browser) => browser.name === "chromium-headless-shell");
  if (shell === undefined) throw new HelperError("browsers.json has no chromium-headless-shell");
  const server = `${value("target-dir")}/release/fvoci-server`;
  const info = {
    sourceSha: value("source-sha"),
    sourceTreeClean:
      run("git", "-C", value("root"), "status", "--porcelain", "--untracked-files=no") === "",
    fvociServer: run(server, "--version"),
    fvociServerBinary: `${server} (cargo build --release from source)`,
    rustc: run("rustc", "-V"),
    bun: run("bun", "--version"),
    playwrightRuntime: "bun --bun x --no-install playwright test",
    playwright: required(
      loadJson(files.test, "the Playwright package.json"),
      "version",
      "the Playwright package.json",
    ),
    browser: {
      name: "chromium-headless-shell",
      revision: required(shell, "revision", "browsers.json"),
      version: required(shell, "browserVersion", "browsers.json"),
      headless: true,
    },
    keycloakImage: value("keycloak-image"),
    keycloakRepoDigests: parseJson(value("repo-digests"), "--repo-digests"),
    keycloakMode: "start-dev --import-realm (dev-file database inside the container)",
    docker: run("docker", "version", "--format", "{{.Server.Version}}"),
    compose: run("docker", "compose", "version", "--short"),
    os: osName(),
    composeProject: value("compose-project"),
    keycloakPublished: value("published"),
    issuer: value("issuer"),
  };
  const text = pyDumpsIndented(info);
  out(`${text}\n`);
  const evidence = value("evidence");
  if (evidence !== "") writeFileSync(join(evidence, "versions.json"), text);
}

function summary(reportPath: string, logPath: string): string {
  let report: unknown;
  try {
    report = JSON.parse(readFileSync(reportPath, "utf8"));
  } catch (error) {
    if (!(error instanceof Error && "code" in error))
      throw new HelperError("the Playwright report is not JSON");
    let log: string | undefined;
    try {
      log = readFileSync(logPath, "utf8");
    } catch {
      log = undefined;
    }
    return groupSummary(undefined, log);
  }
  return groupSummary(report, undefined);
}

async function main(argv: string[]): Promise<void> {
  const [command, ...args] = argv;
  const env = process.env;
  const two = args.length === 2;
  // Every per-run secret in the environment is registered first, used by
  // the command or not.
  registerEnvSecrets(env);
  if (command === "render" && two) {
    const values = realmValues(env);
    createFile(
      args[1] as string,
      renderTemplate(readText(args[0] as string, "the template"), values),
      0o644,
      "the realm file",
    );
  } else if (command === "render-sso" && two) {
    const realms = ssoRealmValues(env);
    const template = readText(args[0] as string, "the template");
    // Readable by the container's keycloak user; the run directory is 0700.
    for (const [name, values] of realms) {
      createFile(
        join(args[1] as string, name),
        renderTemplate(template, values),
        0o644,
        "an SSO realm file",
      );
    }
  } else if (command === "config" && two) {
    createFile(args[1] as string, pyDumps(specConfig(args[0] as string, env)), 0o600, "the config");
  } else if (command === "sso-config" && two) {
    createFile(
      args[1] as string,
      pyDumps(ssoConfig(args[0] as string, env)),
      0o600,
      "the SSO config",
    );
  } else if (command === "ready" && args.length > 0) {
    for (const issuer of args) await ready(issuer);
  } else if (command === "verify" && (args.length === 1 || two)) {
    // The first config's secrets are registered before the second is read.
    const config = loadConfig(args[0] as string);
    const sso = two ? loadJson(args[1] as string, "the SSO config") : undefined;
    const { report, problems } = await verify(config, sso);
    out(`${pyDumpsIndented(report)}\n`);
    if (problems.length > 0)
      throw new HelperError(`imported settings differ:\n  ${problems.join("\n  ")}`);
  } else if (command === "events" && args.length > 0) {
    const report = await events(loadConfig(args[0] as string), args.slice(1));
    out(`${pyDumpsIndented(report)}\n`);
  } else if (command === "selftest" && args.length === 0) {
    const failed = selftestFailures();
    for (const [index, got] of failed) {
      const want = (REDACTION_CASES[index] as readonly [string, string])[1];
      err(
        `redaction case ${String(index)}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}\n`,
      );
    }
    if (failed.length > 0) {
      throw new HelperError(
        `${String(failed.length)} of ${String(REDACTION_CASES.length)} redaction cases failed`,
      );
    }
    out(`redaction selftest: ${String(REDACTION_CASES.length)} cases ok\n`);
  } else if (command === "redact" && args.length <= 1) {
    await redactStdin(args[0]);
  } else if (command === "versions") {
    versions(args);
  } else if (command === "summary" && two) {
    out(`${summary(args[0] as string, args[1] as string)}\n`);
  } else {
    throw new HelperError(command === undefined ? USAGE : "unknown command or arguments");
  }
}

/**
 * Reports an error through the output boundary. Only a HelperError's message
 * is written: any other error (fs, fetch, JSON.parse) may quote a path, URL or
 * input that holds a secret not yet registered, so it is named by its code or
 * type alone; stack and cause are never written.
 */
function fail(error: unknown): void {
  const message =
    error instanceof HelperError ? error.message : `unexpected error (${errorName(error)})`;
  err(`keycloak e2e: ${message}\n`);
  process.exitCode = 1;
}

if (import.meta.main) {
  // Errors outside main's await (a stray rejection or callback) take the same
  // path instead of Bun's default report, which would print them unscrubbed.
  process.on("uncaughtException", (error) => {
    fail(error);
    process.exit(1);
  });
  process.on("unhandledRejection", (error) => {
    fail(error);
    process.exit(1);
  });
  try {
    await main(process.argv.slice(2));
  } catch (error) {
    fail(error);
  }
}
