import { expect, test } from "bun:test";
import { once } from "node:events";
import { join } from "node:path";
import { Worker } from "node:worker_threads";
import { startLocalLibsqlProxy } from "./local-libsql.ts";
import { portClosed } from "../turso/ui-server.ts";

const tls = join(import.meta.dir, "../../tests/fixtures/remote-tls");
const token = "fixture-forwarding-only";
const proxyFor = (upstream: string, cert = join(tls, "server.pem")) =>
  startLocalLibsqlProxy(upstream, token, join(tls, "server.key"), cert);
const request = (origin: string, path = "/v3/pipeline", init: RequestInit = {}) =>
  fetch(origin + path, {
    method: "POST",
    headers: { authorization: "Bearer " + token },
    body: "fixture-body",
    ...init,
    tls: { rejectUnauthorized: false },
  });

test("TLS forwarding progresses while the parent's input verification blocks its event loop", async () => {
  // Upstream and client must both run outside the blocked parent. The shared
  // result is signalled only after the actual TLS response body is consumed.
  const result = new Int32Array(new SharedArrayBuffer(4));
  const source = `
    import { parentPort, workerData } from "node:worker_threads";
    const result = new Int32Array(workerData);
    const server = Bun.serve({hostname:"127.0.0.1",port:0,
      async fetch(request) { return new Response(await request.arrayBuffer()); }});
    parentPort.postMessage(server.url.origin);
    parentPort.once("message", async ({origin, token}) => {
      try {
        const response = await fetch(origin + "/v3/pipeline", {
          method:"POST", headers:{authorization:"Bearer " + token},
          body:"forwarded-with-parent-blocked", tls:{rejectUnauthorized:false},
        });
        const body = await response.text();
        Atomics.store(result, 0, response.status === 200 && body === "forwarded-with-parent-blocked" ? 1 : -1);
      } catch { Atomics.store(result, 0, -1); }
      Atomics.notify(result, 0);
      await server.stop(true);
      parentPort.close();
    });`;
  const client = new Worker(new URL("data:text/javascript," + encodeURIComponent(source)), {
    workerData: result.buffer,
  });
  const exited = once(client, "exit");
  let proxy: Awaited<ReturnType<typeof startLocalLibsqlProxy>> | undefined;
  try {
    const message: unknown[] = await once(client, "message");
    const upstream = message[0];
    proxy = await startLocalLibsqlProxy(
      upstream as string,
      token,
      join(tls, "server.key"),
      join(tls, "server.pem"),
    );
    client.postMessage({ origin: proxy.origin, token });
    // This is a synchronization barrier, not a delay to make a write pass.
    // A same-event-loop proxy cannot complete while this barrier is held.
    Atomics.wait(result, 0, 0, 1000);
    expect(Atomics.load(result, 0)).toBe(1);
    expect((await exited)[0]).toBe(0);
  } finally {
    await client.terminate();
    if (proxy) {
      await proxy.close();
      expect(await portClosed(proxy.origin)).toBe(true);
    }
  }
});

test("TLS proxy retains authentication, method and exact path refusals before forwarding", async () => {
  let forwarded = 0;
  const upstream = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch() {
      forwarded += 1;
      return new Response("unexpected");
    },
  });
  const proxy = await proxyFor(upstream.url.origin);
  try {
    for (const headers of [new Headers(), new Headers({ authorization: "Bearer wrong" })])
      expect((await request(proxy.origin, "/v3/pipeline", { headers })).status).toBe(401);
    for (const path of ["/", "/v3/cursor?target=other", "/v2/other"])
      expect((await request(proxy.origin, path)).status).toBe(400);
    expect(
      (await request(proxy.origin, "/v3/pipeline", { method: "GET", body: undefined })).status,
    ).toBe(400);
    expect(forwarded).toBe(0);
  } finally {
    await proxy.close();
    await upstream.stop(true);
  }
});

test("TLS proxy forwards the allowed body and response without following redirects or encoding", async () => {
  let mode = "echo";
  const seen: string[] = [];
  const upstream = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(incoming) {
      seen.push(new URL(incoming.url).pathname);
      expect(incoming.headers.get("accept-encoding")).toBe("identity");
      if (mode === "redirect")
        return new Response(null, { status: 302, headers: { location: "/other" } });
      if (mode === "encoded")
        return new Response("opaque", { headers: { "content-encoding": "fixture-encoding" } });
      return new Response(await incoming.arrayBuffer(), {
        status: 201,
        headers: { "x-fixture": "forwarded" },
      });
    },
  });
  const proxy = await proxyFor(upstream.url.origin);
  try {
    for (const path of ["/v2/pipeline", "/v3/pipeline", "/v3/cursor"]) {
      const response = await request(proxy.origin, path);
      expect(response.status).toBe(201);
      expect(response.headers.get("x-fixture")).toBe("forwarded");
      expect(await response.text()).toBe("fixture-body");
    }
    mode = "redirect";
    expect((await request(proxy.origin)).status).toBe(502);
    mode = "encoded";
    expect((await request(proxy.origin)).status).toBe(502);
    expect(seen).toEqual([
      "/v2/pipeline",
      "/v3/pipeline",
      "/v3/cursor",
      "/v3/pipeline",
      "/v3/pipeline",
    ]);
  } finally {
    await proxy.close();
    await upstream.stop(true);
  }
});

test("TLS proxy start failure is fixed-code and retires the failed worker", async () => {
  const failure = await proxyFor("http://127.0.0.1:1", join(tls, "missing-certificate")).then(
    () => "unexpected success",
    (error: unknown) => (error instanceof Error ? error.message : "unknown"),
  );
  expect(failure).toBe("TLS proxy worker start failed");
});

test.each(["terminate", "invalid-command"])(
  "TLS proxy rejects actual worker failure: %s",
  async (fault) => {
    const upstream = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: () => new Response("owned"),
    });
    const proxy = await proxyFor(upstream.url.origin);
    // Observe an actual thread exit/error, not a mocked event or a timer.
    const exited = new Promise<void>((resolve) => {
      proxy.worker.once("exit", () => {
        resolve();
      });
    });
    try {
      if (fault === "terminate") await proxy.worker.terminate();
      else proxy.worker.postMessage("invalid-command");
      await exited;
      const failure = await proxy.close().then(
        () => "unexpected success",
        (error: unknown) => (error instanceof Error ? error.message : "unknown"),
      );
      expect(failure).toBe("TLS proxy retirement failed");
      expect(await portClosed(proxy.origin)).toBe(true);
    } finally {
      await proxy.worker.terminate();
      await upstream.stop(true);
    }
  },
);

test("TLS proxy close cancels in-flight forwarding, observes clean worker exit and retires listener", async () => {
  let entered!: () => void;
  const incoming = new Promise<void>((resolve) => {
    entered = resolve;
  });
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const upstream = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch() {
      entered();
      await held;
      return new Response("late");
    },
  });
  const proxy = await proxyFor(upstream.url.origin);
  const exited = once(proxy.worker, "exit");
  const response = request(proxy.origin).then(
    () => "response",
    () => "closed",
  );
  try {
    await incoming;
    await proxy.close();
    expect((await exited)[0]).toBe(0);
    expect(await portClosed(proxy.origin)).toBe(true);
    expect(await response).toBe("closed");
  } finally {
    release();
    await proxy.worker.terminate();
    await upstream.stop(true);
  }
});
