/**
 * `public/sw.js` is neither bundled nor typed, so the push display and click
 * routing contract is locked by running the real file (source `sw.test.ts`).
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const SOURCE = readFileSync(new URL("../../../public/sw.js", import.meta.url), "utf8");

const ORIGIN = "https://wiki.example.com";

interface FakeClient {
  url: string;
  focus: () => Promise<void>;
  navigate?: (url: string) => Promise<FakeClient | null>;
}

function loadWorker(windows: FakeClient[] = []) {
  const listeners = new Map<string, (event: unknown) => void>();
  const shown: { title: string; options: Record<string, unknown> }[] = [];
  const opened: string[] = [];
  const self = {
    addEventListener: (type: string, fn: (event: unknown) => void) => {
      listeners.set(type, fn);
    },
    skipWaiting: () => undefined,
    location: { origin: ORIGIN },
    registration: {
      showNotification: (title: string, options: Record<string, unknown>) => {
        shown.push({ title, options });
        return Promise.resolve();
      },
    },
    clients: {
      claim: () => Promise.resolve(),
      matchAll: () => Promise.resolve(windows),
      openWindow: (url: string) => {
        opened.push(url);
        return Promise.resolve(null);
      },
    },
  };
  new Function("self", SOURCE)(self);
  const fire = async (type: string, event: object): Promise<void> => {
    const pending: Promise<unknown>[] = [];
    listeners.get(type)?.({
      ...event,
      waitUntil: (p: Promise<unknown>) => pending.push(p),
    });
    await Promise.all(pending);
  };
  return { fire, shown, opened, listeners };
}

test("registers install/activate/push/click handlers", () => {
  const worker = loadWorker();
  assert.deepEqual([...worker.listeners.keys()].sort(), [
    "activate",
    "install",
    "notificationclick",
    "push",
  ]);
});

test("shows the push payload title, body and url", async () => {
  const worker = loadWorker();
  await worker.fire("push", {
    data: { json: () => ({ title: "홍길동", body: "새 댓글", url: "/w/acme/OPS-1" }) },
  });
  assert.deepEqual(worker.shown, [
    { title: "홍길동", options: { body: "새 댓글", data: { url: "/w/acme/OPS-1" } } },
  ]);
});

test("a broken payload still shows a notification", async () => {
  const worker = loadWorker();
  await worker.fire("push", {
    data: {
      json: () => {
        throw new SyntaxError("unexpected token");
      },
    },
  });
  assert.equal(worker.shown[0]?.title, "FVOCI");
  assert.deepEqual(worker.shown[0]?.options, { body: "", data: { url: "/" } });
});

test("focuses a tab already at the target url", async () => {
  const focused: string[] = [];
  const client = (url: string): FakeClient => ({
    url,
    focus: () => {
      focused.push(url);
      return Promise.resolve();
    },
  });
  const worker = loadWorker([client(`${ORIGIN}/w/acme`), client(`${ORIGIN}/w/acme/OPS-1`)]);
  await worker.fire("notificationclick", {
    notification: { close: () => undefined, data: { url: "/w/acme/OPS-1" } },
  });
  assert.deepEqual(focused, [`${ORIGIN}/w/acme/OPS-1`]);
  assert.deepEqual(worker.opened, []);
});

test("moves an open window instead of opening a tab", async () => {
  const moved: string[] = [];
  let focused = false;
  const open: FakeClient = {
    url: `${ORIGIN}/w/acme`,
    navigate: (url: string) => {
      moved.push(url);
      return Promise.resolve(null);
    },
    focus: () => {
      focused = true;
      return Promise.resolve();
    },
  };
  const worker = loadWorker([open]);
  await worker.fire("notificationclick", {
    notification: { close: () => undefined, data: { url: "/w/acme/OPS-1" } },
  });
  assert.deepEqual(moved, [`${ORIGIN}/w/acme/OPS-1`]);
  assert.equal(focused, true);
  assert.deepEqual(worker.opened, []);
});

test("opens an absolute url when no window is open", async () => {
  const worker = loadWorker();
  await worker.fire("notificationclick", {
    notification: { close: () => undefined, data: { url: "/w/acme/OPS-1" } },
  });
  assert.deepEqual(worker.opened, [`${ORIGIN}/w/acme/OPS-1`]);
});

test("a notification without url opens the root", async () => {
  const worker = loadWorker();
  await worker.fire("notificationclick", {
    notification: { close: () => undefined, data: {} },
  });
  assert.deepEqual(worker.opened, [`${ORIGIN}/`]);
});
