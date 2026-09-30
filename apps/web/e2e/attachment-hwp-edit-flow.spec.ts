import { expectVueViewer } from "./viewer-app";
import fs from "node:fs";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { HwpDocument, initSync } from "@rhwp/core";
import { decodePageText } from "../src/features/attachments/hwp-page";
import { buildFixtureHwpx, FIXTURE_PAGES } from "../src/features/attachments/hwp-test-fixture";
import { watchCspViolations } from "./helpers";

const owner = {
  email: "hwp-edit@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "편집",
  workspaceSlug: "acme",
  workspaceName: "HWP Edit",
};

const repoRoot = path.resolve(import.meta.dirname, "../../..");
const hancomHwpx = new Uint8Array(fs.readFileSync(path.join(repoRoot, "compat/fixtures/sample.hwpx")));
initSync({
  module: fs.readFileSync(path.join(path.dirname(createRequire(import.meta.url).resolve("@rhwp/core")), "rhwp_bg.wasm")),
});

/** Three pages of Korean text in a real OWPML package (see `hwp-test-fixture.ts`). */
const threePageHwpx = Buffer.from(buildFixtureHwpx(hancomHwpx, FIXTURE_PAGES));

/** The same pages as binary HWP 5.0, written by the pinned rhwp in Node. */
const threePageHwp = (() => {
  const doc = new HwpDocument(new Uint8Array(threePageHwpx));
  try {
    return Buffer.from(doc.exportHwp());
  } finally {
    doc.free();
  }
})();

/** What the fixture pages say, one trailing paragraph break each, as rhwp reads them back. */
const pages = FIXTURE_PAGES.map((text) => `${text}\n`);

/** Page texts of a file, read by rhwp the way the viewer would reopen it. */
function readPages(bytes: Uint8Array): string[] {
  const doc = new HwpDocument(bytes);
  try {
    return Array.from({ length: doc.pageCount() }, (_, i) => decodePageText(doc.getPageText(i)));
  } finally {
    doc.free();
  }
}

const sha256 = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");

async function uploadAttachment(page: Page, wsId: string, documentId: string, name: string, bytes: Buffer) {
  const created = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${documentId}/uploads`, {
    data: { name, sizeBytes: bytes.length },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const upload = (await created.json()) as {
    attachmentId: string;
    partSizeBytes: number;
    parts: { partNumber: number; url: string }[];
  };
  const parts: { partNumber: number; etag: string }[] = [];
  for (const part of upload.parts) {
    const put = await page.request.put(part.url, {
      headers: { "content-type": "application/octet-stream" },
      data: bytes.subarray((part.partNumber - 1) * upload.partSizeBytes, part.partNumber * upload.partSizeBytes),
    });
    expect(put.ok(), await put.text()).toBeTruthy();
    parts.push({ partNumber: part.partNumber, etag: put.headers()["etag"]! });
  }
  const complete = await page.request.post(`/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`, {
    data: { parts },
  });
  expect(complete.ok(), await complete.text()).toBeTruthy();
  return upload.attachmentId;
}

/** A small ink fingerprint of the shown page, so a text edit shows up as a different page image. */
async function pageInk(page: Page): Promise<string> {
  const img = page.locator("[data-hwp-viewer] img.hwp-viewer__page");
  await expect(img).toHaveJSProperty("complete", true);
  return img.evaluate((node) => {
    const el = node as HTMLImageElement;
    const canvas = document.createElement("canvas");
    canvas.width = el.naturalWidth;
    canvas.height = el.naturalHeight;
    const ctx = canvas.getContext("2d")!;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(el, 0, 0);
    const { data, width, height } = ctx.getImageData(0, 0, canvas.width, canvas.height);
    const grid = 32;
    const cells = new Array<number>(grid * grid).fill(0);
    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const at = (y * width + x) * 4;
        if (data[at]! < 110) cells[Math.floor((y * grid) / height) * grid + Math.floor((x * grid) / width)]! += 1;
      }
    }
    return cells.join(",");
  });
}

test("HWP/HWPX 간단 편집: replace, 0-count, revert, draft download, save-copy, current deny and the unsaved-edit guard", async ({
  page,
  browser,
}) => {
  test.setTimeout(240_000);
  const csp = watchCspViolations(page);
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  const editCopyCalls: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/edit-copy")) editCopyCalls.push(request.method());
  });
  const editCopies: string[] = [];
  page.on("response", async (response) => {
    if (new URL(response.url()).pathname.endsWith("/edit-copy") && response.status() === 201) {
      editCopies.push(((await response.json()) as { attachmentId: string }).attachmentId);
    }
  });

  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  // An unknown path still falls back to home under the data router.
  await page.goto("/no-such-route");
  await expect(page).toHaveURL(/\/$/);

  const workspaces = (await (await page.request.get("/api/v1/me/workspaces")).json()) as {
    items: { id: string; slug: string }[];
  };
  const wsId = workspaces.items.find((item) => item.slug === owner.workspaceSlug)!.id;
  const docRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: "HWP 편집" },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const documentId = ((await docRes.json()) as { id: string }).id;
  const hwpxId = await uploadAttachment(page, wsId, documentId, "품의서.hwpx", threePageHwpx);
  const hwpId = await uploadAttachment(page, wsId, documentId, "보고서.hwp", threePageHwp);

  const viewer = page.locator("[data-hwp-viewer]");
  const bar = page.locator("[data-hwp-edit-bar]");
  const dialog = page.getByRole("alertdialog");
  const find = bar.getByLabel("찾을 문자열");
  const replacement = bar.getByLabel("바꿀 문자열");
  const saveButton = bar.getByRole("button", { name: "편집본 저장" });
  const viewPath = `/w/acme/a/${hwpxId}/view`;
  const searchLink = page.getByRole("navigation", { name: "워크스페이스" }).getByRole("link", { name: "검색" });

  await page.goto(viewPath);
  await expectVueViewer(page);
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30_000 });
  await viewer.getByRole("button", { name: "간단 편집" }).click();
  await expect(bar).toBeVisible();
  for (const name of ["원본으로 되돌리기", "편집본 저장", "편집본 다운로드"]) {
    await expect(bar.getByRole("button", { name })).toBeDisabled();
  }

  // A replace-all with no match (count 0) edits nothing and leaves the page free to leave.
  await find.fill("없는문자열");
  await replacement.fill("X");
  await bar.getByRole("button", { name: "모두 바꾸기" }).click();
  await expect(bar.getByRole("alert")).toHaveText("찾는 문자열이 없습니다.");
  await expect(saveButton).toBeDisabled();
  await searchLink.click();
  await expect(page).toHaveURL(/\/w\/acme\/search/);
  await expect(dialog).toHaveCount(0);
  // Keep the no-match link check above, then make search a real document entry
  // for native history protection even when search is now a Vue SPA route.
  await expect(viewer).toHaveCount(0);
  await page.reload();
  await expect(page).toHaveURL(/\/w\/acme\/search/);
  await expect(page.locator("[data-v-app]")).toHaveCount(1);
  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`${viewPath}$`));
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30_000 });

  // Replace all on page 2: the page is drawn again with the new text.
  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await expect(viewer.getByText("2 / 3")).toBeVisible();
  const page2Before = await pageInk(page);
  await viewer.getByRole("button", { name: "간단 편집" }).click();
  await find.fill("하늘과");
  await replacement.fill("구름과");
  await bar.getByRole("button", { name: "모두 바꾸기" }).click();
  await expect(saveButton).toBeEnabled();
  await expect(bar.getByRole("alert")).toHaveCount(0);
  await expect(viewer.getByText("2 / 3")).toBeVisible();
  await expect.poll(() => pageInk(page)).not.toBe(page2Before);

  // A cancelled DOM click must remain cancelled; the dirty guard cannot
  // navigate before the anchor's own event handlers have run.
  const dirtyBeforeLinks = await pageInk(page);
  await page.evaluate(() => {
    const anchor = document.createElement("a");
    anchor.href = "/w/acme/search";
    anchor.addEventListener("click", (event) => event.preventDefault());
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
  });
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`${viewPath}$`));
  await expect(saveButton).toBeEnabled();
  expect(await pageInk(page)).toBe(dirtyBeforeLinks);

  // Opening a new tab leaves this document and its dirty content intact.
  await searchLink.evaluate((anchor) => anchor.setAttribute("target", "_blank"));
  const newTab = page.context().waitForEvent("page");
  await searchLink.click();
  const other = await newTab;
  await expect(other).toHaveURL(/\/w\/acme\/search/);
  await other.close();
  await searchLink.evaluate((anchor) => anchor.removeAttribute("target"));
  await expect(dialog).toHaveCount(0);
  await expect(saveButton).toBeEnabled();
  expect(await pageInk(page)).toBe(dirtyBeforeLinks);

  // Unsaved edits hold in-app navigation: a link click stays put on 취소 (or Escape).
  await searchLink.click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("heading")).toHaveText("저장하지 않은 본문이 있습니다");
  await expect(dialog.getByRole("button", { name: "취소" })).toBeFocused();
  await dialog.getByRole("button", { name: "취소" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`${viewPath}$`));
  await searchLink.click();
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`${viewPath}$`));
  await expect(saveButton).toBeEnabled();
  // Forward to the reloaded search document crosses a browser document boundary.
  // Its native beforeunload prompt must preserve the exact dirty content when
  // dismissed, and discard only after acceptance. Same-app history uses the
  // custom dialog below, after save-copy.
  const dirtyInk = await pageInk(page);
  const dismissed = new Promise<void>((resolve, reject) => page.once("dialog", (prompt) => {
    try { expect(prompt.type()).toBe("beforeunload"); } catch (error) { reject(error); return; }
    void prompt.dismiss().then(resolve, reject);
  }));
  await page.evaluate(() => window.history.forward());
  await dismissed;
  await expect(page).toHaveURL(new RegExp(`${viewPath}$`));
  await expect(saveButton).toBeEnabled();
  expect(await pageInk(page)).toBe(dirtyInk);
  const accepted = new Promise<void>((resolve, reject) => page.once("dialog", (prompt) => {
    try { expect(prompt.type()).toBe("beforeunload"); } catch (error) { reject(error); return; }
    void prompt.accept().then(resolve, reject);
  }));
  await page.evaluate(() => window.history.forward());
  await accepted;
  await expect(page).toHaveURL(/\/w\/acme\/search/);
  await expect(viewer).toHaveCount(0);
  await page.goBack();
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30_000 });
  await expect(bar).toHaveCount(0);

  // Revert: the edited page returns to the original.
  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await expect(viewer.getByText("2 / 3")).toBeVisible();
  await viewer.getByRole("button", { name: "간단 편집" }).click();
  await find.fill("하늘과");
  await replacement.fill("구름과");
  await bar.getByRole("button", { name: "모두 바꾸기" }).click();
  await expect(saveButton).toBeEnabled();
  await expect.poll(() => pageInk(page)).not.toBe(page2Before);
  await bar.getByRole("button", { name: "원본으로 되돌리기" }).click();
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "원본으로 되돌리기" }).click();
  await expect(saveButton).toBeDisabled();
  await expect.poll(() => pageInk(page)).toBe(page2Before);

  // Draft download needs no upload: the HWPX file named "(edited)" with the change in it.
  await find.fill("둘째");
  await replacement.fill("두번째");
  await bar.getByRole("button", { name: "하나 바꾸기" }).click();
  await expect(saveButton).toBeEnabled();
  const downloadEvent = page.waitForEvent("download");
  await bar.getByRole("button", { name: "편집본 다운로드" }).click();
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toBe("품의서 (edited).hwpx");
  const draft = new Uint8Array(fs.readFileSync((await download.path())!));
  expect([...draft.subarray(0, 4)]).toEqual([0x50, 0x4b, 0x03, 0x04]);
  expect(readPages(draft)).toEqual([pages[0], pages[1]!.replace("둘째", "두번째"), pages[2]]);
  expect(editCopyCalls).toEqual([]);

  // Access is checked again when the copy is written: the parent goes to the trash
  // while the upload completes, the save fails, and the edits stay.
  let release!: () => void;
  const held = new Promise<void>((resolve) => (release = resolve));
  let reached!: () => void;
  const completing = new Promise<void>((resolve) => (reached = resolve));
  await page.route("**/api/v1/workspaces/*/attachments/*/complete", async (route) => {
    reached();
    await held;
    await route.continue();
  });
  await saveButton.click();
  await completing;
  const trashed = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${documentId}/trash`);
  expect(trashed.ok(), await trashed.text()).toBeTruthy();
  release();
  await expect(bar.getByRole("alert")).toHaveText(
    "편집본을 저장하지 못했습니다. 다시 시도하거나 편집본을 다운로드하세요.",
    { timeout: 30_000 },
  );
  await page.unroute("**/api/v1/workspaces/*/attachments/*/complete");
  await expect(page).toHaveURL(new RegExp(`${viewPath}$`));
  await expect(saveButton).toBeEnabled();
  const restored = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${documentId}/restore`);
  expect(restored.ok(), await restored.text()).toBeTruthy();
  // The refused copy never became a stored attachment.
  expect(editCopies).toHaveLength(1);
  const refused = await page.request.get(`/api/v1/workspaces/${wsId}/attachments/${editCopies[0]}/download`);
  expect(refused.ok()).toBe(false);

  // Save copy: a new HWPX attachment beside the original, opened in its place, no guard prompt.
  await saveButton.click();
  // The original's URL has the same shape: wait until the save has left it.
  await expect(page).not.toHaveURL(new RegExp(`${viewPath}$`), { timeout: 30_000 });
  await expect(page).toHaveURL(/\/w\/acme\/a\/[0-9a-f-]{36}\/view$/);
  expect(page.url()).not.toContain(hwpxId);
  await expect(dialog).toHaveCount(0);
  const copyId = page.url().split("/a/")[1]!.split("/")[0]!;
  await expect(page.locator(".attachment-viewer__name")).toHaveText("품의서 (edited).hwpx");
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30_000 });
  await expect(viewer.getByRole("button", { name: "간단 편집" })).toBeVisible();
  const copy = new Uint8Array(
    await (await page.request.get(`/api/v1/workspaces/${wsId}/attachments/${copyId}/download`)).body(),
  );
  expect(readPages(copy)).toEqual([pages[0], pages[1]!.replace("둘째", "두번째"), pages[2]]);
  const original = new Uint8Array(
    await (await page.request.get(`/api/v1/workspaces/${wsId}/attachments/${hwpxId}/download`)).body(),
  );
  expect(sha256(original)).toBe(sha256(threePageHwpx));
  // History back from the copy with unsaved edits is held: Escape stays on the copy,
  // 나가기 returns to the untouched original.
  const copyPath = `/w/acme/a/${copyId}/view`;
  await viewer.getByRole("button", { name: "간단 편집" }).click();
  await find.fill("첫째");
  await replacement.fill("처음");
  await bar.getByRole("button", { name: "하나 바꾸기" }).click();
  await expect(saveButton).toBeEnabled();
  await page.evaluate(() => window.history.back());
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`${copyPath}$`));
  await expect(saveButton).toBeEnabled();
  await page.evaluate(() => window.history.back());
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "나가기" }).click();
  await expect(page).toHaveURL(new RegExp(`${viewPath}$`));
  await expect(page.locator(".attachment-viewer__name")).toHaveText("품의서.hwpx");
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30_000 });
  await expect(dialog).toHaveCount(0);

  // Binary HWP keeps its format: the copy is HWP 5.0 (OLE compound file).
  await page.goto(`/w/acme/a/${hwpId}/view`);
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30_000 });
  await viewer.getByRole("button", { name: "간단 편집" }).click();
  await find.fill("백두산이");
  await replacement.fill("한라산이");
  await bar.getByRole("button", { name: "모두 바꾸기" }).click();
  await expect(saveButton).toBeEnabled();
  await saveButton.click();
  await expect(page.locator(".attachment-viewer__name")).toHaveText("보고서 (edited).hwp", { timeout: 30_000 });
  const hwpCopyId = page.url().split("/a/")[1]!.split("/")[0]!;
  const hwpCopy = new Uint8Array(
    await (await page.request.get(`/api/v1/workspaces/${wsId}/attachments/${hwpCopyId}/download`)).body(),
  );
  expect([...hwpCopy.subarray(0, 4)]).toEqual([0xd0, 0xcf, 0x11, 0xe0]);
  expect(readPages(hwpCopy)).toEqual([pages[0], pages[1], pages[2]!.replaceAll("백두산이", "한라산이")]);

  // A share view lays the file out but never offers editing or calls the edit APIs.
  const shareRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${documentId}/share-links`, {
    data: { expiresInDays: 7 },
  });
  expect(shareRes.status(), await shareRes.text()).toBe(201);
  const sharePath = new URL(((await shareRes.json()) as { url: string }).url).pathname;
  const anon = await browser.newContext();
  const reader = await anon.newPage();
  const readerApi: string[] = [];
  reader.on("request", (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith("/api/")) readerApi.push(url.pathname);
  });
  await reader.goto(`${sharePath}/attachments/${hwpxId}/view`);
  await expect(reader.locator("[data-hwp-viewer]").getByText("1 / 3")).toBeVisible({ timeout: 30_000 });
  await expect(reader.getByRole("button", { name: "간단 편집" })).toHaveCount(0);
  for (const apiPath of readerApi) expect(apiPath).not.toMatch(/edit-context|edit-copy|preview-html/);
  await anon.close();

  // With unsaved edits, closing the tab asks first (beforeunload).
  await page.goto(viewPath);
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30_000 });
  await viewer.getByRole("button", { name: "간단 편집" }).click();
  await find.fill("첫째");
  await replacement.fill("처음");
  await bar.getByRole("button", { name: "하나 바꾸기" }).click();
  await expect(saveButton).toBeEnabled();
  expect(csp).toEqual([]);
  expect(pageErrors).toEqual([]);
  const unload = page.waitForEvent("dialog");
  await page.close({ runBeforeUnload: true });
  const prompt = await unload;
  expect(prompt.type()).toBe("beforeunload");
  await prompt.accept();
});
