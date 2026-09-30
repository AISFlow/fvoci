import assert from "node:assert/strict";
import test from "node:test";
import { SETTINGS_CATALOG } from "./settings-catalog.ts";
import {
  assetDigest,
  assetPreviewSrc,
  leafValue,
  listFieldValue,
  numberFieldValue,
  previewText,
  settingLabel,
  settingMatches,
  textFieldValue,
  withLeaf,
  withMessageOverride,
} from "./settings-instance-model.ts";
import { adminActionMessage, daysUntil } from "./admin-users.ts";
import { legalPublishInput } from "./legal-publish.ts";
import { ProblemError } from "../../lib/api.ts";
import { issueMessage } from "../../lib/issue-message.ts";

await test("leafValue and withLeaf read and copy dotted leaves", () => {
  const doc = { share: { enabled: true, defaultExpiresDays: 7 } };
  assert.equal(leafValue(doc, "share.defaultExpiresDays"), 7);
  assert.equal(leafValue(doc, "share.missing.deeper"), undefined);
  assert.equal(leafValue(null, "share"), undefined);
  const next = withLeaf(doc, "share.defaultExpiresDays", 14) as typeof doc;
  assert.deepEqual(next, { share: { enabled: true, defaultExpiresDays: 14 } });
  assert.equal(doc.share.defaultExpiresDays, 7, "the source document is not changed");
  assert.deepEqual(withLeaf(undefined, "a.b", 1), { a: { b: 1 } });
});

await test("field values: emptied numbers are unset, emptied text is null, lists are trimmed and unique", () => {
  assert.equal(numberFieldValue(""), undefined);
  assert.equal(numberFieldValue("14"), 14);
  assert.equal(textFieldValue(""), null);
  assert.equal(textFieldValue(" x "), " x ");
  assert.deepEqual(listFieldValue(" a.example \n\nb.example\na.example\n"), [
    "a.example",
    "b.example",
  ]);
  assert.deepEqual(listFieldValue(""), []);
});

await test("message overrides: empty text removes the key, other keys stay", () => {
  const map = { "seed.status.todo": "할 일", junk: 1 };
  assert.deepEqual(withMessageOverride(map, "seed.status.done", "완료"), {
    "seed.status.todo": "할 일",
    "seed.status.done": "완료",
  });
  assert.deepEqual(withMessageOverride(map, "seed.status.todo", ""), {});
  assert.deepEqual(withMessageOverride(undefined, "a", "b"), { a: "b" });
  assert.equal(previewText("{{url}} 에서 {{minutes}}분"), "url 에서 minutes분");
});

await test("settings search matches key, Korean label, group and leaf names", () => {
  const entry = SETTINGS_CATALOG.attachmentTransfer;
  assert.equal(settingMatches("attachmentTransfer", entry, ""), true);
  assert.equal(settingMatches("attachmentTransfer", entry, "TRANSFER"), true);
  assert.equal(settingMatches("attachmentTransfer", entry, settingLabel(entry.labelKey)), true);
  assert.equal(settingMatches("attachmentTransfer", entry, "mode"), true);
  assert.equal(settingMatches("attachmentTransfer", entry, "없는설정"), false);
  assert.equal(settingLabel("not.a.catalog.key"), "not.a.catalog.key");
});

await test("asset previews are versioned by the upload digest", () => {
  assert.equal(
    assetDigest({ key: "k", sha256: "0123456789abcdef", mime: "image/png" }),
    "0123456789abcdef",
  );
  assert.equal(assetDigest(null), null);
  assert.equal(assetPreviewSrc("logo", "0123456789abcdef"), "/api/v1/branding/logo?v=0123456789ab");
});

await test("admin console messages and the erasure countdown", () => {
  assert.equal(
    adminActionMessage(new ProblemError(409, "last_instance_admin")),
    "마지막 인스턴스 관리자의 권한은 해제할 수 없습니다. 다른 관리자를 먼저 지정하세요.",
  );
  assert.equal(adminActionMessage(new Error("offline")), "연결을 확인하고 다시 시도해 주세요.");
  const now = Date.parse("2031-03-01T00:00:00Z");
  assert.equal(daysUntil("2031-03-15T00:00:00Z", now), 14);
  assert.equal(daysUntil("2031-03-01T00:00:01Z", now), 1);
  assert.equal(daysUntil("2031-02-01T00:00:00Z", now), 0);
});

await test("the legal publish form maps the date to midnight UTC and reports catalog keys", () => {
  const parsed = legalPublishInput.safeParse({
    kind: "terms",
    title: " 약관 ",
    bodyMarkdown: "본문",
    required: true,
    effectiveAt: "2026-01-01",
  });
  assert.ok(parsed.success);
  assert.equal(parsed.data.effectiveAt, "2026-01-01T00:00:00Z");
  assert.equal(parsed.data.title, "약관");
  const bad = legalPublishInput.safeParse({
    kind: "Terms!",
    title: "",
    bodyMarkdown: "",
    required: true,
    effectiveAt: "",
  });
  assert.equal(bad.success, false);
  const issue = bad.error.issues[0];
  assert.ok(issue, "invalid legal input reports an issue");
  assert.equal(issueMessage(issue.message), "입력을 확인해 주세요.");
});
