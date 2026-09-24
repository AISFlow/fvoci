import assert from "node:assert/strict";
import test from "node:test";
import {
  documentResourcePath,
  isDocumentResourceUrl,
  isSuccessfulMatchingDocumentPatch,
  parseJsonBody,
  patchBodyMatches,
} from "./document-save-barrier.ts";

const ids = {
  workspaceId: "11111111-1111-4111-8111-111111111111",
  documentId: "22222222-2222-4222-8222-222222222222",
};

const resource = `http://127.0.0.1:5173${documentResourcePath(ids)}`;

test("matches the exact document resource path only", () => {
  assert.equal(isDocumentResourceUrl(resource, ids), true);
  assert.equal(
    isDocumentResourceUrl(`${resource}/ancestors`, ids),
    false,
  );
  assert.equal(
    isDocumentResourceUrl(
      resource.replace(ids.documentId, "33333333-3333-4333-8333-333333333333"),
      ids,
    ),
    false,
  );
});

test("patch body matches expected keys and treats icon null as a value", () => {
  assert.equal(patchBodyMatches({ title: "연구 노트" }, { title: "연구 노트" }), true);
  assert.equal(
    patchBodyMatches({ title: "연구 노트", version: 2 }, { title: "연구 노트" }),
    true,
  );
  assert.equal(patchBodyMatches({ icon: null }, { icon: null }), true);
  assert.equal(patchBodyMatches({ icon: "📚" }, { icon: null }), false);
  assert.equal(patchBodyMatches({ status: "draft" }, { status: "published" }), false);
  assert.equal(patchBodyMatches({ title: "연구 노트" }, { title: "연구 노트", icon: null }), false);
  assert.equal(patchBodyMatches(null, { title: "연구 노트" }), false);
  assert.deepEqual(parseJsonBody('{"icon":null}'), { icon: null });
  assert.equal(parseJsonBody("not-json"), undefined);
});

test("successful matching PATCH requires method, ok, resource, request and response body", () => {
  const expected = { title: "연구 노트" };
  const base = {
    method: "PATCH",
    url: resource,
    ok: true,
    workspaceId: ids.workspaceId,
    documentId: ids.documentId,
    requestBody: expected,
    responseBody: { id: ids.documentId, title: "연구 노트", icon: null, status: "draft" },
    expected,
  };
  assert.equal(isSuccessfulMatchingDocumentPatch(base), true);
  assert.equal(isSuccessfulMatchingDocumentPatch({ ...base, method: "GET" }), false);
  assert.equal(isSuccessfulMatchingDocumentPatch({ ...base, ok: false }), false);
  assert.equal(
    isSuccessfulMatchingDocumentPatch({
      ...base,
      requestBody: { title: "제목 없음" },
    }),
    false,
  );
  assert.equal(
    isSuccessfulMatchingDocumentPatch({
      ...base,
      responseBody: { title: "제목 없음" },
    }),
    false,
  );
});
