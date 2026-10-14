// Normal-main readiness and closure observations shared by the hosted and the
// orca-local server paths.
import { readFileSync } from "node:fs";
import http from "node:http";
import net from "node:net";
import { decodeUtf8, get, require, UiError } from "./ui-common.ts";
import { now, pause, poll, type Child, type Scope } from "./ui-processes.ts";
import { serverStartDiagnostic } from "./ui-start-diagnostic.ts";

/** Loopback GET; never through a proxy named by the environment. */
export function getJson(url: string, seconds: number): Promise<{ status: number; body: unknown }> {
  return new Promise((resolve, reject) => {
    const request = http.get(url, { timeout: seconds * 1000 }, (response) => {
      const chunks: Buffer[] = [];
      response.on("data", (chunk: Buffer) => chunks.push(chunk));
      response.on("error", reject);
      response.on("end", () => {
        const status = response.statusCode ?? 0;
        if (status >= 400) {
          reject(new Error("HTTP error status"));
          return;
        }
        try {
          resolve({ status, body: JSON.parse(decodeUtf8(Buffer.concat(chunks))) });
        } catch (error) {
          reject(error instanceof Error ? error : new Error("invalid response"));
        }
      });
    });
    request.on("timeout", () => request.destroy(new Error("HTTP timeout")));
    request.on("error", reject);
  });
}

export async function requireSetup(base: string, expected: boolean): Promise<void> {
  const response = await getJson(base + "/api/v1/setup", 10);
  require(response.status === 200 &&
    get(response.body, "needed") === expected, "UI_INITIALIZED_SETUP_CHANGED");
}

const listening = /fvoci-server listening on (http:\/\/127\.0\.0\.1:[0-9]+)/;
export async function awaitListening(
  logpath: string,
  child: Child,
  deadline: number,
  clock: () => number = now,
): Promise<string> {
  for (;;) {
    const match = listening.exec(readFileSync(logpath).toString("latin1"));
    if (match) return match[1] as string;
    require(poll(child) === null && clock() < deadline, "UI_SERVER_START_FAILED");
    await pause(20);
  }
}

export function publishStartDiagnostic(
  scope: Pick<Scope, "io">,
  child: Pick<Child, "exitCode" | "signalCode"> | null,
  logpath: string | null,
  started: number | null,
  deadline: number | null,
  clock: () => number = now,
) {
  const result = serverStartDiagnostic(
    child ? () => poll(child) : null,
    logpath,
    started,
    deadline,
    clock,
  );
  try {
    scope.io.output.out(JSON.stringify(result));
  } catch {
    // The original failure stays the result.
  }
  return result;
}

export async function portClosed(base: string): Promise<boolean> {
  const endpoint = /^https?:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})\/?$/.exec(base);
  require(endpoint !== null, "UI_PORT_CLOSURE_ENDPOINT_REFUSED");
  const port = Number(endpoint[1]);
  require(port <= 65535, "UI_PORT_CLOSURE_ENDPOINT_REFUSED");
  return new Promise((resolve, reject) => {
    const socket = net.connect({ host: "127.0.0.1", port });
    let settled = false;
    const finish = (closed: boolean | null) => {
      if (settled) return;
      settled = true;
      socket.setTimeout(0);
      socket.destroy();
      if (closed === null) reject(new UiError("UI_PORT_CLOSURE_UNCONFIRMED"));
      else resolve(closed);
    };
    socket.setTimeout(1000);
    socket.once("connect", () => {
      finish(false);
    });
    socket.once("timeout", () => {
      finish(null);
    });
    socket.once("error", (error: NodeJS.ErrnoException) => {
      finish(error.code === "ECONNREFUSED" ? true : null);
    });
  });
}
