import { readFileSync, writeFileSync } from "node:fs";
import { expect, type Page, test } from "@playwright/test";
import { createE2eUser } from "./helpers";
import {
  admin, blockAt, caretAtEndOf, createDoc, editorOf, expectBlocks,
  newSignedInPage, openDoc, save, savedBody, setupInstance, workspaceId,
  type TiptapNode,
} from "./workspace-wiki-vue-editor";

test.beforeAll(async ({ browser, baseURL }) => setupInstance(browser, baseURL));

const paragraph = (text: string) => ({ type: "paragraph", content: [{ type: "text", text }] });
const table = { type: "table", content: [{ type: "tableRow", content: [
  { type: "tableCell", content: [paragraph("표 A")] },
  { type: "tableCell", content: [paragraph("표 B")] },
] }] };
type DragEvidence = { type: string; trusted: boolean };
type InputWindow = Window & typeof globalThis & {
  fvociInputDragEvents: DragEvidence[];
  fvociInputPaste: { trusted: boolean; files: number } | null;
};

// Yjs persistence omits null schema defaults; getJSON fills them back in.
// Preserve all meaningful attributes and document order when comparing peers.
function withoutNullDefaults(node: TiptapNode): TiptapNode {
  const { attrs, content, ...rest } = node;
  const present = Object.fromEntries(Object.entries(attrs ?? {}).filter(([, value]) => value !== null));
  return {
    ...rest,
    ...(Object.keys(present).length ? { attrs: present } : {}),
    ...(content ? { content: content.map(withoutNullDefaults) } : {}),
  };
}

async function writeClipboardImage(page: Page): Promise<void> {
  await page.bringToFront();
  await page.evaluate(async () => {
    const canvas = document.createElement("canvas"); canvas.width = 2; canvas.height = 2;
    canvas.getContext("2d")!.fillRect(0, 0, 2, 2);
    const blob = await new Promise<Blob>((resolve) => canvas.toBlob((blob) => resolve(blob!), "image/png"));
    await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
  });
}

test("native block and table handle drags reach a peer and survive save and reload", async ({ browser, baseURL }) => {
  const a = await newSignedInPage(browser, baseURL, admin);
  const b = await newSignedInPage(browser, baseURL, admin);
  try {
    await a.page.setViewportSize({ width: 1280, height: 1200 });
    const ws = await workspaceId(a.page.request);
    const doc = await createDoc(a.page.request, ws, "실제 드래그", {
      json: { type: "doc", content: [paragraph("첫째"), paragraph("둘째"), table, paragraph("끝")] },
    });
    await openDoc(a.page, doc.path);
    await openDoc(b.page, doc.path);
    await a.page.bringToFront();
    await blockAt(a.page, 0).hover({ position: { x: 8, y: 4 } });
    const handle = a.page.locator('[data-gutter="drag"]');
    await expect(handle).toBeVisible();
    await a.page.evaluate(() => {
      (window as InputWindow).fvociInputDragEvents = [];
      for (const type of ["dragstart", "drop"]) document.addEventListener(type, (event) => {
        (window as InputWindow).fvociInputDragEvents.push({ type, trusted: event.isTrusted });
      }, true);
    });
    // Small initial motion crosses Chromium's drag threshold before moving
    // across the editor. Both source and target fit this visible viewport.
    const source = (await handle.boundingBox())!;
    const target = (await blockAt(a.page, 3).boundingBox())!;
    await a.page.mouse.move(source.x + source.width / 2, source.y + source.height / 2);
    await a.page.mouse.down();
    await a.page.mouse.move(source.x + source.width / 2 + 10, source.y + source.height / 2, { steps: 5 });
    await a.page.mouse.move(target.x + 2, target.y + 2, { steps: 12 });
    await a.page.mouse.up();
    expect(await a.page.evaluate(() => (window as InputWindow).fvociInputDragEvents)).toEqual([
      { type: "dragstart", trusted: true }, { type: "drop", trusted: true },
    ]);
    await expectBlocks(a.page, ["둘째", "표 A표 B", "첫째", "끝"]);
    await expectBlocks(b.page, ["둘째", "표 A표 B", "첫째", "끝"]);

    await editorOf(a.page).locator("td").first().click();
    const tableHandle = a.page.locator('[data-table-handle="table"]');
    await expect(tableHandle).toBeVisible();
    const from = await tableHandle.boundingBox();
    const to = await blockAt(a.page, 0).boundingBox();
    expect(from).toBeTruthy(); expect(to).toBeTruthy();
    await a.page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2);
    await a.page.mouse.down();
    await a.page.mouse.move(to!.x + 10, to!.y + 5, { steps: 12 });
    await a.page.mouse.up();
    await expect(a.page.getByRole("menu", { name: "표", exact: true })).toHaveCount(0);
    await expectBlocks(a.page, ["표 A표 B", "둘째", "첫째", "끝"]);
    await expectBlocks(b.page, ["표 A표 B", "둘째", "첫째", "끝"]);
    await caretAtEndOf(b.page, 3);
    await b.page.keyboard.type(" 동료");
    await expectBlocks(a.page, ["표 A표 B", "둘째", "첫째", "끝 동료"]);
    await save(a.page);
    expect((await savedBody(a.page.request, ws, doc.id)).content?.map((node) => node.type))
      .toEqual(["table", "paragraph", "paragraph", "paragraph"]);
    await openDoc(a.page, doc.path);
    await expectBlocks(a.page, ["표 A표 B", "둘째", "첫째", "끝 동료"]);
  } finally { await a.context.close(); await b.context.close(); }
});

async function dropFiles(page: Page, paths: string[], blockIndex: number): Promise<void> {
  await page.bringToFront();
  const block = blockAt(page, blockIndex);
  await block.scrollIntoViewIfNeeded();
  // The OS file payload enters through Chromium's drag input, not dispatchEvent
  // or the editor upload/insert API. Target the end of this paragraph's text.
  const point = await block.evaluate((element) => {
    const range = document.createRange();
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) => node.parentElement?.closest(".collaboration-carets__caret, .collaboration-carets__label")
        ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
    });
    let text = walker.nextNode()!;
    while (walker.nextNode()) text = walker.currentNode;
    range.setStart(text, text.textContent!.length); range.collapse(true);
    const rect = range.getBoundingClientRect();
    return { x: rect.x + 1, y: rect.y + rect.height / 2 };
  });
  const session = await page.context().newCDPSession(page);
  try {
    const data = { items: [], files: paths, dragOperationsMask: 1 };
    await session.send("Input.dispatchDragEvent", { type: "dragEnter", ...point, data });
    await session.send("Input.dispatchDragEvent", { type: "dragOver", ...point, data });
    await session.send("Input.dispatchDragEvent", { type: "drop", ...point, data });
  } finally { await session.detach(); }
}

test("file drop keeps multi-file order and moving anchors, clipboard paste persists and downloads require access", async ({ browser, baseURL }, testInfo) => {
  const who = { email: "vue-input-peer@example.com", password: "peerpass1", givenName: "파일 동료" };
  createE2eUser(who.email, who.password, who.givenName, { workspaceSlug: admin.workspaceSlug, membershipRole: "member" });
  const a = await newSignedInPage(browser, baseURL, admin, { permissions: ["clipboard-read", "clipboard-write"] });
  const b = await newSignedInPage(browser, baseURL, who);
  const anonymous = await browser.newContext({ baseURL });
  try {
    const ws = await workspaceId(a.page.request);
    const doc = await createDoc(a.page.request, ws, "파일 입력", { markdown: "기준\n\n끝\n" });
    await openDoc(a.page, doc.path); await openDoc(b.page, doc.path);
    const files = ["first.txt", "second.txt"].map((name) => {
      const path = testInfo.outputPath(name); writeFileSync(path, `native drop ${name}\n`); return path;
    });
    const release: Array<() => void> = [];
    await a.page.route(`**/documents/${doc.id}/uploads`, async (route) => {
      await new Promise<void>((resolve) => release.push(resolve));
      await route.continue(); // deterministic barrier; real Rust upload, parts and storage
    });
    await dropFiles(a.page, files, 0);
    await expect(a.page.getByRole("progressbar")).toHaveCount(2);
    await expect.poll(() => release.length).toBe(2);
    // A peer inserts before the queued position while both uploads are pending.
    await blockAt(b.page, 0).click(); await b.page.keyboard.press("Home");
    await b.page.keyboard.type("동료 ");
    await expectBlocks(a.page, ["동료 기준", "끝"]);
    release[1]();
    await expect(editorOf(a.page).locator('[data-state="stored"]')).toHaveCount(1);
    release[0]();
    await expect(editorOf(a.page).locator(".afn-attachment-name")).toHaveText(["first.txt", "second.txt"]);
    await expect(editorOf(b.page).locator(".afn-attachment-name")).toHaveText(["first.txt", "second.txt"]);
    await expect(a.page.locator("[data-fvoci-uploads]")).toHaveCount(0);
    await a.page.unroute(`**/documents/${doc.id}/uploads`);

    await writeClipboardImage(a.page);
    await a.page.evaluate(() => {
      (window as InputWindow).fvociInputPaste = null;
      document.addEventListener("paste", (event) => {
        (window as InputWindow).fvociInputPaste = { trusted: event.isTrusted, files: event.clipboardData?.files.length ?? 0 };
      }, { capture: true, once: true });
    });
    // Click the visible text, rather than the centre of the full-width block.
    // Observe PM and the browser agreeing before delivering native paste.
    await blockAt(a.page, 3).click({ position: { x: 8, y: 8 } });
    await a.page.keyboard.press("End");
    await expect.poll(() => editorOf(a.page).evaluate((root) => {
      const editor = (root as HTMLElement & { editor: { state: { selection: { $from: { parent: { textContent: string } } } } } }).editor;
      return { parent: editor.state.selection.$from.parent.textContent, native: window.getSelection()?.anchorNode?.textContent };
    })).toEqual({ parent: "끝", native: "끝" });
    await a.page.keyboard.press("Control+V"); // real browser paste -> FileHandler
    expect(await a.page.evaluate(() => (window as InputWindow).fvociInputPaste)).toEqual({ trusted: true, files: 1 });
    await expect(editorOf(a.page).locator('[data-state="stored"]')).toHaveCount(3);
    await expect(editorOf(b.page).locator('[data-state="stored"]')).toHaveCount(3);
    await save(a.page);
    const body = await savedBody(a.page.request, ws, doc.id);
    expect(body.content?.map((node) => node.type)).toEqual(["paragraph", "attachment", "attachment", "paragraph", "attachment", "paragraph"]);
    expect(body.content?.[0].content?.[0].text).toBe("동료 기준");
    const ids = body.content?.filter((node) => node.type === "attachment").map((node) => node.attrs?.id);
    expect(new Set(ids).size).toBe(3);
    await expect.poll(async () => withoutNullDefaults(await editorOf(b.page).evaluate((root) =>
      (root as HTMLElement & { editor: { getJSON(): TiptapNode } }).editor.getJSON(),
    ))).toEqual(withoutNullDefaults(body));
    await openDoc(a.page, doc.path);
    const cards = editorOf(a.page).locator('a[data-state="stored"]');
    await expect(cards).toHaveCount(3);
    for (let i = 0; i < 3; i += 1) {
      const url = (await cards.nth(i).getAttribute("href"))!;
      const response = await b.page.request.get(url);
      expect(response.status()).toBe(200);
      if (i < 2) expect(await response.body()).toEqual(readFileSync(files[i]));
      else expect((await response.body()).subarray(0, 8)).toEqual(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]));
      const denied = await anonymous.request.get(url);
      expect(denied.status()).toBe(401);
      const downloadPromise = a.page.waitForEvent("download");
      await cards.nth(i).click();
      const download = await downloadPromise;
      expect(await download.failure()).toBeNull();
      expect(readFileSync((await download.path())!)).toEqual(await response.body());
    }
    const me = await b.page.request.get("/api/v1/auth/me");
    const revoke = await a.page.request.delete(`/api/v1/workspaces/${ws}/members/${(await me.json()).userId}`);
    expect(revoke.ok()).toBe(true);
    await expect(b.page).toHaveURL(/\?denied=workspace$/, { timeout: 20_000 });
    const deniedDownload = await b.page.request.get((await cards.first().getAttribute("href"))!);
    expect(deniedDownload.status()).toBe(404);
    const deniedUpload = await b.page.request.post(`/api/v1/workspaces/${ws}/documents/${doc.id}/uploads`, { data: { name: "denied.txt", sizeBytes: 1 } });
    expect(deniedUpload.status()).toBe(404);
    expect(JSON.stringify(await savedBody(a.page.request, ws, doc.id))).toBe(JSON.stringify(body));
  } finally { await a.context.close(); await b.context.close(); await anonymous.close(); }
});

test("an archived read-only wiki refuses file input and body mutation", async ({ browser, baseURL }, testInfo) => {
  const a = await newSignedInPage(browser, baseURL, admin, { permissions: ["clipboard-read", "clipboard-write"] });
  try {
    const ws = await workspaceId(a.page.request);
    const doc = await createDoc(a.page.request, ws, "읽기 전용 입력", { markdown: "보관 본문\n" });
    await openDoc(a.page, doc.path);
    await a.page.getByLabel("문서 상태").selectOption("archived");
    await expect(editorOf(a.page)).toHaveAttribute("contenteditable", "false");
    await expect(a.page.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
    const before = await savedBody(a.page.request, ws, doc.id);
    const uploads: string[] = [];
    a.page.on("request", (request) => {
      if (request.method() === "POST" && request.url().endsWith(`/documents/${doc.id}/uploads`)) uploads.push(request.url());
    });
    const file = testInfo.outputPath("readonly.txt"); writeFileSync(file, "read-only must refuse this");
    await dropFiles(a.page, [file], 0);
    await writeClipboardImage(a.page);
    await blockAt(a.page, 0).click({ position: { x: 8, y: 8 } });
    await a.page.keyboard.press("Control+V");
    await a.page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    await expect(a.page.locator("[data-fvoci-uploads]")).toHaveCount(0);
    expect(uploads).toEqual([]);
    await blockAt(a.page, 0).click();
    await a.page.keyboard.type("denied mutation");
    expect(await savedBody(a.page.request, ws, doc.id)).toEqual(before);
  } finally { await a.context.close(); }
});
