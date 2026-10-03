import { expect, test, type Page } from "@playwright/test";
import { writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { z } from "zod";
const revisionDetailSchema = z
  .object({
    id: z.string(),
    reason: z.string(),
    restoredFromId: z.string().nullable(),
    createdBy: z.string().nullable(),
    createdAt: z.string(),
    contentJson: z.unknown(),
    ySnapshot: z.string(),
  })
  .passthrough();
import {
  admin,
  blockAt,
  createDoc,
  editorOf,
  newSignedInPage,
  openDoc,
  savedBody,
  setupInstance,
  workspaceId,
  type TiptapNode,
} from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
});

function paragraph(id: string | null, text: string): TiptapNode {
  return { type: "paragraph", ...(id ? { attrs: { id } } : {}), content: [{ type: "text", text }] };
}
function corpus(file: TiptapNode, reference: string, after: boolean): TiptapNode {
  const intro = paragraph("w4-intro", after ? "한국어 연구 계획" : "한국어 공부 계획");
  const move = paragraph("w4-move", "옮길 문단");
  return {
    type: "doc",
    content: [
      ...(after ? [move, intro] : [intro, move]),
      paragraph(after ? "w4-added" : "w4-removed", after ? "추가한 문단" : "삭제할 문단"),
      {
        type: "taskList",
        attrs: { id: "w4-checklist" },
        content: [
          {
            type: "taskItem",
            attrs: { id: "w4-check", checked: after },
            content: [paragraph("w4-check-text", "검토 완료")],
          },
        ],
      },
      {
        type: "table",
        attrs: { id: "w4-table" },
        content: [
          {
            type: "tableRow",
            content: [
              { type: "tableCell", content: [paragraph("w4-cell", after ? "45분" : "30분")] },
            ],
          },
          ...(after
            ? [
                {
                  type: "tableRow",
                  content: [{ type: "tableCell", content: [paragraph("w4-cell-added", "20분")] }],
                },
              ]
            : []),
        ],
      },
      {
        type: "paragraph",
        attrs: { id: "w4-link" },
        content: [
          {
            type: "text",
            text: "자료",
            marks: [
              {
                type: "link",
                attrs: { href: after ? "https://example.com/new" : "https://example.com/old" },
              },
            ],
          },
        ],
      },
      file,
      {
        type: "paragraph",
        attrs: { id: "w4-reference" },
        content: [
          {
            type: "mention",
            attrs: { entity: "document", id: reference, label: after ? "새 참조" : "원본 참조" },
          },
        ],
      },
      paragraph(null, after ? "ID 없는 문단의 편집" : "과거 ID 없는 문단"),
    ],
  };
}

async function nativeDrop(page: Page, path: string): Promise<void> {
  const point = await blockAt(page, 0).evaluate((element) => {
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    const text = walker.nextNode();
    if (!text?.textContent) throw new Error("Missing real drop text target");
    const range = document.createRange();
    range.setStart(text, text.textContent.length);
    range.collapse(true);
    const box = range.getBoundingClientRect();
    return { x: box.x + 1, y: box.y + box.height / 2 };
  });
  const session = await page.context().newCDPSession(page);
  try {
    for (const type of ["dragEnter", "dragOver", "drop"] as const)
      await session.send("Input.dispatchDragEvent", {
        type,
        ...point,
        data: { items: [], files: [path], dragOperationsMask: 1 },
      });
  } finally {
    await session.detach();
  }
}

/** Only the current invocation's isolated restricted role, read-only tenant transaction. */
function restrictedWitness(workspace: string, document: string): Record<string, unknown> {
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  const connection = process.env.DATABASE_APP_URL;
  if (!container?.startsWith("fvoci-rust-test-pg-") || !connection)
    throw new Error("Missing owned app-role DB fixture");
  const app = new URL(connection);
  if (
    app.hostname !== "127.0.0.1" ||
    !/^fvoci_app_fvoci_e2e_[a-f0-9]{16}$/.test(app.username) ||
    !/^\/fvoci_e2e_[a-f0-9]{16}$/.test(app.pathname)
  )
    throw new Error("Refusing non-fixture DB");
  for (const id of [workspace, document])
    if (!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(id))
      throw new Error("Invalid fixture ID");
  const result = spawnSync(
    "docker",
    [
      "exec",
      "-i",
      container,
      "psql",
      "-X",
      "-qAt",
      "-U",
      app.username,
      "-d",
      app.pathname.slice(1),
      "-v",
      "ON_ERROR_STOP=1",
    ],
    {
      input: `BEGIN READ ONLY;
SET LOCAL app.tenant_id = '${workspace}';
SELECT jsonb_build_object('role',current_user,'superuser',r.rolsuper,'bypassRls',r.rolbypassrls,
'notOwner',pg_get_userbyid(c.relowner) <> current_user,'forced',c.relforcerowsecurity,'rlsActive',row_security_active(c.oid),
'body',(SELECT content_json FROM fvoci.documents WHERE workspace_id='${workspace}' AND id='${document}'),
'history',(SELECT jsonb_agg(jsonb_build_object('id',id,'reason',reason,'source',restored_from_id,'actor',created_by,'at',created_at,'base',restore_base_tail_seq::text,'committed',restore_committed_tail_seq::text) ORDER BY created_at,id) FROM fvoci.revisions WHERE workspace_id='${workspace}' AND target_kind='document' AND target_id='${document}'))
FROM pg_roles r JOIN pg_class c ON c.oid='fvoci.revisions'::regclass WHERE r.rolname=current_user;
ROLLBACK;`,
      encoding: "utf8",
      timeout: 10000,
    },
  );
  expect(result.status, result.stderr).toBe(0);
  const witness = JSON.parse(result.stdout) as Record<string, unknown>;
  expect(witness).toMatchObject({
    role: app.username,
    superuser: false,
    bypassRls: false,
    notOwner: true,
    forced: true,
    rlsActive: true,
  });
  return witness;
}

async function frozenPair(page: Page, before: string, after: string): Promise<void> {
  await page.getByTestId("revision-before").selectOption(before);
  await page.getByTestId("revision-after").selectOption(after);
  await expect(page.getByTestId("revision-diff")).toHaveAttribute("data-before-revision", before);
  await expect(page.getByTestId("revision-diff")).toHaveAttribute("data-after-revision", after);
}

test("literal Korean semantic pair navigates, previews a conflict, restores new history, and fresh client reedits actual files", async ({
  browser,
  baseURL,
}, info) => {
  const a = await newSignedInPage(browser, baseURL, admin);
  const storageState = await a.context.storageState();
  const peerContext = await browser.newContext({ baseURL, storageState });
  const peer = await peerContext.newPage();
  try {
    const ws = await workspaceId(a.page.request);
    const referenced = await createDoc(a.page.request, ws, "참조 자료");
    const doc = await createDoc(a.page.request, ws, "W4 한국어 리비전 의미 비교", {
      json: { type: "doc", content: [paragraph("w4-upload", "첨부 기준")] },
    });
    const base = `/api/v1/workspaces/${ws}/documents/${doc.id}`;
    await openDoc(a.page, doc.path);
    const firstFilePath = info.outputPath("원본 자료.txt");
    writeFileSync(firstFilePath, "원본 첨부 한글\n");
    await nativeDrop(a.page, firstFilePath);
    await expect(editorOf(a.page).locator('[data-state="stored"]')).toHaveCount(1);
    await a.page.getByRole("button", { name: "저장", exact: true }).click();
    await expect(a.page.locator('[data-collab-persisted="true"]')).toBeVisible();
    const firstFile = (await savedBody(a.page.request, ws, doc.id)).content?.find(
      (node) => node.type === "attachment",
    );
    if (!firstFile) throw new Error("Missing actual first upload");
    expect(
      (
        await a.page.request.put(base + "/body", {
          data: { contentJson: corpus(firstFile, referenced.id, false) },
        })
      ).ok(),
    ).toBe(true);
    const sourceResponse = await a.page.request.post(base + "/revisions");
    expect(sourceResponse.status()).toBe(201);
    const source = (await sourceResponse.json()) as { id: string };
    const sourceDetail = revisionDetailSchema.parse(
      await (await a.page.request.get(base + "/revisions/" + source.id)).json(),
    );
    await openDoc(a.page, doc.path);
    const secondFilePath = info.outputPath("새 자료.txt");
    writeFileSync(secondFilePath, "새 첨부 한글\n");
    await nativeDrop(a.page, secondFilePath);
    await expect(editorOf(a.page).locator('[data-state="stored"]')).toHaveCount(2);
    await a.page.getByRole("button", { name: "저장", exact: true }).click();
    await expect(a.page.locator('[data-collab-persisted="true"]')).toBeVisible();
    const secondFile = (await savedBody(a.page.request, ws, doc.id)).content?.find(
      (node) => node.type === "attachment" && node.attrs?.id !== firstFile.attrs?.id,
    );
    if (!secondFile) throw new Error("Missing actual second upload");
    expect(
      (
        await a.page.request.put(base + "/body", {
          data: { contentJson: corpus(secondFile, referenced.id, true) },
        })
      ).ok(),
    ).toBe(true);
    await openDoc(a.page, doc.path);
    await openDoc(peer, doc.path);
    await a.page.getByTestId("revision-history").click();
    const created = a.page.waitForResponse(
      (response) =>
        response.request().method() === "POST" && response.url().endsWith(base + "/revisions"),
    );
    await a.page.getByTestId("revision-save").click();
    expect((await created).status()).toBe(201);
    const after = (await (await created).json()) as { id: string };
    await frozenPair(a.page, source.id, after.id);
    for (const kind of [
      "added",
      "removed",
      "moved",
      "text",
      "checkbox",
      "table",
      "link",
      "attachment",
      "reference",
    ])
      await expect(
        a.page.locator(`.revision-diff__changes [data-change-kind="${kind}"]`).first(),
      ).toBeVisible();
    await expect(a.page.getByTestId("revision-diff")).toContainText("ID");
    await a.page.getByTestId("revision-change-next").click();
    await expect(a.page.getByTestId("revision-change")).toHaveAttribute(
      "data-before-revision",
      source.id,
    );
    await expect(a.page.getByTestId("revision-change")).toHaveAttribute(
      "data-after-revision",
      after.id,
    );
    // Rows expose time/name; select the exact source through the actual metadata order.
    const history = (await (await a.page.request.get(base + "/revisions")).json()) as {
      items: { id: string }[];
    };
    const sourceIndex = history.items.findIndex((item) => item.id === source.id);
    expect(sourceIndex).toBeGreaterThanOrEqual(0);
    const restoreButton = a.page
      .getByTestId("revision-item")
      .nth(sourceIndex)
      .getByTestId("revision-restore");
    await restoreButton.click();
    await expect(a.page.getByTestId("revision-restore-preview")).toHaveAttribute(
      "data-source-revision",
      source.id,
    );
    await expect(a.page.getByTestId("revision-restore-current")).toContainText("연구 계획");
    await expect(a.page.getByTestId("revision-restore-source")).toContainText("공부 계획");
    await a.page.getByTestId("revision-restore-cancel").press("Escape");
    await expect(a.page.getByTestId("revision-restore-preview")).toHaveCount(0);
    await expect(restoreButton).toBeFocused();
    await restoreButton.click();
    await expect(a.page.getByTestId("revision-restore-preview")).toBeVisible();
    await editorOf(peer).click();
    await peer.keyboard.press("End");
    await peer.keyboard.type(" 동료 최신 변경");
    await peer.getByRole("button", { name: "저장", exact: true }).click();
    await expect(peer.locator('[data-collab-persisted="true"]')).toBeVisible();
    const conflict = a.page.waitForResponse((response) =>
      response.url().endsWith(`/revisions/${source.id}/restore`),
    );
    await a.page.getByTestId("revision-restore-confirm").click();
    expect((await conflict).status()).toBe(409);
    await expect(a.page.getByTestId("revision-restore-confirm")).toBeDisabled();
    await a.page.getByTestId("revision-restore-refresh").click();
    await expect(a.page.getByTestId("revision-restore-current")).toContainText("동료 최신 변경");
    const restoredResponse = a.page.waitForResponse((response) =>
      response.url().endsWith(`/revisions/${source.id}/restore`),
    );
    await a.page.getByTestId("revision-restore-confirm").click();
    expect((await restoredResponse).status()).toBe(200);
    const restored = (await (await restoredResponse).json()) as { revisionId: string };
    expect(restored.revisionId).not.toBe(source.id);
    expect(restored.revisionId).not.toBe(after.id);
    await expect(editorOf(a.page)).toContainText("공부 계획");
    await expect(editorOf(peer)).toContainText("공부 계획");
    await expect(editorOf(peer)).not.toContainText("동료 최신 변경");
    const sourceAfter = revisionDetailSchema.parse(
      await (await a.page.request.get(base + "/revisions/" + source.id)).json(),
    );
    expect(sourceAfter).toEqual(sourceDetail);
    const me = (await (await a.page.request.get("/api/v1/auth/me")).json()) as { userId: string };
    const newDetail = revisionDetailSchema.parse(
      await (await a.page.request.get(base + "/revisions/" + restored.revisionId)).json(),
    );
    expect(newDetail).toMatchObject({
      reason: "restore",
      restoredFromId: source.id,
      createdBy: me.userId,
      contentJson: sourceDetail.contentJson,
    });
    expect(Date.parse(newDetail.createdAt)).toBeGreaterThan(Date.now() - 60000);
    const firstReadback = restrictedWitness(ws, doc.id);
    // Historical idless nodes may acquire IDs in the live editor. Check the
    // literal meaning and existing identities; never normalize away known IDs.
    const restoredBody = firstReadback.body as TiptapNode;
    expect(
      restoredBody.content?.find((node) => node.attrs?.id === "w4-intro")?.content?.[0]?.text,
    ).toBe("한국어 공부 계획");
    expect(
      restoredBody.content?.find((node) => node.attrs?.id === "w4-checklist")?.content?.[0]?.attrs
        ?.checked,
    ).toBe(false);
    expect(
      restoredBody.content?.find((node) => node.attrs?.id === "w4-removed")?.content?.[0]?.text,
    ).toBe("삭제할 문단");
    expect(restoredBody.content?.some((node) => node.attrs?.id === "w4-added")).toBe(false);
    expect(
      restoredBody.content
        ?.filter((node) => node.type === "attachment")
        .map((node) => node.attrs?.id),
    ).toEqual([firstFile.attrs?.id]);
    const freshContext = await browser.newContext({ baseURL, storageState });
    try {
      const fresh = await freshContext.newPage();
      await openDoc(fresh, doc.path);
      await expect(editorOf(fresh)).toContainText("공부 계획");
      const body = await savedBody(fresh.request, ws, doc.id);
      expect(body.content?.find((node) => node.attrs?.id === "w4-intro")?.content?.[0]?.text).toBe(
        "한국어 공부 계획",
      );
      expect(
        body.content?.filter((node) => node.type === "attachment").map((node) => node.attrs?.id),
      ).toEqual([firstFile.attrs?.id]);
      const download = await fresh.request.get(
        `/api/v1/workspaces/${ws}/attachments/${String(firstFile.attrs?.id)}/download`,
      );
      expect(download.status()).toBe(200);
      expect((await download.body()).toString()).toBe("원본 첨부 한글\n");
      await editorOf(fresh).click();
      await fresh.keyboard.press("End");
      await fresh.keyboard.type(" 복원 후 재편집");
      await fresh.getByRole("button", { name: "저장", exact: true }).click();
      await expect(fresh.locator('[data-collab-persisted="true"]')).toBeVisible();
      await expect(editorOf(a.page)).toContainText("복원 후 재편집");
      expect(JSON.stringify(restrictedWitness(ws, doc.id).body)).toContain("복원 후 재편집");
      expect(
        await (await fresh.request.get(base + "/revisions/" + restored.revisionId)).json(),
      ).toEqual(newDetail);
    } finally {
      await freshContext.close();
    }
    await a.page.setViewportSize({ width: 320, height: 800 });
    await a.page.evaluate(() => {
      document.documentElement.style.fontSize = "200%";
    });
    await frozenPair(a.page, source.id, after.id);
    const layout = await a.page.getByTestId("revision-diff").evaluate((element) => ({
      width: element.clientWidth,
      scroll: element.scrollWidth,
      font: getComputedStyle(element.querySelector("article") ?? element).fontSize,
    }));
    expect(layout.scroll).toBeLessThanOrEqual(layout.width + 1);
    expect(parseFloat(layout.font)).toBeGreaterThanOrEqual(32);
    await info.attach("w4-real-pair-restore-app-role-readback.json", {
      body: JSON.stringify({ source: source.id, after: after.id, restored, firstReadback, layout }),
      contentType: "application/json",
    });
  } finally {
    await peerContext.close();
    await a.context.close();
  }
});
