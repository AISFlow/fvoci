import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { collabBadge, collabRefusalNote } from "./collab-badge.ts";

test("connected 배지는 persist ack 없이 「저장됨」을 쓰지 않는다", () => {
  assert.equal(collabBadge("connected", false).label, "doc.collab.connected");
  assert.equal(collabBadge("connected", false).tone, "live");
  assert.equal(collabBadge("connected", false, false).label, "doc.collab.connected");
  assert.equal(collabBadge("connected", false, true).label, "doc.collab.saved");
  assert.equal(collabBadge("connected", false, true).tone, "live");
  assert.equal(collabBadge("connected", true).label, "doc.collab.pending");
  assert.equal(collabBadge("connected", true).tone, "wait");
  assert.equal(collabBadge("connected", true, true).label, "doc.collab.pending");
});

test("연결 밖 상태는 미전송 변경·persist ack 과 무관하게 그 상태 배지다", () => {
  assert.equal(collabBadge("connecting", false).label, "doc.collab.connecting");
  assert.equal(collabBadge("connecting", false, true).label, "doc.collab.connecting");
  assert.equal(collabBadge("disconnected", true).label, "doc.collab.reconnecting");
  assert.equal(collabBadge("unauthorized", false).label, "doc.collab.unauthorized");
  assert.equal(collabBadge("unauthorized", false, true).tone, "danger");
});

test("방 거절은 연결됨이 아니라 거절 배지와 「불러오지 못함」 안내다", () => {
  assert.equal(collabBadge("busy", false).label, "doc.collab.busy");
  assert.equal(collabBadge("busy", false, true).tone, "danger");
  assert.equal(collabBadge("unavailable", true).label, "doc.collab.unavailable");
  assert.equal(collabRefusalNote("busy", false), "doc.collab.busyNote");
  assert.equal(collabRefusalNote("unavailable", false), "doc.collab.unavailableNote");
  for (const status of [
    "connected",
    "connecting",
    "disconnected",
    "unauthorized",
    undefined,
  ] as const) {
    assert.equal(collabRefusalNote(status, false), null);
  }
});

test("본문을 이미 불러왔으면 거절돼도 「불러오지 못함」 안내 없이 배지만 바뀐다", () => {
  assert.equal(collabRefusalNote("busy", true), null);
  assert.equal(collabRefusalNote("unavailable", true), null);
  assert.equal(collabBadge("busy", false).label, "doc.collab.busy");
});

const ko = JSON.parse(
  readFileSync(
    path.join(import.meta.dirname, "../../../../../packages/i18n/src/locales/ko.json"),
    "utf8",
  ),
) as Record<string, string>;

test("거절 안내는 편집 전송을 약속하지 않는다: 안내가 뜨는 동안 본문 편집기는 없다", () => {
  for (const key of ["doc.collab.busyNote", "doc.collab.unavailableNote"]) {
    const note = ko[key];
    assert.ok(note, key);
    assert.ok(note.includes("비어 있는 것이 아닙니다"), `${key} still says the body is not empty`);
    assert.equal(/편집|전송/.test(note), false, `${key} must not promise edits: ${note}`);
  }
});
