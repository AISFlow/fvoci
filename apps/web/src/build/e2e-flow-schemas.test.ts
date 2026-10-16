import { expect, test } from "bun:test";
import { flowSchemas } from "../../e2e/helpers.ts";

// Browser specs read server answers through these oracles. A key typed as
// unknown may be absent, and keys an oracle does not name are kept.
test("e2e flow oracles accept a missing unknown key and keep unnamed keys", () => {
  expect(flowSchemas.body.parse({})).toEqual({});
  expect(flowSchemas.body.parse({ contentJson: { type: "doc" }, version: 3 })).toEqual({
    contentJson: { type: "doc" },
    version: 3,
  });
  const user = { userId: "u", givenName: "한글", email: "a@b.co", locale: "ko" };
  expect(flowSchemas.user.parse(user)).toEqual(user);
});
