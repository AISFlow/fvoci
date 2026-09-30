// Official Nuxt UI editor chrome on FVOCI's existing Tiptap/Yjs host.
// Real browser, Rust server, DB, peer and persisted ACK; no demo document/store.
import { expect, test } from "@playwright/test";
import { login, watchCspViolations } from "./helpers";
import {
  admin, member, blockAt, caretAtEndOf, createDoc, editorOf, newSignedInPage,
  expectBlocks, openDoc, save, savedBody, setupInstance, watchIconRequests, workspaceId,
} from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
test.beforeAll(async ({ browser, baseURL }) => { await setupInstance(browser, baseURL); });

test("fixed insert and history use the existing room, selection and persisted document", async ({ browser, baseURL, page }) => {
  const csp = watchCspViolations(page);
  const iconRequests = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "템플릿 도구", { markdown: "시작\n\n동료\n" });
  const peer = await newSignedInPage(browser, baseURL, member);
  try {
    await openDoc(page, doc.path);
    await openDoc(peer.page, doc.path);
    const toolbar = page.locator(".fvoci-template-toolbar--fixed");
    await expect(toolbar.locator('[role="group"]')).toHaveCount(2);
    await expect(toolbar.getByRole("button", { name: "실행 취소", exact: true })).toBeDisabled();
    await caretAtEndOf(page, 0);
    await page.keyboard.type(" 내편집");
    await expectBlocks(peer.page, ["시작 내편집", "동료"]);
    await caretAtEndOf(peer.page, 1);
    await peer.page.keyboard.type(" 원격편집");
    await expectBlocks(page, ["시작 내편집", "동료 원격편집"]);
    await toolbar.getByRole("button", { name: "실행 취소", exact: true }).click();
    await expectBlocks(page, ["시작", "동료 원격편집"]);
    await expectBlocks(peer.page, ["시작", "동료 원격편집"]);
    await toolbar.getByRole("button", { name: "다시 실행", exact: true }).click();
    await expectBlocks(peer.page, ["시작 내편집", "동료 원격편집"]);

    await caretAtEndOf(page, 1);
    await toolbar.getByRole("button", { name: "삽입", exact: true }).click();
    const insertMenu = page.getByRole("menu", { name: "삽입", exact: true });
    await expect(insertMenu.getByRole("menuitem").first()).toBeFocused();
    await insertMenu.getByRole("menuitem").first().click();
    const slash = page.locator(".fvoci-suggestion");
    await expect(slash).toBeVisible();
    await expect(slash.getByRole("group", { name: "블록 유형" }).first()).toBeVisible();
    await page.keyboard.type("math");
    await slash.getByRole("option", { name: "math", exact: true }).click();
    await expect(editorOf(peer.page).locator(".afn-math-edit")).toHaveCount(1);
    await save(page);
    const json = JSON.stringify(await savedBody(page.request, wsId, doc.id));
    expect(json).toContain('"type":"math"');
    expect(json).toContain("원격편집");
    await page.reload();
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
    await expect(editorOf(page).locator(".afn-math-edit")).toHaveCount(1);
    expect(csp).toEqual([]);
    expect(iconRequests).toEqual([]);
  } finally { await peer.context.close(); }
});

test("link popup keeps native selection and composing Enter cannot apply the URL", async ({ page }) => {
  const csp = watchCspViolations(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "템플릿 링크", { markdown: "한글과 😀 링크\n" });
  await openDoc(page, doc.path);
  await caretAtEndOf(page, 0);
  await page.keyboard.press("Shift+Home");
  const bubble = page.locator("[data-fvoci-bubble]");
  const trigger = bubble.getByRole("button", { name: "링크", exact: true });
  await expect(bubble).toBeVisible();
  await trigger.click();
  const dialog = page.getByRole("dialog", { name: "링크", exact: true });
  const url = dialog.getByLabel("URL");
  await expect(url).toBeFocused();
  await url.fill("https://example.com/한글");
  await url.dispatchEvent("compositionstart", { data: "한" });
  await url.dispatchEvent("keydown", { key: "Enter", isComposing: true, keyCode: 229 });
  await expect(dialog).toBeVisible();
  await expect(blockAt(page, 0).locator("a")).toHaveCount(0);
  await url.dispatchEvent("compositionend", { data: "한글" });
  await page.keyboard.press("Escape");
  await expect(trigger).toBeFocused();
  expect(await editorOf(page).evaluate(() => window.getSelection()?.toString())).toBe("한글과 😀 링크");
  await trigger.click();
  await url.fill("https://example.com/한글");
  await url.press("Enter");
  await expect(dialog).toHaveCount(0);
  await expect(blockAt(page, 0).locator("a")).toHaveText("한글과 😀 링크");
  await save(page);
  expect(JSON.stringify(await savedBody(page.request, wsId, doc.id))).toContain('"type":"link"');
  expect(csp).toEqual([]);
});

test("emoji insertion and mobile groups remain keyboard usable without viewport overflow", async ({ page }) => {
  const iconRequests = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "템플릿 이모지", { markdown: "문단\n" });
  await openDoc(page, doc.path);
  await caretAtEndOf(page, 0);
  await page.locator(".fvoci-template-toolbar--fixed").getByRole("button", { name: "삽입", exact: true }).click();
  await page.getByRole("menu", { name: "삽입", exact: true }).getByRole("menuitem", { name: ":", exact: true }).click();
  await page.keyboard.type("smile");
  const suggestions = page.locator(".fvoci-suggestion");
  await expect(suggestions).toBeVisible();
  await expect(suggestions.getByRole("option").first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowDown");
  await expect(suggestions.getByRole("option").nth(1)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(suggestions).toHaveCount(0);
  await save(page);
  expect(JSON.stringify(await savedBody(page.request, wsId, doc.id))).toContain('"type":"emoji"');
  await page.setViewportSize({ width: 390, height: 844 });
  const mobile = page.locator("[data-mobile-toolbar]");
  await expect(mobile).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  const bold = mobile.getByRole("button", { name: "굵게", exact: true });
  const box = await bold.boundingBox();
  expect(box?.height).toBeGreaterThanOrEqual(44);
  await caretAtEndOf(page, 0);
  await page.keyboard.press("Shift+Home");
  await bold.click();
  await expect(blockAt(page, 0).locator("strong")).toContainText("문단");
  await expect(page.locator("[data-fvoci-bubble]")).toBeHidden();
  expect(iconRequests).toEqual([]);
});
