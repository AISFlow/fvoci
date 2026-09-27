import assert from "node:assert/strict";
import test from "node:test";
import {
  attempted,
  logoutWithPushDisconnect,
  PUSH_OWNER_KEY,
  readPushOwner,
  withTimeout,
  writePushOwner,
  boundTo,
  decodeKey,
  PermissionBlocked,
  pushBlocker,
  subscriptionBody,
} from "./push-subscription.ts";

const KEY = `B${"A".repeat(85)}E`; // 65 bytes: 0x04, zeros, 0x01

test("decodeKey turns unpadded base64url into raw bytes", () => {
  const bytes = decodeKey(KEY);
  assert.equal(bytes.length, 65);
  assert.equal(bytes[0], 4);
  assert.equal(bytes[64], 1);
  assert.deepEqual([...decodeKey("-_8")], [0xfb, 0xff]);
});

test("boundTo compares the subscription key with the live public key", () => {
  const same = decodeKey(KEY).buffer;
  assert.equal(boundTo(same, KEY), true);
  const other = decodeKey(KEY);
  other[64] = 2;
  assert.equal(boundTo(other.buffer, KEY), false);
  assert.equal(boundTo(new Uint8Array(64).buffer, KEY), false);
  assert.equal(boundTo(null, KEY), false);
});

test("subscriptionBody drops expirationTime for the strict API body", () => {
  assert.deepEqual(
    subscriptionBody({
      endpoint: "https://push.example.com/x",
      expirationTime: null,
      keys: { p256dh: "p", auth: "a" },
    }),
    { endpoint: "https://push.example.com/x", keys: { p256dh: "p", auth: "a" } },
  );
  assert.throws(() => subscriptionBody({ endpoint: "https://push.example.com/x" }));
});

test("blocker states: unsupported, unavailable after load only, then attempts", () => {
  const base = { supported: true, instanceLoaded: true, publicKey: KEY, attempted: null };
  assert.equal(pushBlocker({ ...base, supported: false }), "unsupported");
  assert.equal(pushBlocker({ ...base, publicKey: null }), "unavailable");
  assert.equal(pushBlocker({ ...base, instanceLoaded: false, publicKey: null }), null);
  assert.equal(pushBlocker(base), null);
  assert.equal(pushBlocker({ ...base, attempted: "blocked" }), "blocked");
  assert.equal(attempted(new PermissionBlocked()), "blocked");
  assert.equal(attempted(new Error("blocked")), "failed");
  assert.equal(attempted("x"), "failed");
});

function logoutHarness(options: {
  endpoint?: () => Promise<string | null>;
  post?: (endpoint: string | null) => Promise<{ ok: boolean }>;
  unsubscribe?: () => Promise<void>;
}) {
  const calls: string[] = [];
  const run = () =>
    logoutWithPushDisconnect({
      currentEndpoint: options.endpoint ?? (async () => "https://push.example/a"),
      postLogout: async (endpoint) => {
        calls.push(`post:${endpoint}`);
        return (options.post ?? (async () => ({ ok: true })))(endpoint);
      },
      isOk: (result) => result.ok,
      unsubscribe: async () => {
        calls.push("unsubscribe");
        await (options.unsubscribe ?? (async () => undefined))();
      },
      clearOwner: () => calls.push("clearOwner"),
    });
  return { calls, run };
}

test("logout reports this browser's endpoint, then unsubscribes", async () => {
  const { calls, run } = logoutHarness({});
  assert.deepEqual(await run(), { ok: true });
  assert.deepEqual(calls, ["post:https://push.example/a", "clearOwner", "unsubscribe"]);
});

test("logout still completes when the endpoint lookup or unsubscribe fails", async () => {
  const lookup = logoutHarness({ endpoint: async () => Promise.reject(new Error("sw")) });
  assert.deepEqual(await lookup.run(), { ok: true });
  assert.deepEqual(lookup.calls, ["post:null", "clearOwner", "unsubscribe"]);

  const unsub = logoutHarness({ unsubscribe: async () => Promise.reject(new Error("gone")) });
  assert.deepEqual(await unsub.run(), { ok: true });
  assert.deepEqual(unsub.calls, ["post:https://push.example/a", "clearOwner", "unsubscribe"]);
});

test("a failed logout keeps the signed-in user's subscription", async () => {
  const rejected = logoutHarness({ post: async () => ({ ok: false }) });
  assert.deepEqual(await rejected.run(), { ok: false });
  assert.deepEqual(rejected.calls, ["post:https://push.example/a"]);

  const offline = logoutHarness({ post: async () => Promise.reject(new TypeError("fetch")) });
  await assert.rejects(offline.run(), TypeError);
  assert.deepEqual(offline.calls, ["post:https://push.example/a"]);
});

test("owner marker round-trips and tolerates unavailable storage", () => {
  const map = new Map<string, string>();
  const storage = {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
    removeItem: (key: string) => void map.delete(key),
  };
  writePushOwner(storage, "user-a");
  assert.equal(map.get(PUSH_OWNER_KEY), "user-a");
  assert.equal(readPushOwner(storage), "user-a");
  writePushOwner(storage, null);
  assert.equal(readPushOwner(storage), null);
  const broken = {
    getItem: () => {
      throw new Error("denied");
    },
    setItem: () => {
      throw new Error("denied");
    },
    removeItem: () => {
      throw new Error("denied");
    },
  };
  assert.equal(readPushOwner(broken), null);
  writePushOwner(broken, "user-a");
  assert.equal(readPushOwner(null), null);
});

test("withTimeout falls back on a hung or failed promise", async () => {
  assert.equal(await withTimeout(new Promise<string>(() => undefined), 10, "fallback"), "fallback");
  assert.equal(await withTimeout(Promise.reject(new Error("x")), 1000, "fallback"), "fallback");
  assert.equal(await withTimeout(Promise.resolve("value"), 1000, "fallback"), "value");
});
