import type { components } from "../src/generated/api";
/**
 * Product /collab acceptance for two real FvociEditor clients.
 * Registered separately in the collaboration-flow CI job. Full product
 * acceptance still requires the recorded security/review gates. Invocation:
 * FVOCI_E2E_PENDING=1 bash scripts/run-web-e2e.sh
 */
import {
  admin,
  applyBoldToSelection,
  applyLinkToSelection,
  attachCollabWire,
  closeCollabContext,
  createE2eUser,
  createWikiDoc,
  editorLocator,
  editorShape,
  ensureCollabFixture,
  ensureInstanceSetup,
  expectAwarenessTokenNotSession,
  expectConverged,
  expectMatchingPersistAck,
  expectNotDurablySaved,
  expectTokens,
  expectTokensAbsent,
  indexedDbNames,
  attachmentNodes,
  insertSlashAttachment,
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
  installCaretProbe,
  readCaretProbe,
  readEditorSelection,
  sentPersistRequests,
  sessionCookie,
  storedAttachmentDownloadBytes,
  test,
  expect,
  uniqueBlockIds,
  UUID_RE,
  waitConnected,
  workspaceId,
} from "./collab-helpers";

// One worker preserves setup order. Scenarios own separate documents/users;
// a failed scenario must not skip the remaining independent acceptance cases.

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

test("content caret glyph fallback stays before a trailing empty table", async ({ page }) => {
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "텍스트 끝 caret");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("가나다");
  await insertSlashTable(page);
  const before = await editorShape(page);
  expect(before.table).not.toBeNull();

  // Force the valid opposite side of the last content glyph. This exercises
  // the native-key fallback deterministically, including the empty table edge.
  const click = page.mouse.click.bind(page.mouse);
  page.mouse.click = async (_x, _y, options) => {
    const rect = await editor.evaluate((root) => {
      const text = root.querySelector("p")?.firstChild;
      if (!text) throw new Error("selection fixture requires paragraph text");
      const range = document.createRange();
      range.setStart(text, 2);
      range.setEnd(text, 3);
      const rect = range.getBoundingClientRect();
      return { x: rect.left + 0.1, y: rect.top + rect.height / 2 };
    });
    return click(rect.x, rect.y, options);
  };
  try {
    await placeContentCaret(page, "end");
  } finally {
    page.mouse.click = click;
  }
  const selection = await readEditorSelection(page);
  expect([selection.from, selection.to]).toEqual([4, 4]);
  await page.keyboard.type("끝");
  const after = await editorShape(page);
  expect(after.text).toBe("가나다끝");
  expect(after.table).toEqual(before.table);
  await persistBody(page);
  await page.reload();
  await waitConnected(page);
  expect(await editorShape(page)).toEqual(after);
});

for (const edge of ["start", "end"] as const) {
  test(`content caret accepts ${edge} settling after an adjacent sample`, async ({ page }) => {
    await ensureCollabFixture(page);
    await login(page, member.email, member.password);
    const doc = await createWikiDoc(page, `caret observation ${edge}`);
    const editor = await openEditor(page, doc.url);
    await editor.click();
    await page.keyboard.type("가나다");
    await insertSlashTable(page);
    const before = await editorShape(page);
    expect(before.table).not.toBeNull();
    const desired = edge === "start" ? 1 : 4;
    const adjacent = edge === "start" ? 2 : 3;
    const points = await editor.evaluate((root, edge) => {
      const text = root.querySelector("p")?.firstChild;
      if (!text) throw new Error("selection fixture requires paragraph text");
      const range = document.createRange();
      range.setStart(text, edge === "start" ? 0 : 2);
      range.setEnd(text, edge === "start" ? 1 : 3);
      const rect = range.getBoundingClientRect();
      return {
        desiredX: edge === "start" ? rect.left + 0.1 : rect.right - 0.1,
        adjacentX: edge === "start" ? rect.right - 0.1 : rect.left + 0.1,
        y: rect.top + rect.height / 2,
      };
    }, edge);
    const click = page.mouse.click.bind(page.mouse);
    const evaluate = editor.evaluate.bind(editor);
    const locate = page.locator.bind(page);
    const press = page.keyboard.press.bind(page.keyboard);
    let captured: unknown;
    let settled: unknown;
    let correctionKeys = 0;
    // Deliver an actual adjacent selection observation after a second native
    // click has settled at the requested edge. This controls the ordering of
    // the observation response without assigning DOM or PM selections.
    page.mouse.click = async (_x, _y, options) => click(points.adjacentX, points.y, options);
    page.locator = (selector, options) =>
      selector === ".fvoci-editor .ProseMirror" ? editor : locate(selector, options);
    editor.evaluate = async (pageFunction, arg, options) => {
      const sample = await evaluate(pageFunction, arg, options);
      if (captured === undefined && Array.isArray(sample) && sample.length === 2) {
        expect(sample).toEqual([adjacent, adjacent]);
        captured = sample;
        await click(points.desiredX, points.y);
        await expect
          .poll(() =>
            evaluate((root) => {
              const live = (
                root as HTMLElement & {
                  editor: { state: { selection: { from: number; to: number } } };
                }
              ).editor;
              return [live.state.selection.from, live.state.selection.to];
            }),
          )
          .toEqual([desired, desired]);
        settled = await readEditorSelection(page);
      }
      return sample;
    };
    page.keyboard.press = async (key, options) => {
      if (key === "ArrowLeft" || key === "ArrowRight") correctionKeys++;
      return press(key, options);
    };
    try {
      await placeContentCaret(page, edge);
    } finally {
      page.mouse.click = click;
      page.locator = locate;
      editor.evaluate = evaluate;
      page.keyboard.press = press;
    }
    expect(captured).toEqual([adjacent, adjacent]);
    expect(settled).toMatchObject({ from: desired, to: desired });
    expect(correctionKeys).toBe(0);
    const selection = await readEditorSelection(page);
    expect([selection.from, selection.to]).toEqual([desired, desired]);
    await page.keyboard.type("끝");
    const after = await editorShape(page);
    expect(after.text).toBe(edge === "start" ? "끝가나다" : "가나다끝");
    expect(after.table).toEqual(before.table);
    await persistBody(page);
    await page.reload();
    await waitConnected(page);
    expect(await editorShape(page)).toEqual(after);
  });
}

for (const invalid of ["unrelated", "range"] as const) {
  test(`content caret rejects a settled ${invalid} selection`, async ({ page }) => {
    await ensureCollabFixture(page);
    await login(page, member.email, member.password);
    const doc = await createWikiDoc(page, `invalid caret ${invalid}`);
    const editor = await openEditor(page, doc.url);
    await editor.click();
    await page.keyboard.type("가나다");
    const before = await editorShape(page);
    const click = page.mouse.click.bind(page.mouse);
    const press = page.keyboard.press.bind(page.keyboard);
    let correctionKeys = 0;
    page.mouse.click = async (_x, _y, options) => {
      const point = await editor.evaluate((root) => {
        const text = root.querySelector("p")?.firstChild;
        if (!text) throw new Error("selection fixture requires paragraph text");
        const range = document.createRange();
        range.setStart(text, 2);
        range.setEnd(text, 3);
        const rect = range.getBoundingClientRect();
        return { x: rect.right - 0.1, y: rect.top + rect.height / 2 };
      });
      await click(point.x, point.y, options);
      if (invalid === "range") await press("Shift+ArrowLeft");
      await expect
        .poll(async () => {
          const selection = await readEditorSelection(page);
          return [selection.from, selection.to];
        })
        .toEqual(invalid === "range" ? [3, 4] : [4, 4]);
    };
    page.keyboard.press = async (key, options) => {
      if (key === "ArrowLeft" || key === "ArrowRight") correctionKeys++;
      return press(key, options);
    };
    try {
      await expect(placeContentCaret(page, "start")).rejects.toThrow("Timeout 5000ms");
    } finally {
      page.mouse.click = click;
      page.keyboard.press = press;
    }
    expect(correctionKeys).toBe(0);
    const selection = await readEditorSelection(page);
    expect([selection.from, selection.to]).toEqual(invalid === "range" ? [3, 4] : [4, 4]);
    expect(await editorShape(page)).toEqual(before);
  });
}

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
    await ensureCollabFixture(pageA);
    await login(pageA, member.email, member.password);
    await login(pageB, member.email, member.password);
    const doc = await createWikiDoc(pageA, "삽입 삭제 충돌");
    const editorA = await openEditor(pageA, doc.url);
    await editorA.click();
    await pageA.keyboard.type("한글본문");
    await expectTokens(pageA, ["한글본문"]);
    await openEditor(pageB, doc.url);
    await expectTokens(pageB, ["한글본문"]);
    await Promise.all([installCaretProbe(pageA), installCaretProbe(pageB)]);
    try {
      await Promise.all([placeContentCaret(pageA, "start"), placeContentCaret(pageB, "end")]);
    } catch (error) {
      console.info(
        "placeContentCaret failure",
        JSON.stringify({
          a: await readCaretProbe(pageA).catch(() => null),
          b: await readCaretProbe(pageB).catch(() => null),
        }),
      );
      throw error;
    }
    await Promise.all([
      (async () => {
        await pageA.keyboard.type("앞쪽삽입");
      })(),
      (async () => {
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
    await openEditor(pageB, doc.url);
    await expectTokens(pageA, ["안녕🙂세계"]);
    await expectTokens(pageB, ["안녕🙂세계"]);
    await expectConverged(pageA, pageB);
    await Promise.all([installCaretProbe(pageA), installCaretProbe(pageB)]);
    const beforeIds = uniqueBlockIds(await editorShape(pageA));
    await Promise.all([placeContentCaret(pageA, "start"), placeContentCaret(pageB, "end")]);
    await Promise.all([
      (async () => {
        await pageA.keyboard.press("ArrowRight");
        await pageA.keyboard.type("중간");
      })(),
      (async () => {
        await pageB.keyboard.press("ArrowLeft");
        await pageB.keyboard.press("ArrowLeft");
        await pageB.keyboard.press("ArrowLeft");
        await pageB.keyboard.press("Delete");
        console.info("emoji after Delete", await readCaretProbe(pageB));
      })(),
    ]);
    await expectTokens(pageA, ["안중간녕세계"]);
    await expectTokens(pageB, ["안중간녕세계"]);
    await expectTokensAbsent(pageA, ["🙂"]);
    await expectTokensAbsent(pageB, ["🙂"]);
    await expectConverged(pageA, pageB);
    expect(uniqueBlockIds(await editorShape(pageA))).toEqual(beforeIds);
    expect(uniqueBlockIds(await editorShape(pageB))).toEqual(beforeIds);
  } catch (error) {
    console.info("concurrent emoji Delete failure", {
      a: await readCaretProbe(pageA).catch(() => null),
      b: await readCaretProbe(pageB).catch(() => null),
    });
    throw error;
  } finally {
    await ctxA.close();
    await ctxB.close();
  }
});

// Keep the insertion/deletion race above independent of caret association at the
// same boundary. Separately exercise native Delete beside a remote caret, with
// an otherwise identical control whose remote caret is at the document start.
// These two cases observe settled selection; the unpaced race above does not.
// Delete is a real key event; ProseMirror performs the emoji atom deletion.
for (const remotePosition of ["adjacent", "start"] as const) {
  test(`native emoji Delete with remote caret ${remotePosition}`, async ({
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
      const doc = await createWikiDoc(pageA, `이모지 삭제 커서 ${remotePosition}`);
      const editorA = await openEditor(pageA, doc.url);
      await editorA.click();
      await pageA.keyboard.type("안녕🙂세계");
      await openEditor(pageB, doc.url);
      await expectTokens(pageB, ["안녕🙂세계"]);
      await expectConverged(pageA, pageB);
      const ids = uniqueBlockIds(await editorShape(pageA));
      await Promise.all([installCaretProbe(pageA), installCaretProbe(pageB)]);
      await placeContentCaret(pageA, "start");
      if (remotePosition === "adjacent") {
        await pageA.keyboard.press("ArrowRight");
        await pageA.keyboard.press("ArrowRight");
      }
      const remoteCaretPosition = remotePosition === "adjacent" ? 3 : 1;
      await expect
        .poll(() =>
          editorLocator(pageB).evaluate((root) => {
            const live = (
              root as HTMLElement & {
                editor?: { view: { posAtDOM(node: Node, offset: number): number } };
              }
            ).editor;
            const caret = root.querySelector(".collaboration-carets__caret");
            return live && caret ? live.view.posAtDOM(caret, 0) : null;
          }),
        )
        .toBe(remoteCaretPosition);
      await placeContentCaret(pageB, "end");
      await pageB.keyboard.press("ArrowLeft");
      await pageB.keyboard.press("ArrowLeft");
      await pageB.keyboard.press("ArrowLeft");
      // Observe the requested native selection; do not repair it or dispatch a
      // Tiptap transaction. The emoji is an atom, so PM textContent omits it.
      await expect
        .poll(async () => {
          const probe = (await readCaretProbe(pageB)) as {
            current?: { from: number; to: number; nativePmPos: number; focused: boolean };
          };
          const selection = probe.current;
          return (
            selection && [selection.from, selection.to, selection.nativePmPos, selection.focused]
          );
        })
        .toEqual([3, 3, 3, true]);
      await pageB.keyboard.press("Delete");
      await expectTokensAbsent(pageB, ["🙂"]);
      await expectTokensAbsent(pageA, ["🙂"]);
      await expectTokens(pageA, ["안녕세계"]);
      await expectTokens(pageB, ["안녕세계"]);
      await expectConverged(pageA, pageB);
      expect(uniqueBlockIds(await editorShape(pageA))).toEqual(ids);
      expect(uniqueBlockIds(await editorShape(pageB))).toEqual(ids);
      failed = false;
    } finally {
      if (failed) {
        console.info("native emoji Delete failure", remotePosition, {
          a: await readCaretProbe(pageA).catch(() => null),
          b: await readCaretProbe(pageB).catch(() => null),
        });
      }
      await Promise.all([closeCollabContext(ctxA, failed), closeCollabContext(ctxB, failed)]);
    }
  });
}

async function blurEditorToTitle(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("textbox", { name: "문서 제목" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () => document.activeElement?.classList.contains("document-page__title") ?? false,
      ),
    )
    .toBe(true);
}

test("host-padding focus(end) types at the document end", async ({ page }) => {
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "포커스 끝");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("first");
  await page.keyboard.press("Enter");
  await page.keyboard.type("second");
  await expectTokens(page, ["first", "second"]);
  await blurEditorToTitle(page);
  await page.locator(".fvoci-editor").dispatchEvent("mousedown");
  await expect
    .poll(() =>
      editor.evaluate((root) => {
        const live = (
          root as HTMLElement & {
            editor?: {
              view: { hasFocus(): boolean };
              state: {
                selection: { from: number; to: number };
                doc: { content: { size: number } };
              };
            };
          }
        ).editor;
        if (!live) return null;
        const end = live.state.doc.content.size - 1;
        return [live.view.hasFocus(), live.state.selection.from, live.state.selection.to, end];
      }),
    )
    .toEqual([true, 14, 14, 14]);
  await page.keyboard.type("X");
  await page.keyboard.press("Enter");
  await page.keyboard.type("Y");
  const shape = await editorShape(page);
  expect(shape.text).toBe("firstsecondXY");
  expect(shape.blocks.map((block) => block.text)).toEqual(["first", "secondX", "Y"]);
});

test("blurred insertContent types at the intended position", async ({ page }) => {
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "블러 삽입");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("second");
  await expectTokens(page, ["second"]);
  await blurEditorToTitle(page);
  await editor.evaluate((root) => {
    const live = (
      root as HTMLElement & {
        editor?: {
          chain(): { focus(): { insertContent(content: string): { run(): boolean } } };
        };
      }
    ).editor;
    if (!live) throw new Error("missing live editor");
    live.chain().focus().insertContent("Z").run();
  });
  await expect.poll(async () => (await editorShape(page)).text).toBe("secondZ");
  await expect
    .poll(() =>
      editor.evaluate((root) => {
        const live = (
          root as HTMLElement & {
            editor?: { view: { hasFocus(): boolean } };
          }
        ).editor;
        return live?.view.hasFocus() ?? false;
      }),
    )
    .toBe(true);
  await page.keyboard.type("W");
  expect((await editorShape(page)).text).toBe("secondZW");
});

test("focus(pos) types at the requested document position", async ({ page }) => {
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "포커스 위치");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("first");
  await expectTokens(page, ["first"]);
  await blurEditorToTitle(page);
  await editor.evaluate((root) => {
    const live = (
      root as HTMLElement & {
        editor?: { commands: { focus(pos: number): boolean } };
      }
    ).editor;
    if (!live) throw new Error("missing live editor");
    live.commands.focus(3);
  });
  await expect
    .poll(() =>
      editor.evaluate((root) => {
        const live = (
          root as HTMLElement & {
            editor?: {
              view: { hasFocus(): boolean };
              state: { selection: { from: number; to: number } };
            };
          }
        ).editor;
        if (!live) return null;
        return [live.view.hasFocus(), live.state.selection.from, live.state.selection.to];
      }),
    )
    .toEqual([true, 3, 3]);
  await page.keyboard.type("Q");
  expect((await editorShape(page)).text).toBe("fiQrst");
});

test("offline typing reconnects with unsent text and without a persist ack", async ({
  page,
  context,
  collabApp,
}) => {
  const wire = attachCollabWire(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "재접속");
  const editor = await openEditor(page, doc.url);
  await collabApp.shutdownGraceful();
  try {
    await expect(page.locator('[data-collab-status="connected"]')).toHaveCount(0);
    await expect(editor).toBeVisible();
    await editor.focus();
    await page.keyboard.type("오프라인에서 쓴 줄");
    await expect(editor).toContainText("오프라인에서 쓴 줄");
    await expectNotDurablySaved(page);
    expect(sentPersistRequests(wire)).toEqual([]);
  } finally {
    await collabApp.recycle();
  }
  await waitConnected(page);
  await expect(editor).toContainText("오프라인에서 쓴 줄");
  await expectNotDurablySaved(page);
  expect(sentPersistRequests(wire)).toEqual([]);
  const observer = await context.newPage();
  try {
    await openEditor(observer, doc.url);
    await expectTokens(observer, ["오프라인에서 쓴 줄"]);
    await expectConverged(page, observer);
  } finally {
    await observer.close();
  }
});

test("archived document stays connected and read-only", async ({ page }) => {
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "보관 문서");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("보관 전 문장");
  await persistBody(page);
  await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
  await expect(page.getByLabel("문서 상태")).toBeVisible();
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

test("membership revoke while connected stops further edits", async ({ browser, collabApp }) => {
  installCollabPeer();
  const ownerCtx = await newCollabContext(browser, collabApp.baseUrl);
  const memberCtx = await newCollabContext(browser, collabApp.baseUrl);
  const ownerPage = await ownerCtx.newPage();
  const memberPage = await memberCtx.newPage();
  try {
    await login(memberPage, peer.email, peer.password);
    const me = await memberPage.request.get("/api/v1/auth/me");
    expect(me.ok()).toBe(true);
    const memberId = ((await me.json()) as components["schemas"]["SessionUserOutput"]).userId;

    await login(ownerPage, admin.email, admin.password);
    const doc = await createWikiDoc(ownerPage, "철회 문서");
    const editor = await openEditor(memberPage, doc.url);
    await editor.click();
    await memberPage.keyboard.type("철회 전 문장");
    await expect(editor).toContainText("철회 전 문장");

    const ws = await workspaceId(ownerPage, admin.workspaceSlug);
    const revoke = await ownerPage.request.delete(`/api/v1/workspaces/${ws}/members/${memberId}`);
    expect(revoke.ok()).toBe(true);

    // Membership loss is reconciled via workspace access-stream → home eviction, not
    // the in-document collab unauthorized badge (see task-stream-resync / workspace-flow).
    await expect(memberPage).toHaveURL(/\?denied=workspace$/, { timeout: 20_000 });
    await expect(memberPage.getByRole("alert")).toContainText("접근 권한");
    await expect(editorLocator(memberPage)).toHaveCount(0);

    await memberPage.goto(doc.url);
    await expect(memberPage).toHaveURL(/\?denied=workspace$/);
    await expect(editorLocator(memberPage)).toHaveCount(0);
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
  await editor.click();
  await page.keyboard.press("Control+A");
  await page.keyboard.press("Backspace");
  await expectNotDurablySaved(page);
  await persistBody(page);
  const secondId = sentPersistRequests(wire).at(-1);
  expect(secondId).not.toBe(firstId);
  await expectMatchingPersistAck(page, wire);

  let reconnects = 0;
  page.on("websocket", (socket) => {
    if (socket.url().includes("/collab")) reconnects += 1;
  });
  const authFramesBefore = wire.received.filter((frame) => frame.kind === "auth-scope").length;
  await installCaretProbe(page);
  await editor.click();
  await page.keyboard.type("굵은링크");
  try {
    // Observe the native input precondition; never repair focus or selection.
    // A failure here distinguishes input/remount trouble from losing Shift+Home.
    await expect
      .poll(
        () =>
          editor.evaluate((root) => {
            const live = (
              root as HTMLElement & {
                editor?: {
                  view: { posAtDOM(node: Node, offset: number): number };
                  state: {
                    selection: { from: number; to: number; empty: boolean };
                    doc: {
                      textContent: string;
                      childCount: number;
                      firstChild: { content: { size: number } } | null;
                    };
                  };
                };
              }
            ).editor;
            const native = window.getSelection();
            const anchor = native?.anchorNode;
            const inside = Boolean(anchor && root.contains(anchor));
            const end =
              live?.state.doc.childCount === 1 && live.state.doc.firstChild
                ? live.state.doc.firstChild.content.size + 1
                : null;
            const selection = live?.state.selection;
            return {
              text: live?.state.doc.textContent ?? null,
              focused: document.activeElement === root || root.contains(document.activeElement),
              editable: root.getAttribute("contenteditable"),
              nativeAtEnd: Boolean(
                inside &&
                native?.isCollapsed &&
                live &&
                anchor &&
                live.view.posAtDOM(anchor, native.anchorOffset) === end,
              ),
              editorAtEnd: Boolean(
                selection?.empty && selection.from === end && selection.to === end,
              ),
            };
          }),
        { message: "native input must finish with the focused caret at the typed paragraph end" },
      )
      .toEqual({
        text: "굵은링크",
        focused: true,
        editable: "true",
        nativeAtEnd: true,
        editorAtEnd: true,
      });

    await page.keyboard.press("Shift+Home");
    // Both native and PM selections must reflect the real keyboard gesture.
    await expect
      .poll(
        async () => {
          const selection = await readEditorSelection(page);
          return { browser: selection.browser, editor: selection.editor };
        },
        { message: "native and editor selection must cover the intended marked text" },
      )
      .toEqual({ browser: "굵은링크", editor: "굵은링크" });
  } catch (error) {
    console.info("caret probe on selection failure", {
      probe: await readCaretProbe(page),
      reconnects,
      newAuthFrames:
        wire.received.filter((frame) => frame.kind === "auth-scope").length - authFramesBefore,
    });
    throw error;
  }
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
  expect(restored).toEqual(structured);
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
  const started = Date.now();
  const phase = (name: string) => {
    console.info("crash recovery phase", {
      name,
      elapsedMs: Date.now() - started,
    });
  };
  const logSelection = async (page: import("@playwright/test").Page, label: string) => {
    console.info("crash recovery selection", {
      label,
      ...(await readEditorSelection(page)),
    });
  };
  const logEditorText = async (page: import("@playwright/test").Page, label: string) => {
    console.info("crash recovery editor text", {
      label,
      text: (await editorShape(page)).text,
    });
  };
  const seedA = await newCollabContext(browser, collabApp.baseUrl);
  const seedB = await newCollabContext(browser, collabApp.baseUrl);
  const pageA = await seedA.newPage();
  const pageB = await seedB.newPage();
  const wire = attachCollabWire(pageA);
  let url = "";
  let seeded: Awaited<ReturnType<typeof editorShape>> | undefined;
  let seedBodyFailed = false;
  let seedContextsOpen = true;
  const ensureSeedContextsClosed = async () => {
    if (!seedContextsOpen) return;
    seedContextsOpen = false;
    await Promise.all([
      closeCollabContext(seedA, seedBodyFailed),
      closeCollabContext(seedB, seedBodyFailed),
    ]);
  };
  try {
    await test.step("authenticate independent seed clients", () =>
      Promise.all([
        login(pageA, member.email, member.password),
        login(pageB, member.email, member.password),
      ]));
    phase("seed clients authenticated");
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
    await expectConverged(pageA, pageB);
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
  } catch (error) {
    seedBodyFailed = true;
    throw error;
  } finally {
    if (seedBodyFailed) {
      await ensureSeedContextsClosed();
    }
  }
  try {
    await test.step("SIGKILL owned process tree while collaboration helper is live", () =>
      collabApp.crashKillWhenHelperLive());
    phase("process tree crashed with live helper");
    await ensureSeedContextsClosed();
    phase("persist acknowledged and old clients closed");
    await test.step("restart owned server from DB", () => collabApp.rebindAfterCrash());
    phase("server restarted");
  } finally {
    await ensureSeedContextsClosed();
  }

  const freshA = await newCollabContext(browser, collabApp.baseUrl);
  const freshB = await newCollabContext(browser, collabApp.baseUrl);
  const restoredA = await freshA.newPage();
  const restoredB = await freshB.newPage();
  const restoredWires = [attachCollabWire(restoredA), attachCollabWire(restoredB)];
  let restoreBodyFailed = false;
  try {
    await test.step("authenticate independent fresh clients", () =>
      Promise.all([
        login(restoredA, member.email, member.password),
        login(restoredB, member.email, member.password),
      ]));
    phase("fresh clients authenticated");
    expect(await indexedDbNames(restoredA)).toEqual([]);
    expect(await indexedDbNames(restoredB)).toEqual([]);
    expect(
      (await indexedDbNames(restoredA)).some((name) => /yjs|y-indexeddb|hocus/i.test(name)),
    ).toBe(false);
    await openEditor(restoredA, url);
    expect(seeded).toBeTruthy();
    const restored = await editorShape(restoredA);
    expect(restored).toEqual(seeded);
    phase("fresh client recovered exact structure from DB");

    await test.step("open second fresh client on recovered document", async () => {
      await openEditor(restoredB, url);
      await expectTokens(restoredB, ["살아남을한글"]);
      await expectTokensAbsent(restoredB, ["지울토큰XYZ"]);
      expect(await editorShape(restoredB)).toEqual(seeded);
    });
    phase("second fresh client verified against DB structure");

    await test.step("subsequent A edit at document end", async () => {
      await editorLocator(restoredA).click();
      await logSelection(restoredA, "restoredA before placeContentCaret end");
      await placeContentCaret(restoredA, "end");
      await logSelection(restoredA, "restoredA after placeContentCaret end");
      await restoredA.keyboard.type("후속A");
      await logSelection(restoredA, "restoredA after type 후속A");
      await logEditorText(restoredA, "restoredA after type 후속A");
      await logEditorText(restoredB, "restoredB after A type 후속A");
    });
    phase("client A typed subsequent edit");

    await test.step("subsequent B edit at document start", async () => {
      await editorLocator(restoredB).focus();
      await logSelection(restoredB, "restoredB after focus before Control+Home");
      await restoredB.keyboard.press("Control+Home");
      await logSelection(restoredB, "restoredB after Control+Home");
      await restoredB.keyboard.press("Enter");
      await logSelection(restoredB, "restoredB after Enter");
      await restoredB.keyboard.type("후속B");
      await logSelection(restoredB, "restoredB after type 후속B");
      await logEditorText(restoredB, "restoredB after type 후속B");
      await logEditorText(restoredA, "restoredA after B type 후속B");
    });
    phase("client B typed subsequent edit");

    await test.step("subsequent edits converged across fresh clients", async () => {
      await expectTokens(restoredB, ["후속B"]);
      await expectTokens(restoredA, ["살아남을한글", "후속A", "후속B"]);
      await expectTokens(restoredB, ["살아남을한글", "후속A", "후속B"]);
      await expectConverged(restoredA, restoredB);
      await expectTokensAbsent(restoredA, ["지울토큰XYZ"]);
      expect((await editorShape(restoredA)).table?.id).toBe(seeded.table?.id);
    });
    phase("subsequent edits converged");
  } catch (error) {
    restoreBodyFailed = true;
    // Only frame categories: never print cookies, auth tokens or document data.
    for (const [client, wire] of restoredWires.entries()) {
      const category = (frame: (typeof wire.sent)[number]) =>
        frame.kind === "other" ? `document-type-${String(frame.type)}` : frame.kind;
      console.info("crash recovery fresh wire", {
        client,
        sentCount: wire.sent.length,
        receivedCount: wire.received.length,
        sentTail: wire.sent.slice(-32).map(category),
        receivedTail: wire.received.slice(-32).map(category),
      });
    }
    throw error;
  } finally {
    await Promise.all([
      closeCollabContext(freshA, restoreBodyFailed),
      closeCollabContext(freshB, restoreBodyFailed),
    ]);
  }
});

test("@ mention uses authorized suggestions and preserves the selected user after reload", async ({
  page,
}) => {
  await ensureCollabFixture(page);
  // Other scenarios revoke the shared peer; own this suggestion's membership.
  const mentionPeer = {
    ...peer,
    email: "collab-mention-peer@example.com",
    givenName: "제안",
    familyName: "멘션",
  };
  const mentionLabel = "멘션제안";
  installCollabPeer(mentionPeer);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "멤버 멘션");
  const editor = await openEditor(page, doc.url);
  const membersPath = `/api/v1/workspaces/${doc.workspaceId}/members`;
  const groupsPath = `/api/v1/workspaces/${doc.workspaceId}/groups`;
  // Empty @ queries only load these two contracted metadata collections.
  // Check exact paths/methods/query strings, including enrichment after reload;
  // search, lookup and legacy mention/user endpoints must not sneak through.
  const mentionHits: Array<{ method: string; path: string; query: string }> = [];
  const failedApiResponses: Array<{ path: string; status: number }> = [];
  let selectingMention = true;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (
      url.pathname.startsWith("/api/") &&
      (selectingMention ||
        /\/(search|lookup|members|groups|mentions?|users)(\/|$)/.test(url.pathname))
    ) {
      mentionHits.push({ method: request.method(), path: url.pathname, query: url.search });
    }
  });
  page.on("response", (response) => {
    const path = new URL(response.url()).pathname;
    if (path.startsWith("/api/") && response.status() >= 400) {
      failedApiResponses.push({ path, status: response.status() });
    }
  });
  const membersResponse = page.waitForResponse(
    (response) => new URL(response.url()).pathname === membersPath,
  );
  const groupsResponse = page.waitForResponse(
    (response) => new URL(response.url()).pathname === groupsPath,
  );
  await editor.click();
  await page.keyboard.type("@");
  const [members, groups] = await Promise.all([membersResponse, groupsResponse]);
  expect(members.status()).toBe(200);
  expect(groups.status()).toBe(200);
  const body = (await members.json()) as components["schemas"]["MembersResponse"];
  const selected = body.items.find((item) => item.email === mentionPeer.email);
  expect(selected).toMatchObject({
    givenName: mentionPeer.givenName,
    familyName: mentionPeer.familyName,
    role: "member",
  });
  if (!selected) throw new Error("mention fixture member missing");
  expect(selected.userId).toMatch(UUID_RE);
  await expect(
    page.getByRole("listbox").getByRole("option", { name: mentionLabel, exact: true }),
  ).toBeVisible();
  await page.getByRole("listbox").getByRole("option", { name: mentionLabel, exact: true }).click();
  await expect(editor.locator("[data-mention]")).toHaveText(`@${mentionLabel}`);
  const before = await editorShape(page);
  expect(
    before.document.content
      ?.flatMap((node) => node.content ?? [])
      .filter((node) => node.type === "mention"),
  ).toEqual([
    { type: "mention", attrs: { entity: "user", id: selected.userId, label: mentionLabel } },
  ]);
  selectingMention = false;
  await persistBody(page);
  await page.reload();
  await waitConnected(page);
  expect((await editorShape(page)).document).toEqual(before.document);
  await expect(editorLocator(page).locator("[data-mention]")).toHaveText(`@${mentionLabel}`);
  expect(mentionHits).toContainEqual({ method: "GET", path: membersPath, query: "" });
  expect(mentionHits).toContainEqual({ method: "GET", path: groupsPath, query: "" });
  expect(
    mentionHits.filter(
      (hit) =>
        hit.method !== "GET" || hit.query !== "" || ![membersPath, groupsPath].includes(hit.path),
    ),
  ).toEqual([]);
  expect(failedApiResponses).toEqual([]);
});

test("@ mention member metadata denies guests, nonmembers and unauthenticated users", async ({
  page,
}) => {
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const id = await workspaceId(page, admin.workspaceSlug);
  const path = `/api/v1/workspaces/${id}/members`;
  createE2eUser("collab-mention-guest@example.com", "guestpass1", "멘션게스트", {
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "guest",
  });
  createE2eUser("collab-mention-outsider@example.com", "outsiderpass1", "멘션외부인");
  for (const [email, password] of [
    ["collab-mention-guest@example.com", "guestpass1"],
    ["collab-mention-outsider@example.com", "outsiderpass1"],
  ] as const) {
    await login(page, email, password);
    const response = await page.request.get(path);
    expect(response.status()).toBe(404);
    expect(await response.json()).not.toHaveProperty("items");
  }
  await page.context().clearCookies();
  const response = await page.request.get(path);
  expect(response.status()).toBe(401);
  expect(await response.json()).not.toHaveProperty("items");
});

test("slash attachment uploads, shows metadata, downloads bytes, and survives persist reload", async ({
  page,
}) => {
  const uploadHits: string[] = [];
  page.on("request", (request) => {
    const url = request.url();
    if (url.includes("/uploads") || url.includes("/attachments/")) uploadHits.push(url);
  });
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "첨부 업로드");
  await openEditor(page, doc.url);
  const fixtureBytes = Buffer.from("fvoci-attachment-e2e\n", "utf8");
  await insertSlashAttachment(page, {
    name: "collab-fixture.bin",
    buffer: fixtureBytes,
  });
  await expect(page.locator(".afn-attachment-badge")).toContainText("application/octet-stream");
  const downloaded = await storedAttachmentDownloadBytes(page);
  expect(downloaded.equals(fixtureBytes)).toBe(true);
  expect(uploadHits.some((url) => url.includes("/uploads"))).toBe(true);
  expect(uploadHits.some((url) => url.includes("/complete"))).toBe(true);
  const before = await editorShape(page);
  const beforeAttachments = attachmentNodes(before);
  expect(beforeAttachments.length).toBeGreaterThan(0);
  expect(beforeAttachments[0]?.name).toBe("collab-fixture.bin");
  expect(beforeAttachments[0]?.attachmentId).toMatch(UUID_RE);
  expect(beforeAttachments[0]?.image).toBe(false);
  await persistBody(page);
  await page.reload();
  await waitConnected(page);
  const after = await editorShape(page);
  expect(attachmentNodes(after)).toEqual(beforeAttachments);
  await expect(page.locator('.afn-attachment[data-state="stored"]')).toBeVisible();
  const reloaded = await storedAttachmentDownloadBytes(page);
  expect(reloaded.equals(fixtureBytes)).toBe(true);
});

test("stored attachment bytes survive owned-server restart", async ({ page, collabApp }) => {
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "첨부 재시작");
  await openEditor(page, doc.url);
  const fixtureBytes = Buffer.from("fvoci-attachment-restart\n", "utf8");
  await insertSlashAttachment(page, {
    name: "restart-fixture.bin",
    buffer: fixtureBytes,
  });
  await persistBody(page);
  const href = await page.locator('.afn-attachment[data-state="stored"]').getAttribute("href");
  expect(href).toBeTruthy();
  if (!href) throw new Error("stored attachment has no download href");
  await collabApp.crashAndRestart();
  await page.reload();
  await waitConnected(page);
  await expect(page.locator('.afn-attachment[data-state="stored"]')).toBeVisible();
  const restarted = await storedAttachmentDownloadBytes(page);
  expect(restarted.equals(fixtureBytes)).toBe(true);
});

test("revoked member cannot download or create wiki attachments", async ({
  browser,
  collabApp,
}) => {
  const revokePeer = {
    ...peer,
    email: "collab-attach-revoke-peer@example.com",
    givenName: "첨부철회",
  };
  installCollabPeer(revokePeer);
  const ownerCtx = await newCollabContext(browser, collabApp.baseUrl);
  const memberCtx = await newCollabContext(browser, collabApp.baseUrl);
  const ownerPage = await ownerCtx.newPage();
  const memberPage = await memberCtx.newPage();
  try {
    await ensureCollabFixture(ownerPage);
    await login(memberPage, revokePeer.email, revokePeer.password);
    const me = await memberPage.request.get("/api/v1/auth/me");
    expect(me.ok()).toBe(true);
    const memberId = ((await me.json()) as components["schemas"]["SessionUserOutput"]).userId;

    await login(ownerPage, admin.email, admin.password);
    const doc = await createWikiDoc(ownerPage, "첨부 철회");
    await openEditor(memberPage, doc.url);
    const fixtureBytes = Buffer.from("fvoci-attachment-revoke\n", "utf8");
    await insertSlashAttachment(memberPage, {
      name: "revoke-fixture.bin",
      buffer: fixtureBytes,
    });
    const href = await memberPage
      .locator('.afn-attachment[data-state="stored"]')
      .getAttribute("href");
    expect(href).toBeTruthy();
    if (!href) throw new Error("stored attachment has no download href");

    const ws = await workspaceId(ownerPage, admin.workspaceSlug);
    const revoke = await ownerPage.request.delete(`/api/v1/workspaces/${ws}/members/${memberId}`);
    expect(revoke.ok()).toBe(true);

    const revokedDownload = await memberPage.request.get(href);
    expect(revokedDownload.status()).toBe(404);
    const revokedCreate = await memberPage.request.post(
      `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/uploads`,
      {
        data: {
          name: "blocked-after-revoke.bin",
          sizeBytes: 8,
        },
      },
    );
    expect(revokedCreate.status()).toBe(404);
  } finally {
    await ownerCtx.close();
    await memberCtx.close();
  }
});

test("guest attachment upload and download are denied by the product APIs", async ({ page }) => {
  await ensureCollabFixture(page);
  createE2eUser("collab-attach-guest@example.com", "guestpass1", "첨부게스트", {
    familyName: "위키",
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "guest",
  });
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "첨부 권한");
  await openEditor(page, doc.url);
  await insertSlashAttachment(page, {
    name: "guest-deny.bin",
    buffer: Buffer.from("guest-deny\n", "utf8"),
  });
  const href = await page.locator('.afn-attachment[data-state="stored"]').getAttribute("href");
  expect(href).toBeTruthy();
  if (!href) throw new Error("stored attachment has no download href");
  await page.context().clearCookies();
  await login(page, "collab-attach-guest@example.com", "guestpass1");
  const guestDownload = await page.request.get(href);
  expect(guestDownload.status()).toBe(404);
  const guestCreate = await page.request.post(
    `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/uploads`,
    {
      data: {
        name: "blocked.bin",
        sizeBytes: 8,
      },
    },
  );
  expect(guestCreate.status()).toBe(404);
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
  const presencePeer = { ...peer, email: "collab-presence@example.com" };
  installCollabPeer(presencePeer);
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
    await login(pageB, presencePeer.email, presencePeer.password);
    await openEditor(pageB, doc.url);
    await expect(pageA.getByLabel(/동시 접속 1명/)).toBeVisible({ timeout: 15_000 });
    await expect(pageB.getByLabel(/동시 접속 1명/)).toBeVisible();
    await expect(
      pageA.getByRole("button", { name: `${PEER_PRESENCE} 커서 위치로 이동` }),
    ).toBeVisible();
    await expect(
      pageB.getByRole("button", { name: `${MEMBER_PRESENCE} 커서 위치로 이동` }),
    ).toBeVisible();
    await ctxB.close();
    await expect(pageA.getByLabel(/동시 접속 \d+명/)).toBeHidden({ timeout: 20_000 });
    await expect(
      pageA.getByRole("button", { name: `${PEER_PRESENCE} 커서 위치로 이동` }),
    ).toHaveCount(0);
  } finally {
    const summary = {
      a: {
        sent: wireA.sent.map((frame) => frame.kind),
        received: wireA.received.map((frame) => frame.kind),
      },
      b: {
        sent: wireB.sent.map((frame) => frame.kind),
        received: wireB.received.map((frame) => frame.kind),
      },
    };
    await testInfo.attach("collab-wire-kinds.json", {
      body: Buffer.from(JSON.stringify(summary)),
      contentType: "application/json",
    });
    await ctxA.close();
    await ctxB.close().catch(() => undefined);
  }
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
    await ensureCollabFixture(pageA);
    await login(pageA, member.email, member.password);
    await login(pageB, member.email, member.password);
    const doc = await createWikiDoc(pageA, "개정 복원");
    const editorA = await openEditor(pageA, doc.url);
    await openEditor(pageB, doc.url);
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
    expect((await editorShape(pageB)).text).toContain("개정 전 본문");
    await pageA.reload();
    await waitConnected(pageA);
    expect((await editorShape(pageA)).text).toContain("개정 전 본문");
    expect((await editorShape(pageA)).text).not.toContain("그리고 더 작성");
  } finally {
    await closeCollabContext(ctxA, false);
    await closeCollabContext(ctxB, false);
  }
});
