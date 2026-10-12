import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";
import type { components } from "../src/generated/api";
import {
  admin,
  attachCollabWire,
  clearEditor,
  closeCollabContext,
  expectMatchingPersistAck,
  login,
  newCollabContext,
  openEditor,
  persistBody,
} from "./collab-helpers";

type Schema = components["schemas"];

// A separate invocation after the immutable selected normal-main test. All
// state is observed through actual API responses and fresh browser clients.
test("SQLite ON revision restore: matching ACK, immutable history and fresh client readback", async ({
  browser,
  baseURL,
}, info) => {
  expect(process.env.FVOCI_E2E_SELECTED_BACKEND).toBe("sqlite");
  expect(process.env.FVOCI_E2E_SELECTED_FLOW).toBe("on");
  if (!baseURL) throw new Error("Allocated normal SQLite server required");
  const context = await newCollabContext(browser, baseURL);
  const page = await context.newPage();
  const wire = attachCollabWire(page);
  const token = randomUUID();
  try {
    await login(page, admin.email, admin.password);
    const workspaces = (await (await page.request.get("/api/v1/me/workspaces")).json()) as {
      items: { id: string; slug: string }[];
    };
    const workspace = workspaces.items.find((item) => item.slug === admin.workspaceSlug);
    if (!workspace) throw new Error("Selected workspace required");
    const response = await page.request.post(`/api/v1/workspaces/${workspace.id}/documents`, {
      data: { commandId: randomUUID(), title: "리비전 복원 " + token, parentId: null },
    });
    expect(response.status()).toBe(201);
    const document = (await response.json()) as Schema["DocumentMetaResponse"];
    const base = `/api/v1/workspaces/${workspace.id}/documents/${document.id}`;
    const url = `/w/${workspace.slug}/WIKI-${String(document.number)}`;
    const editor = await openEditor(page, url);
    await editor.fill("복원할 한국어 본문 " + token);
    await persistBody(page);
    await expectMatchingPersistAck(page, wire);
    const created = await page.request.post(base + "/revisions");
    expect(created.status()).toBe(201);
    const source = (await created.json()) as { id: string };
    const detailResponse = await page.request.get(`${base}/revisions/${source.id}`);
    expect(detailResponse.status()).toBe(200);
    const sourceDetail = (await detailResponse.json()) as Schema["RevisionDetailResponse"];
    await clearEditor(page);
    await editor.fill("교체된 최신 본문 " + token);
    await persistBody(page);
    await expectMatchingPersistAck(page, wire);
    await page.getByTestId("revision-history").click();
    const historyResponse = await page.request.get(base + "/revisions");
    expect(historyResponse.status()).toBe(200);
    const history = (await historyResponse.json()) as Schema["RevisionListResponse"];
    const index = history.items.findIndex((item) => item.id === source.id);
    expect(index).toBeGreaterThanOrEqual(0);
    const previewResponse = page.waitForResponse((r) =>
      r.url().endsWith(`/revisions/${source.id}/restore-preview`),
    );
    await page.getByTestId("revision-item").nth(index).getByTestId("revision-restore").click();
    const previewHttp = await previewResponse;
    expect(previewHttp.status()).toBe(200);
    const preview = (await previewHttp.json()) as Schema["RevisionRestorePreviewResponse"];
    await expect(page.getByTestId("revision-restore-source")).toContainText("복원할 한국어 본문");
    await expect(page.getByTestId("revision-restore-current")).toContainText("교체된 최신 본문");
    const restoreResponse = page.waitForResponse(
      (r) => r.request().method() === "POST" && r.url().endsWith(`/revisions/${source.id}/restore`),
    );
    await page.getByTestId("revision-restore-confirm").click();
    const restoredHttp = await restoreResponse;
    expect(restoredHttp.status()).toBe(200);
    const intent = restoredHttp.request().postDataJSON() as Schema["RevisionRestoreBody"];
    expect(intent.expectedTailSeq).toBe(preview.currentTailSeq);
    const restored = (await restoredHttp.json()) as Schema["RevisionRestoreResponse"];
    expect(restored.restored).toBe(true);
    expect(restored.revisionId).not.toBe(source.id);
    await expect(editor).toContainText("복원할 한국어 본문 " + token);
    await expect(editor).not.toContainText("교체된 최신 본문");
    const matching = await page.request.get(`${base}/revisions/${source.id}/restore-preview`);
    expect(matching.status()).toBe(200);
    const committed = (await matching.json()) as Schema["RevisionRestorePreviewResponse"];
    expect(BigInt(committed.currentTailSeq)).toBe(BigInt(intent.expectedTailSeq) + 1n);
    expect(committed.source).toEqual(sourceDetail);
    const restoredDetail = await page.request.get(`${base}/revisions/${restored.revisionId}`);
    expect(restoredDetail.status()).toBe(200);
    expect(await restoredDetail.json()).toMatchObject({
      id: restored.revisionId,
      reason: "restore",
      restoredFromId: source.id,
      contentJson: sourceDetail.contentJson,
    });
    const replay = await page.request.post(`${base}/revisions/${source.id}/restore`, {
      data: intent,
    });
    expect(replay.status()).toBe(200);
    expect(await replay.json()).toEqual(restored);
    const finalHistory = await page.request.get(base + "/revisions");
    expect(finalHistory.status()).toBe(200);
    const final = (await finalHistory.json()) as Schema["RevisionListResponse"];
    expect(final.items.filter((item) => item.reason === "restore")).toHaveLength(1);
    const fresh = await newCollabContext(browser, baseURL);
    try {
      const client = await fresh.newPage();
      const freshWire = attachCollabWire(client);
      await login(client, admin.email, admin.password);
      const readback = await openEditor(client, url);
      await expect(readback).toContainText("복원할 한국어 본문 " + token);
      await expect(readback).not.toContainText("교체된 최신 본문");
      const persisted = await client.request.get(base + "/body");
      expect(persisted.status()).toBe(200);
      expect((await persisted.json()) as Schema["BodyResponse"]).toMatchObject({
        contentJson: sourceDetail.contentJson,
      });
      await readback.press("End");
      await client.keyboard.type(" 복원 후 재편집");
      await persistBody(client);
      await expectMatchingPersistAck(client, freshWire);
      await expect(editor).toContainText("복원 후 재편집");
    } finally {
      await closeCollabContext(fresh);
    }
    await info.attach("sqlite-restore-api-ack.json", {
      body: JSON.stringify({
        source: source.id,
        restored,
        expected: intent.expectedTailSeq,
        committed: committed.currentTailSeq,
      }),
      contentType: "application/json",
    });
  } finally {
    await closeCollabContext(context);
  }
});
