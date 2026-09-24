/**
 * Document revision create/restore on two live collab peers.
 * Run with: FVOCI_E2E_PENDING=1 bash scripts/run-web-e2e.sh
 */
import {
  closeCollabContext,
  createWikiDoc,
  editorShape,
  ensureInstanceSetup,
  expectConverged,
  installCollabMember,
  login,
  member,
  newCollabContext,
  openEditor,
  persistBody,
  test,
  expect,
  waitConnected,
} from "./collab-helpers";

test("instance setup then member fixture", async ({ page }) => {
  await ensureInstanceSetup(page);
  installCollabMember();
  await login(page, member.email, member.password);
});

test("edit, create revision, restore, both peers see restored content after reload", async ({
  browser,
  collabApp,
}) => {
  const ctxA = await newCollabContext(browser, collabApp.baseUrl);
  const ctxB = await newCollabContext(browser, collabApp.baseUrl);
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  try {
    await login(pageA, member.email, member.password);
    await login(pageB, member.email, member.password);
    const doc = await createWikiDoc(pageA, "개정 복원");
    const editorA = await openEditor(pageA, doc.url);
    const editorB = await openEditor(pageB, doc.url);
    await editorA.click();
    await pageA.keyboard.type("개정 전 본문");
    await persistBody(pageA);
    await pageA.getByTestId("revision-history").click();
    await pageA.getByTestId("revision-save").click();
    await expect(pageA.getByTestId("revision-item").first()).toBeVisible();
    await pageA.getByTestId("revision-history").click();
    await editorA.click();
    await pageA.keyboard.type(" 그리고 더 작성");
    await persistBody(pageA);
    await expect.poll(async () => (await editorShape(pageA)).text).toContain("그리고 더 작성");
    await pageA.getByTestId("revision-history").click();
    await pageA.getByTestId("revision-restore").first().click();
    await pageA.getByTestId("revision-restore-confirm").click();
    await expect
      .poll(async () => (await editorShape(pageA)).text, { timeout: 15_000 })
      .toContain("개정 전 본문");
    await expect
      .poll(async () => (await editorShape(pageA)).text, { timeout: 15_000 })
      .not.toContain("그리고 더 작성");
    await expectConverged(pageA, pageB);
    await expect((await editorShape(pageB)).text).toContain("개정 전 본문");
    await pageA.reload();
    await waitConnected(pageA);
    expect((await editorShape(pageA)).text).toContain("개정 전 본문");
    expect((await editorShape(pageA)).text).not.toContain("그리고 더 작성");
  } finally {
    await closeCollabContext(ctxA, false);
    await closeCollabContext(ctxB, false);
  }
});
