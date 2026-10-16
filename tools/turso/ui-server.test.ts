import { afterEach, expect, mock, spyOn, test } from "bun:test";
import { EventEmitter } from "node:events";
import net from "node:net";
import { portClosed } from "./ui-server.ts";
import { failureOf } from "./ui-fakes.ts";

class SocketWitness extends EventEmitter {
  timeouts: number[] = [];
  destroyed = false;
  setTimeout(milliseconds: number) {
    this.timeouts.push(milliseconds);
    return this;
  }
  destroy() {
    this.destroyed = true;
    return this;
  }
}
afterEach(() => {
  mock.restore();
});

for (const event of ["timeout", "ECONNRESET", "EHOSTUNREACH", "EACCES"] as const) {
  test("port closure refuses unknown " + event, async () => {
    const socket = new SocketWitness();
    spyOn(net, "connect").mockReturnValue(socket as unknown as net.Socket);
    const result = portClosed("http://127.0.0.1:43121");
    if (event === "timeout") socket.emit("timeout");
    else socket.emit("error", Object.assign(new Error("private network cause"), { code: event }));
    expect(await failureOf(result)).toBe("UI_PORT_CLOSURE_UNCONFIRMED");
    expect(socket.timeouts[0]).toBe(1000);
    expect(socket.destroyed).toBe(true);
  });
}

test("port closure accepts only connection refusal", async () => {
  const socket = new SocketWitness();
  spyOn(net, "connect").mockReturnValue(socket as unknown as net.Socket);
  const result = portClosed("https://127.0.0.1:43121");
  socket.emit("error", Object.assign(new Error("refused"), { code: "ECONNREFUSED" }));
  expect(await result).toBe(true);
});

for (const base of [
  "http://other.invalid:43121",
  "http://localhost:43121",
  "http://127.0.0.1:0",
  "http://127.0.0.1:65536",
  "http://127.0.0.1:43121/private",
  "http://127.0.0.1:43121?target=other",
  "http://user@127.0.0.1:43121",
  "not a URL",
]) {
  test("port closure refuses invalid or unallocated endpoint " + String(base.length), async () => {
    const connect = spyOn(net, "connect");
    expect(await failureOf(portClosed(base))).toBe("UI_PORT_CLOSURE_ENDPOINT_REFUSED");
    expect(connect).not.toHaveBeenCalled();
  });
}

test("actual owned listener is closed after its process exit is observed", async () => {
  const child = Bun.spawn(
    [
      process.execPath,
      "--no-env-file",
      "--eval",
      'const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(){return new Response("owned");}});process.stdout.write(String(server.port)+"\\n");for await(const chunk of Bun.stdin.stream()){}await server.stop(true);process.exit(0);',
    ],
    {
      env: { PATH: process.env.PATH ?? "" },
      stdin: "pipe",
      stdout: "pipe",
      stderr: "ignore",
    },
  );
  const reader = child.stdout.getReader();
  let input = "";
  try {
    while (!input.includes("\n")) {
      const chunk = await reader.read();
      if (chunk.done) throw new Error("Owned listener exited before its port witness");
      input += new TextDecoder().decode(chunk.value);
    }
    expect(input).toMatch(/^[1-9][0-9]*\n$/);
    const base = "http://127.0.0.1:" + input.trim();
    expect(await portClosed(base)).toBe(false);
    await child.stdin.end();
    expect(await child.exited).toBe(0);
    expect(await portClosed(base)).toBe(true);
  } finally {
    reader.releaseLock();
    if (child.exitCode === null) child.kill();
    await child.exited;
  }
});
