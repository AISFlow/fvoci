import assert from "node:assert/strict";
import test from "node:test";
import {
  COLLAB_PERSIST_DONE,
  COLLAB_PERSIST_FAILED,
  COLLAB_PERSIST_REQUEST,
  FVOCI_YDOC_FRAGMENT,
} from "@fvoci/editor/collab";

test("persist strings and fragment stay on the original contract", () => {
  assert.equal(FVOCI_YDOC_FRAGMENT, "prosemirror");
  assert.equal(COLLAB_PERSIST_REQUEST, "persist");
  assert.equal(COLLAB_PERSIST_DONE, "persisted");
  assert.equal(COLLAB_PERSIST_FAILED, "persist-failed");
});
