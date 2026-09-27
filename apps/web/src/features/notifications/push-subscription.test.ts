import assert from "node:assert/strict";
import test from "node:test";
import {
  attempted,
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
