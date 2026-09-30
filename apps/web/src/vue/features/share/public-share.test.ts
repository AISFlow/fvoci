import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { t } from "@fvoci/i18n";
import { ProblemError } from "@/lib/api";
import {
  sharePublicBodyQuery,
  sharePublicMetaQuery,
  sharePublicTreeQuery,
} from "@/lib/queries/share";
import { hardenShareFragmentHtml } from "./harden-share-html.ts";
import { failMessage } from "./public-share-fail.ts";
import { publicShareSearchQuery } from "./public-share-search.ts";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

test("failMessage: 404 is expired, known title, else share failed or network", () => {
  assert.equal(failMessage(new ProblemError(404)), t("share.expired"));
  assert.equal(failMessage(new ProblemError(404, "not_found")), t("share.expired"));
  const known = new ProblemError(403, "forbidden");
  assert.equal(known.titleKnown, true);
  assert.equal(failMessage(known), known.title);
  assert.equal(failMessage(new ProblemError(500)), t("error.share.failed"));
  assert.equal(failMessage(new Error("offline")), t("error.network"));
});

test("tree and body queries stay disabled until enabled is true", () => {
  assert.equal(sharePublicTreeQuery("tok", false).enabled, false);
  assert.equal(sharePublicTreeQuery("tok", true).enabled, true);
  assert.equal(sharePublicBodyQuery("tok", null, false).enabled, false);
  assert.equal(sharePublicBodyQuery("tok", "doc-1", false).enabled, false);
  assert.equal(sharePublicBodyQuery("tok", null, true).enabled, true);
  assert.equal(sharePublicMetaQuery("tok").enabled, undefined);
});

test("the Vue public share page is an anonymous reader of /api/v1/share/{token}", () => {
  const page = source("../../pages/PublicSharePage.vue");
  assert.match(page, /sharePublicMetaQuery/);
  assert.match(page, /sharePublicTreeQuery\(token\.value, meta\.isSuccess\.value\)/);
  assert.match(
    page,
    /sharePublicBodyQuery\(token\.value, selectedDocumentId\.value, meta\.isSuccess\.value\)/,
  );
  assert.match(page, /failMessage/);
  assert.match(page, /from ["']@\/lib\/queries\/share["']/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
  assert.doesNotMatch(page, /meQuery|setupStatusQuery|useWorkspaceSession|SetupGuard/);
  assert.doesNotMatch(page, /useCollabRoom|WikiDocumentView|FvociEditor/);
  assert.doesNotMatch(page, /from ["']@fvoci\/editor/);
  assert.doesNotMatch(page, /shareAttachmentQuery/);
  assert.doesNotMatch(page, /RouterLink/);
});

test("the public share view reuses share.css and keeps tree select and body retry", () => {
  const view = source("PublicShareView.vue");
  assert.match(view, /shareTreeRoots/);
  assert.match(view, /downloadSharePdf/);
  assert.match(view, /emit\('selectDocument'/);
  assert.match(view, /emit\('retryBody'/);
  assert.match(view, /t\(['"]load.retry['"]\)/);
  assert.doesNotMatch(view, /RouterLink/);
  assert.doesNotMatch(view, /from ["']react["']/);

  const page = source("../../pages/PublicSharePage.vue");
  assert.match(page, /@\/features\/share\/share\.css/);
  assert.match(page, /onSelectDocument/);
  assert.match(page, /body\.refetch\(\)/);
});

test("hardenShareFragmentHtml strips unsafe hrefs before v-html", () => {
  const out = hardenShareFragmentHtml(
    `<p><a href="javascript:alert(1)">x</a><a href="https://ok.example/a">y</a></p>`,
  );
  assert.doesNotMatch(
    hardenShareFragmentHtml(`<a href='javascript:alert(1)'>x</a>`),
    /javascript:/i,
  );
  assert.match(out, /https:\/\/ok\.example\/a/);
  assert.match(out, /target="blank"|target="_blank"/);
  assert.match(out, /noopener/);
  const body = source("ShareBodyView.vue");
  assert.match(body, /hardenShareFragmentHtml/);
  assert.match(body, /v-html="hardened"/);
  assert.doesNotMatch(body, /watchPostEffect/);
});

test("share attachment view is not this page", () => {
  const page = source("../../pages/PublicSharePage.vue");
  assert.doesNotMatch(page, /shareAttachmentQuery|ShareAttachmentViewPage/);
  const branch = source("PublicTreeBranch.vue");
  assert.doesNotMatch(branch, /shareAttachment/);
});

test("anonymous search requires a bounded nonempty query and does not retry denial or throttling", () => {
  assert.equal(publicShareSearchQuery("", "word").enabled, false);
  assert.equal(publicShareSearchQuery("token", "  ").enabled, false);
  assert.equal(publicShareSearchQuery("token", "x".repeat(201)).enabled, false);
  const query = publicShareSearchQuery("token", "한글 ✅");
  assert.equal(query.enabled, true);
  assert.deepEqual(query.queryKey, ["share-search", "token", "한글 ✅"]);
  assert.equal(typeof query.retry, "function");
  const retry = query.retry as (count: number, error: Error) => boolean;
  assert.equal(retry(0, new ProblemError(404)), false);
  assert.equal(retry(0, new ProblemError(429)), false);
  assert.equal(retry(0, new Error("offline")), true);
  assert.equal(retry(2, new Error("offline")), false);
});
