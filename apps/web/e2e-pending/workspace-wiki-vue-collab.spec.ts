/**
 * The Vue wiki page's collab room across a server restart: the owned server
 * (collab-restart.ts) stops gracefully and comes back on the same port while
 * two editors keep the document open. Runs in the collaboration-flow job
 * after workspace-wiki-collab.spec.ts (same worker server).
 * Invocation: FVOCI_E2E_PENDING=1 bash scripts/run-web-e2e.sh
 */
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
