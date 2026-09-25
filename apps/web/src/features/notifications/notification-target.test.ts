import assert from "node:assert/strict";
import test from "node:test";
import { notificationMessage } from "@fvoci/i18n";
import { notificationHref, payloadRecord } from "./notification-target.ts";

test("assignment message includes task number and title", () => {
  assert.equal(
    notificationMessage({
      verb: "task.updated",
      payload: { number: 3, title: "알림 수신 확인 태스크" },
    }),
    "태스크 #3 「알림 수신 확인 태스크」의 담당자로 지정되었습니다",
  );
});

test("notificationHref uses the display id path", () => {
  assert.equal(
    notificationHref("acme", {
      id: "00000000-0000-0000-0000-000000000001",
      workspaceId: "00000000-0000-0000-0000-000000000002",
      eventId: "00000000-0000-0000-0000-000000000003",
      verb: "task.updated",
      actorUserId: null,
      actorGivenName: null,
      actorFamilyName: null,
      targetType: "task",
      targetId: "00000000-0000-0000-0000-000000000004",
      displayId: "NTF-2",
      payload: {},
      readAt: null,
      archivedAt: null,
      createdAt: "2026-01-01T00:00:00Z",
    }),
    "/w/acme/NTF-2",
  );
  assert.equal(payloadRecord(null).title, undefined);
});
