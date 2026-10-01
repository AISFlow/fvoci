import assert from "node:assert/strict";
import { getSchema } from "@tiptap/core";
import type { Node as ProseMirrorNode } from "@tiptap/pm/model";
import { createFvociExtensions } from "@fvoci/editor/tiptap-schema";
import type { TiptapDoc } from "@fvoci/editor/json";
import { expect, test, type APIRequestContext } from "@playwright/test";
import { z } from "zod";
import { schemaCorpus } from "../../../packages/editor/test/schema-corpus";
import { emojiGlyph } from "../../../packages/editor/src/emoji-glyph";
import { extractText } from "../../../packages/editor/src/extract";
import { flowSchemas, readJson } from "./helpers";
import {
  admin,
  createDoc,
  newSignedInPage,
  openDoc,
  save,
  savedBody,
  setupInstance,
  workspaceId,
} from "./workspace-wiki-vue-editor";

const schema = getSchema(createFvociExtensions());
const detailSchema = z.object({ contentJson: z.unknown() }).passthrough();
const revisionSchema = z.object({ id: z.string() }).passthrough();
const errorSchema = z.object({ code: z.string() }).passthrough();

// Compare all supported IDs, references, attrs, mark sets, nesting and UTF-16
// text through the product schema. Added defaults/object-key order are not loss.
function expectSemanticBody(actual: unknown, expected: TiptapDoc): void {
  const node = schema.nodeFromJSON(actual);
  node.check();
  function semantic(n: ProseMirrorNode): ProseMirrorNode {
    // Emoji text -> named emoji atoms is an intentional editor conversion.
    // Compare the glyph WITH its marks; dropping emoji marks still fails.
    if (n.type.name === "emoji") return schema.text(emojiGlyph(n.toJSON()), n.marks);
    if (n.isText) return n;
    const children: ProseMirrorNode[] = [];
    n.forEach((child) => {
      children.push(semantic(child));
    });
    // The TOC's separate generated anchor is presentation metadata. The block
    // id, used for internal references and block PATCH, is compared exactly.
    const attrs = n.type.name === "heading" ? { ...n.attrs, "data-toc-id": null } : n.attrs;
    return n.type.create(attrs, children, n.marks);
  }
  expect(semantic(node).eq(semantic(schema.nodeFromJSON(expected))), JSON.stringify(actual)).toBe(
    true,
  );
}

async function uploadFile(
  request: APIRequestContext,
  wsId: string,
  docId: string,
  bytes: Buffer,
): Promise<string> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/documents/${docId}/uploads`, {
    data: { name: "한글 첨부.txt", sizeBytes: bytes.length },
  });
  expect(res.status()).toBe(201);
  const upload = await readJson(res, flowSchemas.upload);
  const parts: { partNumber: number; etag: string }[] = [];
  for (const part of upload.parts) {
    const put = await request.put(part.url, {
      headers: { "content-type": "application/octet-stream" },
      data: bytes.subarray(
        (part.partNumber - 1) * upload.partSizeBytes,
        part.partNumber * upload.partSizeBytes,
      ),
    });
    expect(put.ok()).toBe(true);
    const etag = put.headers()["etag"];
    assert(etag);
    parts.push({ partNumber: part.partNumber, etag });
  }
  expect(
    (
      await request.post(`/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`, {
        data: { parts },
      })
    ).ok(),
  ).toBe(true);
  return upload.attachmentId;
}

test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
});

test("schema corpus survives Rust durable save, fresh client, revision preview/restore, re-edit and export", async ({
  browser,
  baseURL,
}) => {
  const first = await newSignedInPage(browser, baseURL, admin);
  let fresh: Awaited<ReturnType<typeof newSignedInPage>> | undefined;
  try {
    const { page } = first;
    const wsId = await workspaceId(page.request);
    const doc = await createDoc(page.request, wsId, "의미 보존 corpus");
    const related = await createDoc(page.request, wsId, "관련 문서");
    const me = await page.request.get("/api/v1/auth/me");
    expect(me.ok()).toBe(true);
    const user = await readJson(me, flowSchemas.user);
    const bytes = Buffer.from("실제 한국어 첨부 😀\n", "utf8");
    const attachment = await uploadFile(page.request, wsId, doc.id, bytes);
    const corpus = schemaCorpus({ user: user.userId, document: related.id, attachment });
    const bodyUrl = `/api/v1/workspaces/${wsId}/documents/${doc.id}/body`;
    const revisionsUrl = `/api/v1/workspaces/${wsId}/documents/${doc.id}/revisions`;
    expect((await page.request.put(bodyUrl, { data: { contentJson: corpus } })).ok()).toBe(true);
    expectSemanticBody(await savedBody(page.request, wsId, doc.id), corpus);
    const editor = await openDoc(page, doc.path);
    await expect(editor).toContainText("한글 보존 🧑‍💻");
    await expect(editor.locator("table")).toHaveCount(2);
    await expect(editor).toContainText("한글 첨부.txt");
    await save(page);
    expectSemanticBody(await savedBody(page.request, wsId, doc.id), corpus);

    await page.getByTestId("revision-history").click();
    const created = page.waitForResponse(
      (r) => r.request().method() === "POST" && r.url().endsWith(`/documents/${doc.id}/revisions`),
    );
    await page.getByTestId("revision-save").click();
    const response = await created;
    expect(response.status()).toBe(201);
    const revision = revisionSchema.parse(await response.json());
    expectSemanticBody(
      detailSchema.parse(await (await page.request.get(`${revisionsUrl}/${revision.id}`)).json())
        .contentJson,
      corpus,
    );
    await page.getByTestId("revision-history").click();
    await first.context.close();

    // A new browser context/provider/Y.Doc, with no warmed editor or room state.
    fresh = await newSignedInPage(browser, baseURL, admin);
    const client = fresh.page;
    const reopened = await openDoc(client, doc.path);
    await expect(reopened.locator("table")).toHaveCount(2);
    const tail = reopened.locator('[data-id="corpus-edit-tail"]');
    await tail.click();
    await client.keyboard.press("End");
    await client.keyboard.type(" 수정 한글😀");
    await save(client);
    const edited = schemaCorpus({ user: user.userId, document: related.id, attachment });
    const editedNode = schema.nodeFromJSON(edited).toJSON() as {
      content: { content?: { text?: string }[] }[];
    };
    const last = editedNode.content.at(-1)?.content?.[0];
    assert(last);
    last.text = "재편집 위치 수정 한글😀";
    expectSemanticBody(await savedBody(client.request, wsId, doc.id), editedNode as TiptapDoc);

    await client.getByTestId("revision-history").click();
    const previewResponse = client.waitForResponse(
      (r) => r.request().method() === "GET" && r.url().endsWith(`/revisions/${revision.id}`),
    );
    await client.getByTestId("revision-item").first().locator("button").first().click();
    expectSemanticBody(
      detailSchema.parse(await (await previewResponse).json()).contentJson,
      corpus,
    );
    // The panel is an existing text-only summary: non-text atoms are omitted.
    // The fetched snapshot above still preserves their full semantics.
    await expect(client.getByTestId("revision-preview")).toContainText("한국어 문서");
    await expect(client.getByTestId("revision-preview")).toContainText("한글 보존 🧑‍💻");
    await expect(client.getByTestId("revision-preview")).not.toContainText("수정 한글");
    await client.getByTestId("revision-restore").first().click();
    await client.getByTestId("revision-restore-confirm").click();
    await expect(reopened.locator('[data-id="corpus-edit-tail"]')).toHaveText("재편집 위치");
    expectSemanticBody(await savedBody(client.request, wsId, doc.id), corpus);
    await client.getByTestId("revision-history").click();
    await client.reload();
    await expect(client.locator('[data-collab-status="connected"]')).toBeVisible();
    expectSemanticBody(await savedBody(client.request, wsId, doc.id), corpus);
    await reopened.locator('[data-id="corpus-edit-tail"]').click();
    await client.keyboard.press("End");
    await client.keyboard.type(" 수정 한글😀");
    await save(client);
    expectSemanticBody(await savedBody(client.request, wsId, doc.id), editedNode as TiptapDoc);

    await reopened.locator('[data-id="corpus-edit-tail"]').click();
    await client.keyboard.press("ControlOrMeta+z");
    await save(client);
    expectSemanticBody(await savedBody(client.request, wsId, doc.id), corpus);
    await reopened.locator('[data-id="corpus-edit-tail"]').click();
    await client.keyboard.press("ControlOrMeta+Shift+z");
    await save(client);
    expectSemanticBody(await savedBody(client.request, wsId, doc.id), editedNode as TiptapDoc);

    const download = await client.request.get(
      `/api/v1/workspaces/${wsId}/attachments/${attachment}/download`,
    );
    expect(download.status()).toBe(200);
    expect(await download.body()).toEqual(bytes);
    const exported = await client.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}/md`);
    expect(exported.status()).toBe(200);
    const markdown = await exported.text();
    for (const preserved of [
      "한국어 문서 😀",
      "한글 보존 🧑‍💻",
      "\\alpha + x^2",
      "\\int_0^1 x^2 dx = \\frac{1}{3}",
      "안쪽 한글",
      "김철수",
      `attachment:${attachment}`,
      "수정 한글😀",
    ]) {
      expect(markdown).toContain(preserved);
    }
    // Markdown is an export view, not a lossless archive of block IDs, cell
    // backgrounds/widths, caption and every mark. Durable JSON above owns them.
    const imported = await createDoc(client.request, wsId, "Markdown 편집 사본", { markdown });
    const importedBody = await savedBody(client.request, wsId, imported.id);
    for (const preserved of [
      "한국어 문서 😀",
      "한글 보존 🧑‍💻",
      "\\alpha + x^2",
      "\\int_0^1 x^2 dx = \\frac{1}{3}",
      "안쪽 한글",
      "김철수",
      "한글 첨부.txt",
      "수정 한글😀",
    ]) {
      expect(extractText(importedBody)).toContain(preserved);
    }
    const importedNode = schema.nodeFromJSON(importedBody);
    importedNode.check();
    let attachmentFound = false;
    importedNode.descendants((node) => {
      if (node.type.name === "attachment") {
        expect(node.attrs.id).toBe(attachment);
        attachmentFound = true;
      }
      if (node.isText && node.text?.includes("한글 보존")) {
        expect(node.marks.map((mark) => mark.type.name)).toEqual(["bold", "italic"]);
      }
    });
    expect(attachmentFound).toBe(true);
  } finally {
    await first.context.close();
    await fresh?.context.close();
  }
});

test("Rust JSON seeding refuses unknown nodes and marks without changing durable content", async ({
  browser,
  baseURL,
}) => {
  const { context, page } = await newSignedInPage(browser, baseURL, admin);
  try {
    const wsId = await workspaceId(page.request);
    const baseline: TiptapDoc = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "keep-id" },
          content: [{ type: "text", text: "원본 한글 😀", marks: [{ type: "bold" }] }],
        },
      ],
    };
    const doc = await createDoc(page.request, wsId, "거부 정책", { json: baseline });
    const url = `/api/v1/workspaces/${wsId}/documents/${doc.id}/body`;
    for (const invalid of [
      { type: "doc", content: [{ type: "futureNode", attrs: { id: "future-ref" } }] },
      {
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "future", marks: [{ type: "futureMark" }] }],
          },
        ],
      },
    ]) {
      const res = await page.request.put(url, { data: { contentJson: invalid } });
      expect(res.status()).toBe(400);
      expect(errorSchema.parse(await res.json()).code).toBe("invalid_document_body");
      expectSemanticBody(await savedBody(page.request, wsId, doc.id), baseline);
    }
    const editor = await openDoc(page, doc.path);
    await expect(editor).toContainText("원본 한글 😀");
  } finally {
    await context.close();
  }
});

test("unrepresentable emoji imports are refused; marking a live Unicode-backed atom survives save and reload", async ({
  browser,
  baseURL,
}) => {
  const { context, page } = await newSignedInPage(browser, baseURL, admin);
  try {
    const wsId = await workspaceId(page.request);
    const body: TiptapDoc = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "explicit-emoji" },
          content: [{ type: "emoji", attrs: { name: "grinning" }, marks: [{ type: "bold" }] }],
        },
      ],
    };
    const baseline: TiptapDoc = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "explicit-emoji" },
          content: [{ type: "text", text: "원본 😀" }],
        },
      ],
    };
    const doc = await createDoc(page.request, wsId, "emoji 표현 계약", { json: baseline });
    const url = `/api/v1/workspaces/${wsId}/documents/${doc.id}/body`;
    for (const name of ["grinning", "unknown-custom"]) {
      const marked = structuredClone(body) as {
        type: "doc";
        content: { content: { attrs: { name: string } }[] }[];
      };
      const atom = marked.content[0]?.content[0];
      assert(atom);
      atom.attrs.name = name;
      const refused = await page.request.put(url, { data: { contentJson: marked } });
      expect(refused.status()).toBe(400);
      expect(errorSchema.parse(await refused.json()).code).toBe("invalid_document_body");
      expectSemanticBody(await savedBody(page.request, wsId, doc.id), baseline);
    }
    const editor = await openDoc(page, doc.path);
    await expect(editor.locator('[data-type="emoji"]')).toHaveCount(1);
    await editor.click();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.press("ControlOrMeta+b");
    await save(page);
    const expected: TiptapDoc = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "explicit-emoji" },
          content: [{ type: "text", text: "원본 😀", marks: [{ type: "bold" }] }],
        },
      ],
    };
    expectSemanticBody(await savedBody(page.request, wsId, doc.id), expected);
    await page.reload();
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
    expectSemanticBody(await savedBody(page.request, wsId, doc.id), expected);
  } finally {
    await context.close();
  }
});
