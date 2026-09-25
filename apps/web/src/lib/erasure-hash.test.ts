import assert from "node:assert/strict";
import test from "node:test";
import { erasureRecoveryHash, parseErasureHash } from "./erasure-hash.ts";

test("mail link fragment carries only the token", () => {
  assert.deepEqual(parseErasureHash("#token=abc_-1"), {
    token: "abc_-1",
    eraseAt: null,
    scheduled: false,
    mailSent: null,
  });
});

test("recovery hash round-trips the withdraw response", () => {
  const hash = erasureRecoveryHash({
    token: "tok",
    eraseAt: "2026-10-10T00:00:00Z",
    mailSent: false,
  });
  assert.deepEqual(parseErasureHash(`#${hash}`), {
    token: "tok",
    eraseAt: "2026-10-10T00:00:00Z",
    scheduled: true,
    mailSent: false,
  });
});

test("empty token and malformed deadline are dropped", () => {
  const parsed = parseErasureHash("token=&eraseAt=not-a-date&mailSent=x");
  assert.equal(parsed.token, null);
  assert.equal(parsed.eraseAt, null);
  assert.equal(parsed.mailSent, null);
});
