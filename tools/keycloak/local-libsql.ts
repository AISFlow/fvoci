// One owned official sqld and a standard TLS forwarding endpoint. This does
// not parse Hrana or SQL. It is a local fixture, never proof of hosted Turso.
import { randomBytes, timingSafeEqual } from "node:crypto";
import {
  chmodSync,
  mkdirSync,
  openSync,
  closeSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  rmSync,
} from "node:fs";
import { join } from "node:path";
import { sha } from "../selected-backend-ci/io.ts";
import { cleanEnv } from "../turso/ui-common.ts";
import {
  communicate,
  now,
  pause,
  poll,
  type Child,
  type UiProcesses,
} from "../turso/ui-processes.ts";
import { portClosed } from "../turso/ui-server.ts";
import { HelperError } from "./redact.ts";

export const SQLD_SHA256 = "0863c3fbe68ac9714bca2cec1330def7a0ba5e4a29f199bf60ef46fa0c95b895";
const require = (condition: unknown, message: string) => {
  if (!condition) throw new HelperError(message);
};

/** Port zero observed from this unreaped child's actual socket FDs. */
export async function ownedListener(child: Child): Promise<string> {
  const deadline = now() + 10;
  for (;;) {
    require(poll(child) === null, "sqld exited before listening");
    const sockets = new Set<string>();
    for (const fd of readdirSync(`/proc/${String(child.pid)}/fd`)) {
      try {
        const match = /^socket:\[([0-9]+)\]$/.exec(
          readlinkSync(`/proc/${String(child.pid)}/fd/${fd}`),
        );
        if (match) sockets.add(match[1] as string);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
    }
    const ports = readFileSync("/proc/net/tcp", "utf8")
      .split("\n")
      .slice(1)
      .map((line) => line.trim().split(/\s+/))
      .filter(
        (fields) =>
          fields[3] === "0A" && sockets.has(fields[9] ?? "") && fields[1]?.startsWith("0100007F:"),
      )
      .map((fields) => parseInt((fields[1] as string).slice(9), 16));
    require(ports.length <= 1, "sqld has ambiguous owned listeners");
    if (ports.length === 1) return "http://127.0.0.1:" + String(ports[0]);
    require(now() < deadline, "sqld listener not observed");
    await pause(20);
  }
}

export interface LocalLibsql {
  environment: Record<string, string>;
  certificateSha256: string;
  close(): Promise<void>;
}

/** Caller must already have admitted the exact sqld binary and output root. */
export async function startLocalLibsql(
  scope: UiProcesses,
  binary: string,
  directory: string,
): Promise<LocalLibsql> {
  require(sha(binary) === SQLD_SHA256, "official sqld digest mismatch");
  mkdirSync(directory, { mode: 0o700 });
  const key = join(directory, "tls.key"),
    cert = join(directory, "tls.crt");
  const fd = openSync(join(directory, "sqld.log"), "wx", 0o600);
  let child: Child | undefined;
  let proxy: ReturnType<typeof Bun.serve> | undefined;
  let upstream: string | undefined;
  const close = async () => {
    const problems: string[] = [];
    const origin = proxy?.url.origin;
    if (proxy) {
      try {
        await proxy.stop(true);
      } catch {
        problems.push("TLS endpoint retirement failed");
      }
    }
    if (child) {
      try {
        await scope.finish(child, true);
      } catch {
        problems.push("sqld retirement failed");
      }
    }
    closeSync(fd);
    for (const base of [origin, upstream])
      if (base && !(await portClosed(base))) problems.push("libsql fixture port remains open");
    rmSync(directory, { recursive: true });
    require(problems.length === 0, problems.join("; "));
  };
  try {
    // OpenSSL owns certificate/key generation and X.509 encoding; no trust
    // store is changed. Native children receive this exact CA file only.
    const generation = scope.spawn(
      [
        "openssl",
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-days",
        "1",
        "-subj",
        "/CN=fvoci-local-fixture",
        "-addext",
        "subjectAltName=IP:127.0.0.1",
        // Explicit server leaf; the host config may default to a CA profile.
        "-addext",
        "basicConstraints=critical,CA:FALSE",
        "-addext",
        "keyUsage=critical,digitalSignature,keyEncipherment",
        "-addext",
        "extendedKeyUsage=serverAuth",
        "-keyout",
        key,
        "-out",
        cert,
      ],
      "local-tls-certificate",
      { env: cleanEnv(), stdin: "ignore", stdout: "pipe", stderr: "ignore" },
    );
    try {
      await communicate(generation, "", 30);
      require((await scope.finish(generation)) === 0, "local TLS certificate generation failed");
    } finally {
      if (poll(generation) === null) await scope.finish(generation, true);
    }
    chmodSync(key, 0o600);
    chmodSync(cert, 0o600);
    child = scope.spawn(
      [
        binary,
        "--db-path",
        join(directory, "database.sqld"),
        "--http-listen-addr",
        "127.0.0.1:0",
        "--disable-metrics",
        "--no-welcome",
      ],
      "official-sqld",
      {
        env: { ...cleanEnv(), RUST_LOG: "libsql_server=info" },
        stdin: "ignore",
        stdout: fd,
        stderr: fd,
      },
    );
    upstream = await ownedListener(child);
    const target = upstream;
    const token = randomBytes(32).toString("hex");
    const expected = Buffer.from("Bearer " + token);
    proxy = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      tls: { key: readFileSync(key), cert: readFileSync(cert) },
      async fetch(request) {
        const supplied = Buffer.from(request.headers.get("authorization") ?? "");
        if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected))
          return new Response(null, { status: 401 });
        const path = new URL(request.url);
        if (
          path.hostname !== "127.0.0.1" ||
          path.search !== "" ||
          request.method !== "POST" ||
          !["/v2/pipeline", "/v3/pipeline", "/v3/cursor"].includes(path.pathname)
        )
          return new Response(null, { status: 400 });
        const headers = new Headers(request.headers);
        headers.delete("host");
        headers.delete("content-length");
        headers.set("accept-encoding", "identity");
        const response = await fetch(target + path.pathname, {
          method: "POST",
          headers,
          body: await request.arrayBuffer(),
          redirect: "error",
        });
        if (
          response.headers.has("content-encoding") &&
          response.headers.get("content-encoding") !== "identity"
        )
          return new Response(null, { status: 502 });
        const returned = new Headers(response.headers);
        returned.delete("transfer-encoding");
        return new Response(response.body, { status: response.status, headers: returned });
      },
      error() {
        return new Response(null, { status: 502 });
      },
    });
    return {
      environment: {
        FVOCI_LIBSQL_URL: proxy.url.origin,
        FVOCI_LIBSQL_AUTH_TOKEN: token,
        SSL_CERT_FILE: cert,
      },
      certificateSha256: sha(cert),
      close,
    };
  } catch (error) {
    try {
      await close();
    } catch (cleanup) {
      throw new AggregateError([error, cleanup], "local libSQL start and cleanup failed", {
        cause: cleanup,
      });
    }
    throw error;
  }
}
