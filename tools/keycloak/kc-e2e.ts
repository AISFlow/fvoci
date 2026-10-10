// Helpers of scripts/keycloak-oidc-e2e.sh (local, opt-in Keycloak OIDC check).
//
// Secrets come from the environment or the mode-600 config file and are never
// printed; `redact` replaces them and every code/state/token-shaped value.
import { closeSync, existsSync, openSync, readFileSync, writeFileSync, writeSync } from "node:fs";
import { machine, release, type } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { parseArgs } from "node:util";
import { events, ready, verify } from "./admin.ts";
import {
  HelperError,
  groupSummary,
  pyDumps,
  pyDumpsIndented,
  realmValues,
  renderTemplate,
  specConfig,
  ssoConfig,
  ssoRealmValues,
  type Json,
} from "./realm.ts";
import {
  LineSplitter,
  REDACTION_CASES,
  redactLine,
  secretsOf,
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

/** Creates `path` (never overwrites) with `mode` and writes `text`. */
function createFile(path: string, text: string, mode: number): void {
  const fd = openSync(path, "wx", mode);
  try {
    writeSync(fd, text);
  } finally {
    closeSync(fd);
  }
}

function loadJson(path: string): Json {
  let text: string;
  try {
    text = readFileSync(path, "utf8");
  } catch (error) {
    throw new HelperError(`cannot read ${path} (${(error as { code?: string }).code ?? "error"})`);
  }
  try {
    return JSON.parse(text) as Json;
  } catch {
    // Never quote the content: the configs hold the per-run secrets.
    throw new HelperError(`${path} is not JSON`);
  }
}

async function redactStdin(configPath: string | undefined): Promise<void> {
  const secrets = configPath && existsSync(configPath) ? secretsOf(loadJson(configPath)) : [];
  // Invalid UTF-8 becomes U+FFFD instead of ending the stream; a BOM is kept.
  const decoder = new TextDecoder("utf-8", { ignoreBOM: true });
  const splitter = new LineSplitter();
  // Each complete line is written as soon as it is read: the run log follows
  // a group live.
  const out = async (lines: string[]) => {
    if (lines.length > 0)
      await Bun.write(Bun.stdout, lines.map((line) => redactLine(line, secrets)).join(""));
  };
  const reader = Bun.stdin.stream().getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    await out(splitter.push(decoder.decode(value, { stream: true })));
  }
  await out(splitter.push(decoder.decode()));
  await out(splitter.end());
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
function required(record: Json, key: string, source: string): unknown {
  if (!Object.hasOwn(record, key)) throw new HelperError(`${source} has no ${key}`);
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
  const files = JSON.parse(value("playwright-files")) as { test: string; browsers: string };
  const browsers = loadJson(files.browsers).browsers as Json[];
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
    playwright: required(loadJson(files.test), "version", files.test),
    browser: {
      name: "chromium-headless-shell",
      revision: required(shell, "revision", files.browsers),
      version: required(shell, "browserVersion", files.browsers),
      headless: true,
    },
    keycloakImage: value("keycloak-image"),
    keycloakRepoDigests: JSON.parse(value("repo-digests")) as unknown,
    keycloakMode: "start-dev --import-realm (dev-file database inside the container)",
    docker: run("docker", "version", "--format", "{{.Server.Version}}"),
    compose: run("docker", "compose", "version", "--short"),
    os: osName(),
    composeProject: value("compose-project"),
    keycloakPublished: value("published"),
    issuer: value("issuer"),
  };
  const text = pyDumpsIndented(info);
  process.stdout.write(`${text}\n`);
  const evidence = value("evidence");
  if (evidence !== "") writeFileSync(join(evidence, "versions.json"), text);
}

function summary(reportPath: string, logPath: string): string {
  let report: unknown;
  try {
    report = JSON.parse(readFileSync(reportPath, "utf8"));
  } catch (error) {
    if (!(error instanceof Error && "code" in error))
      throw new HelperError(`${reportPath} is not JSON`);
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
  if (command === "render" && two) {
    const values = realmValues(env);
    createFile(
      args[1] as string,
      renderTemplate(readFileSync(args[0] as string, "utf8"), values),
      0o644,
    );
  } else if (command === "render-sso" && two) {
    const realms = ssoRealmValues(env);
    const template = readFileSync(args[0] as string, "utf8");
    // Readable by the container's keycloak user; the run directory is 0700.
    for (const [name, values] of realms) {
      createFile(join(args[1] as string, name), renderTemplate(template, values), 0o644);
    }
  } else if (command === "config" && two) {
    createFile(args[1] as string, pyDumps(specConfig(args[0] as string, env)), 0o600);
  } else if (command === "sso-config" && two) {
    createFile(args[1] as string, pyDumps(ssoConfig(args[0] as string, env)), 0o600);
  } else if (command === "ready" && args.length > 0) {
    for (const issuer of args) await ready(issuer);
  } else if (command === "verify" && (args.length === 1 || two)) {
    const { report, problems } = await verify(
      loadJson(args[0] as string),
      two ? loadJson(args[1] as string) : undefined,
    );
    process.stdout.write(`${pyDumpsIndented(report)}\n`);
    if (problems.length > 0)
      throw new HelperError(`imported settings differ:\n  ${problems.join("\n  ")}`);
  } else if (command === "events" && args.length > 0) {
    const report = await events(loadJson(args[0] as string), args.slice(1));
    process.stdout.write(`${pyDumpsIndented(report)}\n`);
  } else if (command === "selftest" && args.length === 0) {
    const failed = selftestFailures();
    for (const [index, got] of failed) {
      const want = (REDACTION_CASES[index] as readonly [string, string])[1];
      process.stderr.write(
        `redaction case ${String(index)}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}\n`,
      );
    }
    if (failed.length > 0) {
      throw new HelperError(
        `${String(failed.length)} of ${String(REDACTION_CASES.length)} redaction cases failed`,
      );
    }
    process.stdout.write(`redaction selftest: ${String(REDACTION_CASES.length)} cases ok\n`);
  } else if (command === "redact" && args.length <= 1) {
    await redactStdin(args[0]);
  } else if (command === "versions") {
    versions(args);
  } else if (command === "summary" && two) {
    process.stdout.write(`${summary(args[0] as string, args[1] as string)}\n`);
  } else {
    throw new HelperError(
      command === undefined ? USAGE : `unknown command or arguments: ${command}`,
    );
  }
}

if (import.meta.main) {
  try {
    await main(process.argv.slice(2));
  } catch (error) {
    // Messages name files, variables and HTTP statuses, never secret values.
    process.stderr.write(
      `keycloak e2e: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = 1;
  }
}
