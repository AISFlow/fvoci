import assert from "node:assert/strict";
import test from "node:test";
import { UUID_RE, UUID_SOURCE, uuid } from "../src/uuid.ts";

test("uuid is the source RFC 4122/9562 z.uuid() contract", () => {
  const canonical = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  assert.equal(uuid.safeParse(canonical).success, true);
  assert.equal(UUID_RE.test(canonical), true);
  assert.equal(UUID_SOURCE, "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}");
  assert.equal(uuid.safeParse("not-a-uuid").success, false);
  assert.equal(UUID_RE.test("not-a-uuid"), false);
  assert.equal(uuid.safeParse("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa").success, false);
});

test("UUID_RE accepts the same 8-4-4-4-12 hex form as z.uuid()", () => {
  const samples = [
    "00000000-0000-4000-8000-000000000000",
    "ffffffff-ffff-4fff-bfff-ffffffffffff",
    "01a01f00-0000-7000-8000-000000000001",
  ];
  for (const value of samples) {
    assert.equal(UUID_RE.test(value), uuid.safeParse(value).success, value);
  }
});
