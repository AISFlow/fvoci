/**
 * The Vue wiki page's collab room against the owned server
 * (collab-restart.ts): across a restart, where it stops gracefully and comes
 * back on the same port while two editors keep the document open, and at
 * the room cap, where it refuses a room with close 1013. Runs in the
 * collaboration-flow job after workspace-wiki-collab.spec.ts (same worker
 * server).
 * Invocation: FVOCI_E2E_PENDING=1 bash scripts/run-web-e2e.sh
 */
import type { WebSocket } from "@playwright/test";
import {
  attachCollabWire,
  closeCollabContext,
  createWikiDoc,
  editorLocator,
  ensureCollabFixture,
  expect,
  expectConverged,
  expectMatchingPersistAck,
  expectNotDurablySaved,
  expectTokens,
  login,
  member,
  newCollabContext,
  openEditor,
  persistBody,
  sentPersistRequests,
  test,
  waitConnected,
} from "./collab-helpers";

test("after a collab restart the same editor reconnects with its unsent edits", async ({
  browser,
  collabApp,
}) => {
  const ctxA = await newCollabContext(browser, collabApp.baseUrl);
  const ctxB = await newCollabContext(browser, collabApp.baseUrl);
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  const wireA = attachCollabWire(pageA);
  let failed = true;
  try {
    await ensureCollabFixture(pageA);
    await login(pageA, member.email, member.password);
    await login(pageB, member.email, member.password);
    const doc = await createWikiDoc(pageA, "재시작 재접속");
    const editorA = await openEditor(pageA, doc.url);
    await openEditor(pageB, doc.url);
    // The Vue page, and a marker on the live editor: a remount would lose it.
    await expect(pageA.locator("#root[data-v-app]")).toHaveCount(1);
    await editorA.evaluate((root) => {
      (root as HTMLElement & { fvociProbe?: string }).fvociProbe = "same editor";
    });

    await editorA.click();
    await pageA.keyboard.type("재시작 전 문장");
    await expectTokens(pageB, ["재시작 전 문장"]);

    await collabApp.shutdownGraceful();
    try {
      await expect(pageA.locator('[data-collab-status="connected"]')).toHaveCount(0);
      await expect(pageB.locator('[data-collab-status="connected"]')).toHaveCount(0);
      // The body stays on screen and editable while the room is away.
      await editorA.focus();
      await pageA.keyboard.type(" 끊긴 동안 쓴 문장");
      await expect(editorA).toContainText("끊긴 동안 쓴 문장");
      await expectNotDurablySaved(pageA);
      await expect(pageA.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
    } finally {
      await collabApp.recycle();
    }

    await waitConnected(pageA);
    await waitConnected(pageB);
    expect(
      await editorLocator(pageA).evaluate((root) => (root as HTMLElement & { fvociProbe?: string }).fvociProbe),
    ).toBe("same editor");
    await expectTokens(pageB, ["재시작 전 문장", "끊긴 동안 쓴 문장"]);
    await expectConverged(pageA, pageB);
    expect(sentPersistRequests(wireA)).toEqual([]);

    await persistBody(pageA);
    await expectMatchingPersistAck(pageA, wireA);
    const body = await pageA.request.get(
      `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/body`,
    );
    expect(body.ok()).toBe(true);
    expect(JSON.stringify((await body.json()).contentJson)).toContain("끊긴 동안 쓴 문장");

    await pageB.reload();
    await waitConnected(pageB);
    await expectTokens(pageB, ["재시작 전 문장", "끊긴 동안 쓴 문장"]);
    failed = false;
  } finally {
    await Promise.all([closeCollabContext(ctxA, failed), closeCollabContext(ctxB, failed)]);
  }
});

test("a room refused at the room cap says so, mounts no editor, and opens once a room is free", async ({
  browser,
  collabApp,
}) => {
  const ctxA = await newCollabContext(browser, collabApp.baseUrl);
  const ctxB = await newCollabContext(browser, collabApp.baseUrl);
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  let failed = true;
  try {
    await ensureCollabFixture(pageA);
    await login(pageA, member.email, member.password);
    await login(pageB, member.email, member.password);
    const holder = await createWikiDoc(pageA, "방 점유");
    const refused = await createWikiDoc(pageA, "방 거절");
    const body = await pageA.request.put(
      `/api/v1/workspaces/${refused.workspaceId}/documents/${refused.id}/body`,
      { data: { contentMd: "자리가 나면 열리는 본문\n" } },
    );
    expect(body.ok(), await body.text()).toBe(true);
    // One room slot for this server process (the next recycle restores the default).
    await collabApp.recycle({ maxRooms: 1 });

    // A holds the only room.
    await openEditor(pageA, holder.url);
    await expect(pageA.locator("#root[data-v-app]")).toHaveCount(1);

    // B's room is refused before its body loads: the note says why, the
    // status reads busy, no editor stands in for the body, and the loading
    // note is not shown next to it. B's socket keeps retrying meanwhile.
    const socketsB: WebSocket[] = [];
    pageB.on("websocket", (socket) => {
      if (new URL(socket.url()).pathname.endsWith("/collab")) socketsB.push(socket);
    });
    await pageB.goto(refused.url);
    const bodyB = pageB.locator(".document-page__body");
    await expect(pageB.locator('[data-collab-status="busy"]')).toBeVisible({ timeout: 15_000 });
    await expect(pageB.locator('[data-collab-status="busy"]')).toHaveText("서버 혼잡 · 자동 재시도");
    await expect(bodyB.getByRole("status")).toHaveCount(1);
    await expect(bodyB.getByRole("status")).toContainText("동시에 열린 문서가 많아");
    await expect(pageB.locator(".fvoci-editor")).toHaveCount(0);
    await expect(pageB.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
    await expect.poll(() => socketsB.length, { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
    await expect(pageB.locator(".fvoci-editor")).toHaveCount(0);

    // A leaves; the room it held is idle and reclaimed for B's next retry.
    await pageA.close();
    await expect(pageB.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 45_000 });
    await expect(editorLocator(pageB)).toContainText("자리가 나면 열리는 본문");
    await expect(bodyB.getByRole("status")).toHaveCount(0);
    await editorLocator(pageB).click();
    await pageB.keyboard.press("End");
    await pageB.keyboard.type(" 그리고 편집");
    await expectTokens(pageB, ["자리가 나면 열리는 본문 그리고 편집"]);
    failed = false;
  } finally {
    await Promise.all([closeCollabContext(ctxA, failed), closeCollabContext(ctxB, failed)]);
  }
});
