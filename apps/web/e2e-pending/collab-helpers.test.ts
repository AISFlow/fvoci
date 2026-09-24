import assert from "node:assert/strict";
import { test } from "node:test";
import { attachmentNodesFromDocument } from "./collab-attachment-oracle.ts";

test("attachmentNodesFromDocument extracts stored attachment id, name, and image flag", () => {
  assert.deepEqual(
    attachmentNodesFromDocument({
      type: "doc",
      content: [
        {
          type: "attachment",
          attrs: {
            id: "33333333-3333-7333-8333-333333333333",
            name: "collab-fixture.bin",
            image: false,
          },
        },
      ],
    }),
    [
      {
        attachmentId: "33333333-3333-7333-8333-333333333333",
        name: "collab-fixture.bin",
        image: false,
      },
    ],
  );
});
