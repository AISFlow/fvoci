import assert from "node:assert/strict";
import test from "node:test";
import { IMPORT_ACCEPT, IMPORT_SOURCES, type ImportSource } from "./import-source.ts";

await test("every import source has an accept list the file picker can use", () => {
  for (const source of IMPORT_SOURCES) {
    assert.equal(IMPORT_ACCEPT[source].includes("."), true, source);
  }
  const keys = Object.keys(IMPORT_ACCEPT) as ImportSource[];
  assert.deepEqual(keys.sort(), [...IMPORT_SOURCES].sort());
});
