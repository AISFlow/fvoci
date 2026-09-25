import assert from "node:assert/strict";
import test from "node:test";
import {
  EMPTY_VIEW_QUERY,
  addCustomFilter,
  encodeViewQueryParam,
  isEmptyViewQuery,
  normalizeViewQuery,
  parseViewQueryParam,
  patchViewFilter,
  readPrimarySort,
  removeCustomFilter,
  setPrimarySort,
  viewQueriesEqual,
} from "./view-query.ts";

test("normalizeViewQuery drops empty values and fixes key order", () => {
  const normalized = normalizeViewQuery({
    sort: [{ direction: "desc", field: "due" }],
    filters: { title: "  bug  ", openOnly: false, statusId: "", type: "task" },
  });
  assert.deepEqual(normalized, {
    filters: { type: "task", title: "bug" },
    sort: [{ field: "due", direction: "desc" }],
  });
  assert.equal(
    JSON.stringify(normalized),
    '{"filters":{"type":"task","title":"bug"},"sort":[{"field":"due","direction":"desc"}]}',
  );
});

test("normalizeViewQuery rejects shapes the server rejects", () => {
  assert.equal(normalizeViewQuery({ filters: [] }), null);
  assert.equal(normalizeViewQuery({ sort: [{ field: "due", direction: "up" }] }), null);
  assert.equal(
    normalizeViewQuery({
      sort: [
        { field: "a", direction: "asc" },
        { field: "b", direction: "asc" },
        { field: "c", direction: "asc" },
        { field: "d", direction: "asc" },
      ],
    }),
    null,
  );
  assert.equal(normalizeViewQuery({ filters: { dueBefore: "2026/01/01" } }), null);
  assert.equal(normalizeViewQuery({ filters: { custom: [{ fieldId: "f", operator: "gt" }] } }), null);
  assert.deepEqual(normalizeViewQuery(undefined), { filters: {}, sort: [] });
});

test("view query equality ignores key order and empty filters", () => {
  const a = { filters: { title: "x", openOnly: true }, sort: [] };
  const b = { filters: { openOnly: true, title: "x", statusId: "" }, sort: [] } as never;
  assert.equal(viewQueriesEqual(a, b), true);
  assert.equal(viewQueriesEqual(a, EMPTY_VIEW_QUERY), false);
  assert.equal(isEmptyViewQuery({ filters: { openOnly: false }, sort: [] }), true);
});

test("encode/parse round-trips and omits empty queries", () => {
  assert.equal(encodeViewQueryParam(EMPTY_VIEW_QUERY), undefined);
  const query = setPrimarySort(patchViewFilter(EMPTY_VIEW_QUERY, "openOnly", true), "title", "asc");
  const encoded = encodeViewQueryParam(query);
  assert.equal(encoded, '{"filters":{"openOnly":true},"sort":[{"field":"title","direction":"asc"}]}');
  assert.deepEqual(parseViewQueryParam(encoded), query);
  assert.equal(parseViewQueryParam("{not json"), null);
  assert.deepEqual(parseViewQueryParam(null), { filters: {}, sort: [] });
});

test("patch, sort and custom filter helpers", () => {
  let query = patchViewFilter(EMPTY_VIEW_QUERY, "statusId", "s1");
  assert.deepEqual(query.filters, { statusId: "s1" });
  query = patchViewFilter(query, "statusId", undefined);
  assert.deepEqual(query.filters, {});

  query = setPrimarySort(query, "field-uuid", "desc");
  assert.deepEqual(readPrimarySort(query), { field: "field-uuid", direction: "desc" });
  assert.deepEqual(setPrimarySort(query, null, "asc").sort, []);

  const filter = { fieldId: "f1", operator: "equals" as const, value: "opt" };
  query = addCustomFilter(query, filter);
  assert.equal(addCustomFilter(query, filter), query);
  query = addCustomFilter(query, { fieldId: "f2", operator: "empty" });
  assert.equal(query.filters.custom?.length, 2);
  query = removeCustomFilter(query, 0);
  assert.deepEqual(query.filters.custom, [{ fieldId: "f2", operator: "empty" }]);
  query = removeCustomFilter(query, 0);
  assert.equal(query.filters.custom, undefined);
});
