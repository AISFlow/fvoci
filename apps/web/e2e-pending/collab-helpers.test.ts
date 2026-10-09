import assert from "node:assert/strict";
import { test } from "node:test";
import type { Page } from "@playwright/test";
import { createWikiDoc } from "./collab-helpers.ts";
import { attachmentNodesFromDocument } from "./collab-attachment-oracle.ts";

await test("attachmentNodesFromDocument extracts stored attachment id, name, and image flag", () => {
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

await test("wiki fixture keeps one command on lost-response replay and gives distinct creates distinct commands", async () => {
  const workspaceId = "11111111-1111-7111-8111-111111111111";
  const commandId = crypto.randomUUID();
  const requests: Array<{ commandId: string; parentId: null; title: string }> = [];
  const documents = new Map<string, { id: string; displayId: string }>();
  let loseResponse = true;
  const page = {
    request: {
      get(url: string) {
        assert.equal(url, "/api/v1/me/workspaces");
        return Promise.resolve({
          ok: () => true,
          json: () => Promise.resolve({ items: [{ id: workspaceId, slug: "acme" }] }),
        });
      },
      post(url: string, { data }: { data: (typeof requests)[number] }) {
        return Promise.resolve().then(() => {
          assert.equal(url, `/api/v1/workspaces/${workspaceId}/documents`);
          assert.equal(data.parentId, null);
          assert.match(
            data.commandId,
            /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
          );
          requests.push({ ...data });
          const doc = documents.get(data.commandId) ?? {
            id: crypto.randomUUID(),
            displayId: `WIKI-${String(documents.size + 1)}`,
          };
          documents.set(data.commandId, doc);
          if (loseResponse) {
            loseResponse = false;
            throw new Error("response lost after commit");
          }
          return { ok: () => true, json: () => Promise.resolve(doc) };
        });
      },
    },
  } as unknown as Page;
  await assert.rejects(
    createWikiDoc(page, "한 논리 생성 🧪", commandId),
    /response lost after commit/,
  );
  const replay = await createWikiDoc(page, "한 논리 생성 🧪", commandId);
  assert.deepEqual(requests[0], requests[1]);
  assert.equal(replay.id, documents.get(commandId)?.id);
  assert.equal(replay.url, "/w/acme/WIKI-1");
  assert.equal(documents.size, 1);
  const distinct = await createWikiDoc(page, "같은 제목");
  const another = await createWikiDoc(page, "같은 제목");
  assert.notEqual(distinct.id, another.id);
  assert.notEqual(requests[2]?.commandId, requests[3]?.commandId);
  assert.equal(documents.size, 3);
});
