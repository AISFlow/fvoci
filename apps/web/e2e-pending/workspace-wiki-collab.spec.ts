/**
 * Product /collab acceptance for two real FvociEditor clients.
 * Not registered in apps/web/e2e (CI web.yml). Not accepted until the
 * coordinator grants the heavy browser/DB slot. Pending invocation:
 * FVOCI_E2E_PENDING=1 bash scripts/run-web-e2e.sh
 */
import {
  admin,
  applyBoldToSelection,
  applyLinkToSelection,
  attachCollabWire,
  createE2eUser,
  createWikiDoc,
  editorLocator,
  editorShape,
  ensureInstanceSetup,
  expectAwarenessTokenNotSession,
  expectConverged,
  expectMatchingPersistAck,
  expectNotDurablySaved,
  expectTokens,
  expectTokensAbsent,
  indexedDbNames,
  insertSlashTable,
  installCollabMember,
  installCollabPeer,
  login,
  MEMBER_PRESENCE,
  member,
  newCollabContext,
  openEditor,
  PEER_PRESENCE,
  peer,
  persistBody,
  placeContentCaret,
  sentPersistRequests,
  sessionCookie,
  test,
  expect,
  uniqueBlockIds,
  waitConnected,
  workspaceId,
} from "./collab-helpers";

test.describe.configure({ mode: "serial" });

test("instance setup then member fixture", async ({ page }) => {
  await ensureInstanceSetup(page);
  installCollabMember();
  await login(page, member.email, member.password);
});

test("Korean, Han, and emoji keep UniqueID across persist and reload", async ({
  page,
  context,
}) => {
  const wire = attachCollabWire(page);
  await login(page, member.email, member.password);
  const session = await sessionCookie(context);
  const doc = await createWikiDoc(page, "협업 본문");
  const editor = await openEditor(page, doc.url);
  await expectAwarenessTokenNotSession(wire, session);
  await editor.click();
  await page.keyboard.type("본문 한글과 漢字🙂");
  await expectNotDurablySaved(page);
  await persistBody(page);
  await expectMatchingPersistAck(page, wire);
  const before = await editorShape(page);
  const ids = uniqueBlockIds(before);
  expect(before.text).toContain("본문 한글과 漢字🙂");
  await page.reload();
  await waitConnected(page);
  const after = await editorShape(page);
  expect(after.text).toContain("본문 한글과 漢字🙂");
  expect(uniqueBlockIds(after)).toEqual(ids);
});

test("two clients insert at the same caret and both tokens survive", async ({
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
    const doc = await createWikiDoc(pageA, "같은 위치 삽입");
    const editorA = await openEditor(pageA, doc.url);
    const editorB = await openEditor(pageB, doc.url);
    await editorA.click();
    await editorB.click();
    await Promise.all([pageA.keyboard.type("가나다토큰"), pageB.keyboard.type("🙂BETA")]);
    await expectTokens(pageA, ["가나다토큰", "🙂BETA"]);
    await expectTokens(pageB, ["가나다토큰", "🙂BETA"]);
    await expectConverged(pageA, pageB);
  } finally {
    await ctxA.close();
    await ctxB.close();
  }
});

test("insert and delete conflict keeps the insertion and applies the deletion", async ({
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
    const doc = await createWikiDoc(pageA, "삽입 삭제 충돌");
    const editorA = await openEditor(pageA, doc.url);
    await editorA.click();
    await pageA.keyboard.type("한글본문");
    await expectTokens(pageA, ["한글본문"]);
    const editorB = await openEditor(pageB, doc.url);
    await expectTokens(pageB, ["한글본문"]);
    await Promise.all([
      (async () => {
        await editorA.click();
        await placeContentCaret(pageA, "start");
        await pageA.keyboard.type("앞쪽삽입");
      })(),
      (async () => {
        await editorB.click();
        await placeContentCaret(pageB, "end");
        await pageB.keyboard.press("Backspace");
        await pageB.keyboard.press("Backspace");
      })(),
    ]);
    await expectTokens(pageA, ["앞쪽삽입", "한글"]);
    await expectTokens(pageB, ["앞쪽삽입", "한글"]);
    await expectTokensAbsent(pageA, ["한글본문"]);
    await expectTokensAbsent(pageB, ["한글본문"]);
    await expectConverged(pageA, pageB);
  } finally {
    await ctxA.close();
    await ctxB.close();
  }
});

test("Korean plus emoji middle insert and delete converge without dropping IDs", async ({
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
    const doc = await createWikiDoc(pageA, "중간 한글 이모지");
    const editorA = await openEditor(pageA, doc.url);
    await editorA.click();
    await pageA.keyboard.type("안녕🙂세계");
    const editorB = await openEditor(pageB, doc.url);
    await expectTokens(pageA, ["안녕🙂세계"]);
    await expectTokens(pageB, ["안녕🙂세계"]);
    await expectConverged(pageA, pageB);
    const beforeIds = uniqueBlockIds(await editorShape(pageA));
    await Promise.all([
      (async () => {
        await editorA.click();
        await placeContentCaret(pageA, "start");
        await pageA.keyboard.press("ArrowRight");
        await pageA.keyboard.press("ArrowRight");
        await pageA.keyboard.type("중간");
      })(),
      (async () => {
        await editorB.click();
        await placeContentCaret(pageB, "end");
        await pageB.keyboard.press("ArrowLeft");
        await pageB.keyboard.press("ArrowLeft");
        await pageB.keyboard.press("ArrowLeft");
        await pageB.keyboard.press("Delete");
      })(),
    ]);
    await expectTokens(pageA, ["안녕", "중간", "세계"]);
    await expectTokens(pageB, ["안녕", "중간", "세계"]);
    await expectTokensAbsent(pageA, ["🙂"]);
    await expectTokensAbsent(pageB, ["🙂"]);
    await expectConverged(pageA, pageB);
    expect(uniqueBlockIds(await editorShape(pageA))).toEqual(beforeIds);
    expect(uniqueBlockIds(await editorShape(pageB))).toEqual(beforeIds);
  } finally {
    await ctxA.close();
    await ctxB.close();
  }
});

test("offline typing reconnects with unsent text and without a persist ack", async ({
  page,
  context,
}) => {
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "재접속");
  const editor = await openEditor(page, doc.url);
  await context.setOffline(true);
  await editor.click();
  await page.keyboard.type("오프라인에서 쓴 줄");
  await expect(page.getByText("연결됨 · 저장 대기")).toBeVisible();
  await expectNotDurablySaved(page);
  await context.setOffline(false);
  await waitConnected(page);
  await expect(editor).toContainText("오프라인에서 쓴 줄");
  await expectNotDurablySaved(page);
});

test("archived document stays connected and read-only", async ({ page }) => {
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "보관 문서");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("보관 전 문장");
  await persistBody(page);
  await page.getByLabel("문서 상태").selectOption("archived");
  await expect(page.getByLabel("문서 상태")).toHaveValue("archived");
  await expect(page.getByText("읽기 전용")).toBeVisible();
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
  await expectNotDurablySaved(page);
  await expect(page.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
  await editor.click();
  await page.keyboard.type("보관 후 문장");
  await expect(editor).toContainText("보관 전 문장");
  await expect(editor).not.toContainText("보관 후 문장");
});

test("membership revoke while connected stops further edits", async ({
  browser,
  collabApp,
}) => {
  installCollabPeer();
  const ownerCtx = await newCollabContext(browser, collabApp.baseUrl);
  const memberCtx = await newCollabContext(browser, collabApp.baseUrl);
  const ownerPage = await ownerCtx.newPage();
  const memberPage = await memberCtx.newPage();
  try {
    await login(memberPage, peer.email, peer.password);
    const me = await memberPage.request.get("/api/v1/auth/me");
    expect(me.ok()).toBe(true);
    const memberId = (await me.json()).userId as string;

    await login(ownerPage, admin.email, admin.password);
    const doc = await createWikiDoc(ownerPage, "철회 문서");
    const editor = await openEditor(memberPage, doc.url);
    await editor.click();
    await memberPage.keyboard.type("철회 전 문장");
    await expect(editor).toContainText("철회 전 문장");

    const ws = await workspaceId(ownerPage, admin.workspaceSlug);
    const revoke = await ownerPage.request.delete(
      `/api/v1/workspaces/${ws}/members/${memberId}`,
    );
    expect(revoke.ok()).toBe(true);

    await expect(memberPage.locator('[data-collab-status="unauthorized"]')).toBeVisible({
      timeout: 20_000,
    });
    await expect(memberPage.getByRole("alert")).toHaveText("권한 없음 · 다시 로그인");
    await expect(editorLocator(memberPage)).toHaveCount(0);
    await expect(memberPage.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
  } finally {
    await ownerCtx.close();
    await memberCtx.close();
  }
});

test("delete-only save then structured marks, table, and IDs persist", async ({ page }) => {
  const wire = attachCollabWire(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "삭제만");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("지울 문장");
  await persistBody(page);
  const firstId = sentPersistRequests(wire).at(-1);
  expect(firstId).toBeTruthy();
  await page.keyboard.press("Control+A");
  await page.keyboard.press("Backspace");
  await expectNotDurablySaved(page);
  await persistBody(page);
  const secondId = sentPersistRequests(wire).at(-1);
  expect(secondId).not.toBe(firstId);
  await expectMatchingPersistAck(page, wire);

  await editor.click();
  await page.keyboard.type("굵은링크");
  await page.keyboard.press("Shift+Home");
  await applyBoldToSelection(page);
  await applyLinkToSelection(page, "https://example.com");
  await persistBody(page);
  await insertSlashTable(page);
  await persistBody(page);
  const structured = await editorShape(page);
  const ids = uniqueBlockIds(structured);
  expect(structured.table).not.toBeNull();
  expect(structured.table?.rows.length).toBeGreaterThan(0);
  expect(structured.bold.join("")).toContain("굵은링크");
  expect(structured.hrefs.some((link) => link.href === "https://example.com")).toBe(true);
  expect(structured.text).not.toContain("지울 문장");

  await page.reload();
  await waitConnected(page);
  const restored = await editorShape(page);
  expect(restored.table).not.toBeNull();
  expect(restored.table?.id).toBe(structured.table?.id);
  expect(restored.table?.rows).toEqual(structured.table?.rows);
  expect(uniqueBlockIds(restored)).toEqual(ids);
  expect(restored.bold.join("")).toContain("굵은링크");
  expect(restored.hrefs.some((link) => link.href === "https://example.com")).toBe(true);
  expect(restored.text).not.toContain("지울 문장");
});

test("persist ack correlates request id on the real /collab socket", async ({ page }) => {
  const wire = attachCollabWire(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "ack 상관");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("상관 본문");
  await expectNotDurablySaved(page);
  await persistBody(page);
  const requestId = await expectMatchingPersistAck(page, wire);
  expect(sentPersistRequests(wire).filter((id) => id === requestId)).toHaveLength(1);
  const foreignDone = wire.received.filter(
    (frame) =>
      frame.kind === "stateless" &&
      frame.payload.startsWith("persisted:") &&
      frame.payload !== `persisted:${requestId}`,
  );
  expect(foreignDone).toEqual([]);
});

test("fresh context after process-tree crash SIGKILL reloads two-client persisted delete and structure", async ({
  browser,
  collabApp,
}) => {
  const seedA = await newCollabContext(browser, collabApp.baseUrl);
  const seedB = await newCollabContext(browser, collabApp.baseUrl);
  const pageA = await seedA.newPage();
  const pageB = await seedB.newPage();
  const wire = attachCollabWire(pageA);
  let url = "";
  let seeded: Awaited<ReturnType<typeof editorShape>> | undefined;
  try {
    await login(pageA, member.email, member.password);
    await login(pageB, member.email, member.password);
    const doc = await createWikiDoc(pageA, "크래시 복원");
    url = doc.url;
    const editorA = await openEditor(pageA, doc.url);
    const editorB = await openEditor(pageB, doc.url);
    await editorA.click();
    await pageA.keyboard.type("살아남을한글");
    await persistBody(pageA);
    await expectMatchingPersistAck(pageA, wire);
    await expectTokens(pageB, ["살아남을한글"]);

    await editorB.click();
    await placeContentCaret(pageB, "end");
    await pageB.keyboard.type("지울토큰XYZ");
    await expectTokens(pageA, ["살아남을한글", "지울토큰XYZ"]);
    await expectTokens(pageB, ["살아남을한글", "지울토큰XYZ"]);
    await expectConverged(pageA, pageB);
    await persistBody(pageB);

    await editorA.click();
    await placeContentCaret(pageA, "end");
    for (let i = 0; i < "지울토큰XYZ".length; i += 1) {
      await pageA.keyboard.press("Backspace");
    }
    await expectTokensAbsent(pageA, ["지울토큰XYZ"]);
    await expectTokens(pageB, ["살아남을한글"]);
    await persistBody(pageA);
    await expectMatchingPersistAck(pageA, wire);
    await insertSlashTable(pageA);
    await persistBody(pageA);
    await expectMatchingPersistAck(pageA, wire);
    await expect(pageB.locator(".fvoci-editor table")).toBeVisible({ timeout: 15_000 });

    seeded = await editorShape(pageA);
    expect(uniqueBlockIds(seeded).length).toBeGreaterThan(0);
    expect(seeded.table).not.toBeNull();
    expect(seeded.text).toContain("살아남을한글");
    expect(seeded.text).not.toContain("지울토큰XYZ");
    await expectConverged(pageA, pageB);
  } finally {
    await seedA.close();
    await seedB.close();
  }

  await collabApp.crashAndRestart();

  const freshA = await newCollabContext(browser, collabApp.baseUrl);
  const freshB = await newCollabContext(browser, collabApp.baseUrl);
  const restoredA = await freshA.newPage();
  const restoredB = await freshB.newPage();
  try {
    expect(await indexedDbNames(restoredA)).toEqual([]);
    expect(await indexedDbNames(restoredB)).toEqual([]);
    await login(restoredA, member.email, member.password);
    await login(restoredB, member.email, member.password);
    expect(
      (await indexedDbNames(restoredA)).some((name) => /yjs|y-indexeddb|hocus/i.test(name)),
    ).toBe(false);
    await openEditor(restoredA, url);
    expect(seeded).toBeTruthy();
    const restored = await editorShape(restoredA);
    expect(restored).toEqual(seeded);

    await openEditor(restoredB, url);
    await expectTokens(restoredB, ["살아남을한글"]);
    await expectTokensAbsent(restoredB, ["지울토큰XYZ"]);
    await expect(await editorShape(restoredB)).toEqual(seeded);
    await editorLocator(restoredA).click();
    await placeContentCaret(restoredA, "end");
    await restoredA.keyboard.type("후속A");
    await editorLocator(restoredB).click();
    await restoredB.keyboard.press("Enter");
    await restoredB.keyboard.type("후속B");
    await expectTokens(restoredA, ["살아남을한글", "후속A", "후속B"]);
    await expectTokens(restoredB, ["살아남을한글", "후속A", "후속B"]);
    await expectConverged(restoredA, restoredB);
    await expectTokensAbsent(restoredA, ["지울토큰XYZ"]);
    expect((await editorShape(restoredA)).table?.id).toBe(seeded?.table?.id);
  } finally {
    await freshA.close();
    await freshB.close();
  }
});

test("slash attachment and @ mention do not call unsupported APIs", async ({ page }) => {
  const attachmentHits: string[] = [];
  const mentionHits: string[] = [];
  page.on("request", (request) => {
    const url = request.url();
    if (url.includes("/attachments")) attachmentHits.push(url);
    if (url.includes("/search") || url.includes("/lookup") || url.includes("/members")) {
      mentionHits.push(url);
    }
  });
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "미지원 메뉴");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("/첨부");
  await page.keyboard.press("Enter");
  await page.keyboard.type("@");
  expect(attachmentHits).toEqual([]);
  expect(mentionHits).toEqual([]);
});

test("guest still cannot read wiki documents", async ({ page }) => {
  createE2eUser("collab-guest@example.com", "guestpass1", "게스트", {
    familyName: "위키",
    workspaceSlug: "acme",
    membershipRole: "guest",
  });
  await login(page, "collab-guest@example.com", "guestpass1");
  await page.goto("/w/acme/wiki");
  await expect(page.getByText("현재 역할: 게스트")).toBeVisible();
  await expect(page.getByRole("button", { name: "새 문서" })).toHaveCount(0);
});

test("two users show presence and drop it when the peer closes", async ({
  browser,
  collabApp,
}, testInfo) => {
  installCollabPeer();
  const ctxA = await newCollabContext(browser, collabApp.baseUrl);
  const ctxB = await newCollabContext(browser, collabApp.baseUrl);
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  const wireA = attachCollabWire(pageA);
  const wireB = attachCollabWire(pageB);
  try {
    await login(pageA, member.email, member.password);
    const doc = await createWikiDoc(pageA, "프레즌스");
    await openEditor(pageA, doc.url);
    await login(pageB, peer.email, peer.password);
    await openEditor(pageB, doc.url);
    await expect(pageA.getByLabel(/동시 접속 1명/)).toBeVisible({ timeout: 15_000 });
    await expect(pageB.getByLabel(/동시 접속 1명/)).toBeVisible();
    await expect(pageA.getByRole("button", { name: `${PEER_PRESENCE} 커서 위치로 이동` })).toBeVisible();
    await expect(pageB.getByRole("button", { name: `${MEMBER_PRESENCE} 커서 위치로 이동` })).toBeVisible();
    await ctxB.close();
    await expect(pageA.getByLabel(/동시 접속 \d+명/)).toBeHidden({ timeout: 20_000 });
    await expect(pageA.getByRole("button", { name: `${PEER_PRESENCE} 커서 위치로 이동` })).toHaveCount(0);
  } finally {
    const summary = {
      a: { sent: wireA.sent.map((frame) => frame.kind), received: wireA.received.map((frame) => frame.kind) },
      b: { sent: wireB.sent.map((frame) => frame.kind), received: wireB.received.map((frame) => frame.kind) },
    };
    await testInfo.attach("collab-wire-kinds.json", {
      body: Buffer.from(JSON.stringify(summary)),
      contentType: "application/json",
    });
    await ctxA.close();
    await ctxB.close().catch(() => undefined);
  }
});
