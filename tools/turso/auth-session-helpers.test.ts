import { expect, test } from "bun:test";
import type { Page, WebSocketRoute } from "@playwright/test";
import { activeRoom } from "../../apps/web/e2e-pending/auth-session-helpers.ts";

async function routedRoom() {
  let closed: (code?: number, reason?: string) => Promise<void> = () =>
    Promise.reject(new Error("Server close observer was not registered"));
  const forwarded: { code?: number; reason?: string }[] = [];
  const socket = {
    connectToServer: () => ({
      onClose(handler: typeof closed) {
        closed = handler;
      },
    }),
    close(options: { code?: number; reason?: string }) {
      forwarded.push(options);
      return Promise.resolve();
    },
  } as unknown as WebSocketRoute;
  const page = {
    async routeWebSocket(pattern: string, handler: (route: WebSocketRoute) => unknown) {
      expect(pattern).toBe("**/collab");
      await handler(socket);
    },
  } as unknown as Page;
  const actual = await activeRoom(page);
  return { actual, forwarded, close: (code?: number, reason?: string) => closed(code, reason) };
}

test("a code-less navigation closure is observed and forwarded without inventing revocation", async () => {
  const room = await routedRoom();
  await room.close(undefined, undefined);
  expect(room.actual.connections).toBe(1);
  expect(room.actual.closedCodes).toEqual([undefined]);
  expect(room.actual.closedCodes).not.toContain(1008);
  const projection = JSON.parse(JSON.stringify(room.actual)) as { closedCodes: (number | null)[] };
  expect(projection.closedCodes).toEqual([null]);
  expect(room.forwarded).toEqual([{ code: undefined, reason: undefined }]);
});

test("only an observed server 1008 supplies revocation evidence and forwards its actual reason", async () => {
  const room = await routedRoom();
  await room.close(1000, "normal departure");
  expect(room.actual.closedCodes).not.toContain(1008);
  await room.close(1008, "session revoked");
  expect(room.actual.closedCodes).toEqual([1000, 1008]);
  expect(room.forwarded).toEqual([
    { code: 1000, reason: "normal departure" },
    { code: 1008, reason: "session revoked" },
  ]);
});
