import assert from "node:assert/strict";
import test from "node:test";
import { nextReplyTarget, updateIsolatedDraft } from "./comment-drafts.ts";

test("reply draft updates leave the main compose draft unchanged", () => {
  const next = updateIsolatedDraft({ main: "본문 초안", reply: "" }, "reply", "답글 초안");
  assert.equal(next.main, "본문 초안");
  assert.equal(next.reply, "답글 초안");
});

test("reply target toggles per comment without sharing draft identity", () => {
  assert.equal(nextReplyTarget(null, "c1"), "c1");
  assert.equal(nextReplyTarget("c1", "c1"), null);
  assert.equal(nextReplyTarget("c1", "c2"), "c2");
});
