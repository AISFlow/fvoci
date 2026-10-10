import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { EventEmitter } from "node:events";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type { ClientRequest, IncomingMessage } from "node:http";
import type { RequestOptions } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  fetchEnvironmentMetadata,
  gitEnv,
  type EnvironmentMetadata,
  type HttpsRequest,
} from "./guard-io.ts";
import { AdmissionError, API_ROOT } from "./guard-policy.ts";
import { dumps } from "./python-compat.ts";

// Offline doubles for the one HTTPS call: no socket is ever opened, and the
// real node:https request is only the default when no double is passed.
class FakeRequest extends EventEmitter {
  timeoutMs: number | undefined;
  onTimeout: (() => void) | undefined;
  ended = false;
  destroyed = false;
  setTimeout(ms: number, callback: () => void): this {
    this.timeoutMs = ms;
    this.onTimeout = callback;
    return this;
  }
  end(): this {
    this.ended = true;
    return this;
  }
  destroy(): this {
    this.destroyed = true;
    return this;
  }
}

class FakeResponse extends EventEmitter {
  destroyed = false;
  constructor(
    readonly statusCode: number,
    readonly headers: Record<string, string> = {},
  ) {
    super();
  }
  destroy(): this {
    this.destroyed = true;
    return this;
  }
}

interface Sent {
  url: string;
  options: RequestOptions;
  request: FakeRequest;
  response?: FakeResponse;
}

type Script = (
  sent: Sent,
  respond: (response: FakeResponse, chunks?: Uint8Array[]) => void,
) => void;

function offline(script: Script): { send: HttpsRequest; calls: Sent[] } {
  const calls: Sent[] = [];
  const send: HttpsRequest = (url, options, callback) => {
    const request = new FakeRequest();
    const sent: Sent = { url, options, request };
    calls.push(sent);
    queueMicrotask(() => {
      script(sent, (response, chunks = []) => {
        sent.response = response;
        callback(response as unknown as IncomingMessage);
        for (const chunk of chunks) {
          if (response.destroyed) return;
          response.emit("data", Buffer.from(chunk));
        }
        if (!response.destroyed) response.emit("end");
      });
    });
    return request as unknown as ClientRequest;
  };
  return { send, calls };
}

const encode = (text: string) => new TextEncoder().encode(text);
const VALID = '{"name":"fvoci-turso-test","id":123,"deployment_branch_policy":null}';

async function unavailable(promise: Promise<EnvironmentMetadata>): Promise<void> {
  let caught: unknown;
  try {
    await promise;
  } catch (error) {
    caught = error;
  }
  // The fixed code only: no status, header, body or transport error text.
  expect(caught).toBeInstanceOf(AdmissionError);
  expect((caught as Error).message).toBe("ENVIRONMENT_METADATA_UNAVAILABLE");
}

describe("Environment metadata request boundary", () => {
  test("one anonymous GET of the fixed resource with a 15 s timeout and no redirect", async () => {
    const { send, calls } = offline((_sent, respond) => {
      respond(new FakeResponse(200), [encode(VALID)]);
    });
    const metadata = await fetchEnvironmentMetadata(send);
    expect(metadata.idText).toBe("123");
    expect(metadata.value.name).toBe("fvoci-turso-test");
    expect(calls.length).toBe(1);
    const [sent] = calls;
    expect(sent?.url).toBe(API_ROOT);
    // The exact options: any added method, header (Authorization, Cookie,
    // token), agent or redirect setting changes this object.
    expect(sent?.options).toEqual({
      method: "GET",
      headers: {
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2026-03-10",
        "User-Agent": "fvoci-turso-guard",
      },
    });
    const names = Object.keys(sent?.options.headers ?? {}).map((name) => name.toLowerCase());
    for (const credential of ["authorization", "cookie", "proxy-authorization"])
      expect(names).not.toContain(credential);
    expect(sent?.request.timeoutMs).toBe(15000);
    expect(sent?.request.ended).toBe(true);
  });

  test("a redirect is refused, never followed", async () => {
    for (const status of [301, 302, 307, 308]) {
      const { send, calls } = offline((_sent, respond) => {
        respond(new FakeResponse(status, { location: "https://wrong.example/FAKE_PRIVATE_BODY" }), [
          encode(VALID),
        ]);
      });
      await unavailable(fetchEnvironmentMetadata(send));
      expect(calls.length).toBe(1);
      expect(calls[0]?.response?.destroyed).toBe(true);
    }
  });

  test("non-200 status, oversize body, invalid UTF-8, a leading BOM and non-object JSON map to the fixed code", async () => {
    const cases: [number, Uint8Array[]][] = [
      [404, [encode('{"message":"FAKE_PRIVATE_BODY"}')]],
      [500, [encode(VALID)]],
      // Valid JSON one byte over the cap, split across chunks: only the cap refuses it.
      [200, [encode(VALID), encode(" ".repeat(262145 - VALID.length))]],
      [200, [new Uint8Array([0x7b, 0xff, 0x7d])]],
      // RFC 8259 JSON has no BOM; refused even though json.loads(bytes) would skip it.
      [200, [new Uint8Array([0xef, 0xbb, 0xbf]), encode(VALID)]],
      [200, [encode('{"FAKE_PRIVATE_BODY":')]],
      [200, [encode("[1]")]],
      [200, []],
    ];
    for (const [status, chunks] of cases) {
      const { send } = offline((_sent, respond) => {
        respond(new FakeResponse(status), chunks);
      });
      await unavailable(fetchEnvironmentMetadata(send));
    }
    // Exactly at the cap is still read.
    const padded = VALID + " ".repeat(262144 - VALID.length);
    const { send } = offline((_sent, respond) => {
      respond(new FakeResponse(200), [encode(padded)]);
    });
    expect((await fetchEnvironmentMetadata(send)).idText).toBe("123");
  });

  test("request, response and timeout errors map to the fixed code without echo", async () => {
    const requestError = offline((sent) => {
      sent.request.emit("error", new Error("FAKE_PRIVATE_BODY getaddrinfo"));
    });
    await unavailable(fetchEnvironmentMetadata(requestError.send));

    const responseError = offline((_sent, respond) => {
      const response = new FakeResponse(200);
      respond(response, [encode('{"name":')]);
      response.emit("error", new Error("FAKE_PRIVATE_BODY reset"));
    });
    // The stream errored after a partial body; end never parses as success.
    const partial = offline((_sent, respond) => {
      const response = new FakeResponse(200);
      response.on("data", () => {
        response.emit("error", new Error("FAKE_PRIVATE_BODY reset"));
      });
      respond(response, [encode(VALID)]);
    });
    await unavailable(fetchEnvironmentMetadata(responseError.send));
    await unavailable(fetchEnvironmentMetadata(partial.send));

    const timedOut = offline((sent) => {
      sent.request.onTimeout?.();
    });
    await unavailable(fetchEnvironmentMetadata(timedOut.send));
    expect(timedOut.calls[0]?.request.destroyed).toBe(true);
  });
});

describe("source digest", () => {
  test("a tracked name with a leading U+FEFF is its own file", () => {
    const repo = mkdtempSync(join(tmpdir(), "guard-digest-"));
    const env = gitEnv();
    const git = (...args: string[]) => {
      const result = Bun.spawnSync(["git", "-c", "user.name=t", "-c", "user.email=t@t", ...args], {
        cwd: repo,
        env,
        stdout: "ignore",
        stderr: "pipe",
      });
      expect(result.exitCode, result.stderr.toString()).toBe(0);
    };
    const module = JSON.stringify(join(import.meta.dir, "guard-io.ts"));
    const digest = () => {
      const result = Bun.spawnSync(
        [process.execPath, "-e", `console.log((await import(${module})).sourceDigest())`],
        { cwd: repo, env, stdout: "pipe", stderr: "pipe" },
      );
      expect(result.exitCode, result.stderr.toString()).toBe(0);
      return result.stdout.toString().trim();
    };
    const sha256 = (text: string) => createHash("sha256").update(text).digest("hex");
    try {
      git("init", "-q");
      writeFileSync(join(repo, "a"), "plain");
      writeFileSync(join(repo, "\ufeffa"), "first");
      git("add", "-A");
      git("commit", "-qm", "one");
      const expected = createHash("sha256")
        .update(dumps({ a: sha256("plain"), "\ufeffa": sha256("first") }, true))
        .digest("hex");
      expect(digest()).toBe(expected);
      writeFileSync(join(repo, "\ufeffa"), "second");
      git("commit", "-qam", "two");
      expect(digest()).not.toBe(expected);
    } finally {
      rmSync(repo, { recursive: true, force: true });
    }
  });
});
