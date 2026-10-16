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
import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";
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

export interface LocalLibsqlProxy {
  origin: string;
  worker: Worker;
  close(): Promise<void>;
}

/** Only forwarding lives here; the caller still owns sqld and its TLS files. */
export async function startLocalLibsqlProxy(
  target: string,
  token: string,
  key: string,
  cert: string,
): Promise<LocalLibsqlProxy> {
  // Full physical-input hashing intentionally remains synchronous. Keep TLS
  // forwarding on another event loop so an open sqld writer can still finish.
  const worker = new Worker(new URL(import.meta.url), {
    workerData: { kind: "fvoci-local-libsql-proxy", target, token, key, cert },
    env: cleanEnv(),
    stdout: true,
    stderr: true,
  });
  // Worker diagnostics may contain private input paths or endpoints.
  worker.stdout.resume();
  worker.stderr.resume();
  let closing = false;
  let failed = false;
  worker.once("error", () => {
    failed = true;
  });
  const exited = new Promise<number>((resolve) => {
    worker.once("exit", (code) => {
      if (!closing) failed = true;
      resolve(code);
    });
  });
  let deadline: ReturnType<typeof setTimeout> | undefined;
  let ready!: (message: unknown) => void;
  let refused!: () => void;
  let origin: string;
  try {
    origin = await new Promise<string>((resolve, reject) => {
      refused = () => {
        reject(new HelperError("TLS proxy worker start failed"));
      };
      ready = (message) => {
        try {
          const url = new URL(String(message));
          require(url.protocol === "https:" &&
            url.hostname === "127.0.0.1" &&
            url.port !== "" &&
            url.port !== "0" &&
            url.pathname === "/" &&
            url.search === "" &&
            url.hash === "" &&
            url.username === "" &&
            url.password === "", "TLS proxy listener refused");
          resolve(url.origin);
        } catch {
          refused();
        }
      };
      worker.once("message", ready);
      worker.once("error", refused);
      worker.once("exit", refused);
      // Same readiness bound as the owned sqld listener; no retries.
      deadline = setTimeout(refused, 10_000);
    });
  } catch {
    closing = true;
    await worker.terminate();
    await exited;
    throw new HelperError("TLS proxy worker start failed");
  } finally {
    clearTimeout(deadline);
    worker.off("message", ready);
    worker.off("error", refused);
    worker.off("exit", refused);
  }
  let closed: Promise<void> | undefined;
  const close = () =>
    (closed ??= (async () => {
      closing = true;
      try {
        worker.postMessage("close");
        const code = await Promise.race([
          exited,
          new Promise<never>((_resolve, reject) => {
            deadline = setTimeout(() => {
              reject(new HelperError("TLS proxy retirement failed"));
            }, 10_000);
          }),
        ]);
        require(code === 0 && !failed, "TLS proxy retirement failed");
      } finally {
        clearTimeout(deadline);
        await worker.terminate();
        await exited;
        require(await portClosed(origin), "TLS proxy port remains open");
      }
    })());
  return { origin, worker, close };
}

function serveLocalLibsqlProxy(target: string, token: string, key: string, cert: string) {
  const expected = Buffer.from("Bearer " + token);
  const forwarding = new AbortController();
  const server = Bun.serve({
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
        signal: forwarding.signal,
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
    url: server.url,
    async stop() {
      forwarding.abort();
      await server.stop(true);
    },
  };
}

// The same registered source file is the dedicated worker entry. No extra
// CLI, registration list or general process manager is introduced.
if (
  !isMainThread &&
  (workerData as { kind?: unknown } | null)?.kind === "fvoci-local-libsql-proxy"
) {
  const input = workerData as { target: string; token: string; key: string; cert: string };
  try {
    const proxy = serveLocalLibsqlProxy(input.target, input.token, input.key, input.cert);
    require(parentPort !== null, "TLS proxy parent missing");
    parentPort?.postMessage(proxy.url.origin);
    const stop = () => {
      void proxy.stop().then(
        () => {
          parentPort?.close();
        },
        () => {
          setImmediate(() => {
            throw new HelperError("TLS proxy retirement failed");
          });
        },
      );
    };
    parentPort?.once("message", (message: unknown) => {
      require(message === "close", "TLS proxy command refused");
      stop();
    });
    parentPort?.once("close", stop);
  } catch {
    throw new HelperError("TLS proxy worker start failed");
  }
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
  let proxy: LocalLibsqlProxy | undefined;
  let upstream: string | undefined;
  const close = async () => {
    const problems: string[] = [];
    const origin = proxy?.origin;
    if (proxy) {
      try {
        await proxy.close();
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
    const token = randomBytes(32).toString("hex");
    proxy = await startLocalLibsqlProxy(upstream, token, key, cert);
    return {
      environment: {
        FVOCI_LIBSQL_URL: proxy.origin,
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
