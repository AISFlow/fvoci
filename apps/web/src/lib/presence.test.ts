import assert from "node:assert/strict";
import test from "node:test";
import { PRESENCE_COLORS, presenceColorOf } from "./presence.ts";

test("presenceColorOf is a pure function of the trailing hex", () => {
  assert.equal(PRESENCE_COLORS.length, 8);
  assert.equal(
    presenceColorOf("01a01f00-0000-7000-8000-000000000001"),
    PRESENCE_COLORS[1],
  );
  assert.equal(
    presenceColorOf("not-a-uuid"),
    PRESENCE_COLORS[0],
  );
});
