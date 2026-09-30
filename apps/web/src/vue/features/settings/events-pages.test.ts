import assert from "node:assert/strict";
import test from "node:test";
import { flattenEventPages } from "./events-pages.ts";

test("flattenEventPages keeps server order across pages", () => {
  assert.deepEqual(flattenEventPages(undefined), []);
  assert.deepEqual(
    flattenEventPages([{ items: [{ id: "a" }, { id: "b" }] }, { items: [{ id: "c" }] }]),
    [{ id: "a" }, { id: "b" }, { id: "c" }],
  );
});
