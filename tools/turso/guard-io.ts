// Process, git, file and network effects of the guard. Callers decide; these
// helpers only observe or execute.
import { createHash } from "node:crypto";
import { closeSync, openSync, readFileSync, readSync } from "node:fs";
import { request } from "node:https";
import { constants } from "node:os";
import { AdmissionError, API_ROOT, reject } from "./guard-policy.ts";
import { resolved, sha } from "../selected-backend-ci/io.ts";
import { dumps, isRecord } from "./python-compat.ts";

/** Process-local filtering, not a change to user/global Git configuration. */
function gitEnv(): Record<string, string> {
  return { PATH: process.env.PATH ?? "", GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" };
}

export function checkoutSha(): string {
  const result = Bun.spawnSync(["git", "rev-parse", "HEAD"], {
    env: gitEnv(),
    stdout: "pipe",
    stderr: "ignore",
  });
  if (result.exitCode !== 0) throw new Error("git rev-parse failed");
  return result.stdout.toString().trim();
}

/** Digest of every tracked file's working-tree bytes; refuses a dirty tree. */
export function sourceDigest(): string {
  const diff = Bun.spawnSync(["git", "diff", "--quiet", "HEAD"], {
    env: gitEnv(),
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  if (diff.exitCode !== 0) reject("SOURCE_CHANGED");
  const listed = Bun.spawnSync(["git", "ls-files", "-z"], {
    env: gitEnv(),
    stdout: "pipe",
    stderr: "inherit",
  });
  if (listed.exitCode !== 0) throw new Error("git ls-files failed");
  const strict = new TextDecoder("utf-8", { fatal: true });
  const hashes = Object.create(null) as Record<string, string>;
  for (const raw of listed.stdout.toString("latin1").split("\0")) {
    if (raw === "") continue;
    const name = strict.decode(Buffer.from(raw, "latin1"));
    hashes[name] = createHash("sha256").update(readFileSync(name)).digest("hex");
  }
  return createHash("sha256").update(dumps(hashes, true)).digest("hex");
}

/** Streaming sha256 of a file (the frozen libtest binary is large). */
export const fileDigest = sha;

export function startsWithElf(path: string): boolean {
  const fd = openSync(path, "r");
  try {
    const magic = Buffer.alloc(4);
    const read = readSync(fd, magic, 0, 4, 0);
    return read === 4 && magic.equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]));
  } finally {
    closeSync(fd);
  }
}

/** Path.resolve(strict=False): each existing component through its symlink. */
export const resolveLoose = resolved;

export interface ChildResult {
  /** Exit code, or the negated signal number. */
  status: number;
  /** stdout and stderr interleaved in write order, decoded with U+FFFD. */
  output: string;
}

export type RunChild = (argv: string[], env: Record<string, string>) => Promise<ChildResult>;

/**
 * Runs a libtest child with stderr joined onto its stdout pipe, captured in
 * memory only. /bin/sh only performs the dup; env(1) then drops the variables a
 * shell may add (PWD, OLDPWD, SHLVL, _) and execs the binary, so the child
 * sees exactly the allowlisted environment (same pid as env, no shell left).
 */
export const runChild: RunChild = async (argv, env) => {
  const child = Bun.spawn(
    ["/bin/sh", "-c", 'exec /usr/bin/env -u PWD -u OLDPWD -u SHLVL -u _ "$@" 2>&1', "sh", ...argv],
    {
      env,
      stdin: "inherit",
      stdout: "pipe",
      stderr: "inherit",
    },
  );
  const bytes = new Uint8Array(await new Response(child.stdout).arrayBuffer());
  await child.exited;
  const { exitCode, signalCode } = child;
  const status = exitCode ?? -(signalCode ? constants.signals[signalCode] : 1);
  return { status, output: new TextDecoder("utf-8", { ignoreBOM: true }).decode(bytes) };
};

export interface EnvironmentMetadata {
  value: Record<string, unknown>;
  /** The root object's id token when it is an integer literal (5.0 is not), else null. */
  idText: string | null;
}

const METADATA_CAP = 262144;

/** Parses JSON and keeps the root object's integer "id" token, digits exact. */
export function parseEnvironmentBody(body: Uint8Array): EnvironmentMetadata {
  const text = new TextDecoder("utf-8", { fatal: true }).decode(body);
  const integerIds = new WeakMap<object, string | null>();
  const value: unknown = JSON.parse(
    text,
    function (this: object, key, entry, context?: { source?: string }) {
      if (key === "id")
        integerIds.set(
          this,
          context?.source !== undefined && /^-?(?:0|[1-9][0-9]*)$/.test(context.source)
            ? context.source
            : null,
        );
      return entry as unknown;
    },
  );
  if (!isRecord(value)) throw new TypeError("metadata is not an object");
  return { value, idText: integerIds.get(value) ?? null };
}

/**
 * Anonymous public read of the fixed Environment resource: no credential,
 * no proxy, no redirect, bounded body. Every failure is one fixed code.
 */
export function fetchEnvironmentMetadata(): Promise<EnvironmentMetadata> {
  return new Promise((done, fail) => {
    let settled = false;
    const refuse = () => {
      if (settled) return;
      settled = true;
      fail(new AdmissionError("ENVIRONMENT_METADATA_UNAVAILABLE"));
    };
    const req = request(
      API_ROOT,
      {
        method: "GET",
        headers: {
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2026-03-10",
          "User-Agent": "fvoci-turso-guard",
        },
      },
      (response) => {
        if (response.statusCode !== 200) {
          response.destroy();
          refuse();
          return;
        }
        const chunks: Buffer[] = [];
        let size = 0;
        response.on("data", (chunk: Buffer) => {
          size += chunk.length;
          if (size > METADATA_CAP) {
            response.destroy();
            refuse();
            return;
          }
          chunks.push(chunk);
        });
        response.on("error", refuse);
        response.on("end", () => {
          try {
            const parsed = parseEnvironmentBody(Buffer.concat(chunks));
            if (!settled) {
              settled = true;
              done(parsed);
            }
          } catch {
            refuse();
          }
        });
      },
    );
    req.setTimeout(15000, () => {
      req.destroy();
      refuse();
    });
    req.on("error", refuse);
    req.end();
  });
}
