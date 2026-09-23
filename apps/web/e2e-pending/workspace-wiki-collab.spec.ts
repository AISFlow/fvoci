/**
 * PENDING product `/collab`.
 *
 * These Playwright cases are the wiki collab acceptance set. They must not be
 * moved into `e2e/` or executed until a later Composer task mounts the product
 * socket (session cookie, Origin, ACL, engine, DB). Current CI `web.yml` runs
 * every file under `e2e/` through `scripts/run-web-e2e.sh`.
 *
 * Full server restart / fresh-client crash acceptance is coordinated later and
 * is not marked complete here.
 */
import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login } from "../e2e/helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
};

const member = {
  email: "collab-member@example.com",
  password: "memberpass1",
  givenName: "협업",
  familyName: "멤버",
};

const peer = {
  email: "collab-peer@example.com",
  password: "peerpass1",
  givenName: "동료",
  familyName: "편집",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

async function createWikiDoc(page: Page, title: string): Promise<{ id: string; displayId: string; url: string }> {
  const id = await workspaceId(page, "acme");
  const res = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: null, title },
  });
  expect(res.ok()).toBe(true);
  const body = await res.json();
  return { id: body.id, displayId: body.displayId, url: `/w/acme/${body.displayId}` };
}

async function waitConnected(page: Page): Promise<void> {
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({
    timeout: 15_000,
  });
}

async function waitDurableSaved(page: Page): Promise<void> {
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText("연결됨 · 저장됨", { exact: true })).toBeVisible();
}

async function persistBody(page: Page): Promise<void> {
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await waitDurableSaved(page);
}

async function openEditor(page: Page, url: string): Promise<ReturnType<Page["locator"]>> {
  await page.goto(url);
  await waitConnected(page);
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  return editor;
}

test("member wiki doc types Korean/Han/emoji, keeps data-id, and reloads the same text", async ({ page }) => {
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: "acme",
    membershipRole: "member",
  });
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "협업 본문");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("본문 한글과 漢字🙂");
  await persistBody(page);
  await expect(editor.locator("[data-id]").first()).toBeVisible();
  await page.reload();
  await waitConnected(page);
  await expect(page.locator(".fvoci-editor .ProseMirror")).toContainText("본문 한글과 漢字🙂");
});

test("slash table and link popover keep structured nodes", async ({ page }) => {
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "구조 편집");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("/표");
  await page.keyboard.press("Enter");
  await expect(page.locator(".fvoci-editor table")).toBeVisible();

  await editor.click();
  await page.keyboard.type("링크대상");
  await page.keyboard.press("Shift+Home");
  await page.getByRole("button", { name: "링크" }).click();
  await page.getByRole("textbox").fill("https://example.com");
  await page.getByRole("button", { name: "적용" }).click();
  await expect(page.locator('.fvoci-editor a[href="https://example.com"]')).toBeVisible();
});

test("two contexts converge, show awareness, and drop the peer on close", async ({ browser }) => {
  const ctxA = await browser.newContext();
  const ctxB = await browser.newContext();
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  try {
    await login(pageA, member.email, member.password);
    const doc = await createWikiDoc(pageA, "두 탭 수렴");
    const editorA = await openEditor(pageA, doc.url);

    await login(pageB, member.email, member.password);
    const editorB = await openEditor(pageB, doc.url);

    await expect(pageA.getByLabel(/동시 접속 \d+명/)).toBeVisible({ timeout: 15_000 });
    await expect(pageB.getByLabel(/동시 접속 \d+명/)).toBeVisible();
    await expect(pageA.locator('[data-collab-status="connected"]')).toBeVisible();
    await expect(pageB.locator('[data-collab-status="connected"]')).toBeVisible();
    await expect(pageA.locator('[data-collab-persisted="true"]')).toHaveCount(0);
    await expect(pageB.locator('[data-collab-persisted="true"]')).toHaveCount(0);

    await editorA.click();
    await pageA.keyboard.type("A가 쓴 줄");
    await editorB.click();
    await pageB.keyboard.press("Enter");
    await pageB.keyboard.type("B가 쓴 줄");

    await expect(editorA).toContainText("A가 쓴 줄", { timeout: 15_000 });
    await expect(editorA).toContainText("B가 쓴 줄", { timeout: 15_000 });
    await expect(editorB).toContainText("A가 쓴 줄", { timeout: 15_000 });
    await expect(editorB).toContainText("B가 쓴 줄", { timeout: 15_000 });

    await ctxB.close();
    await expect(pageA.getByLabel(/동시 접속 \d+명/)).toBeHidden({ timeout: 20_000 });
  } finally {
    await ctxA.close();
    await ctxB.close().catch(() => {});
  }
});

test("insertion and deletion collide and both survive", async ({ browser }) => {
  const ctxA = await browser.newContext();
  const ctxB = await browser.newContext();
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  try {
    await login(pageA, member.email, member.password);
    const doc = await createWikiDoc(pageA, "삽입 삭제 충돌");
    const editorA = await openEditor(pageA, doc.url);
    await editorA.click();
    await pageA.keyboard.type("공통 문장");
    await expect(editorA).toContainText("공통 문장");

    await login(pageB, member.email, member.password);
    const editorB = await openEditor(pageB, doc.url);
    await expect(editorB).toContainText("공통 문장", { timeout: 15_000 });
    await editorA.click();
    await pageA.keyboard.press("Home");
    await pageA.keyboard.type("앞쪽삽입 ");
    await editorB.click();
    await pageB.keyboard.press("End");
    await pageB.keyboard.press("Backspace");
    await pageB.keyboard.press("Backspace");
    await expect(editorA).toContainText("앞쪽삽입", { timeout: 15_000 });
    await expect(editorB).toContainText("앞쪽삽입", { timeout: 15_000 });
    await expect(editorA).not.toContainText("공통 문장");
    await expect(editorB).not.toContainText("공통 문장");
  } finally {
    await ctxA.close();
    await ctxB.close().catch(() => {});
  }
});

test("offline typing reconnects without dropping unsent text", async ({ page, context }) => {
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "재접속");
  const editor = await openEditor(page, doc.url);
  await context.setOffline(true);
  await editor.click();
  await page.keyboard.type("오프라인에서 쓴 줄");
  await expect(page.getByText("연결됨 · 저장 대기")).toBeVisible();
  await context.setOffline(false);
  await waitConnected(page);
  await expect(page.locator('[data-collab-persisted="true"]')).toHaveCount(0);
  await expect(editor).toContainText("오프라인에서 쓴 줄");
});

test("archived document stays connected and read-only", async ({ page }) => {
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "보관 문서");
  await openEditor(page, doc.url);
  await page.getByLabel("문서 상태").selectOption("archived");
  await expect(page.getByLabel("문서 상태")).toHaveValue("archived");
  await expect(page.getByText("읽기 전용")).toBeVisible();
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
  await expect(page.getByText("연결됨 · 저장됨", { exact: true })).toHaveCount(0);
  await expect(page.locator('[data-collab-persisted="true"]')).toHaveCount(0);
  await expect(page.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
});

test("membership revoke while connected stops further edits", async ({ browser }) => {
  createE2eUser(peer.email, peer.password, peer.givenName, {
    familyName: peer.familyName,
    workspaceSlug: "acme",
    membershipRole: "member",
  });
  const ownerCtx = await browser.newContext();
  const memberCtx = await browser.newContext();
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

    const ws = await workspaceId(ownerPage, "acme");
    const revoke = await ownerPage.request.delete(
      `/api/v1/workspaces/${ws}/members/${memberId}`,
    );
    expect(revoke.ok()).toBe(true);

    await expect(memberPage.getByText("권한 없음 · 다시 로그인")).toBeVisible({
      timeout: 20_000,
    });
    await memberPage.keyboard.type("철회 후 문장");
    await expect(editor).not.toContainText("철회 후 문장");
  } finally {
    await ownerCtx.close();
    await memberCtx.close();
  }
});

test("deletion-only then structured subsequent edits survive persist", async ({ page }) => {
  await login(page, member.email, member.password);
  const doc = await createWikiDoc(page, "삭제만");
  const editor = await openEditor(page, doc.url);
  await editor.click();
  await page.keyboard.type("지울 문장");
  await persistBody(page);
  await page.keyboard.press("Control+A");
  await page.keyboard.press("Backspace");
  await expect(page.locator('[data-collab-persisted="true"]')).toHaveCount(0);
  await persistBody(page);
  await page.keyboard.type("/표");
  await page.keyboard.press("Enter");
  await expect(page.locator(".fvoci-editor table")).toBeVisible();
  await persistBody(page);
  await page.reload();
  await waitConnected(page);
  await expect(page.locator(".fvoci-editor table")).toBeVisible();
  await expect(page.locator(".fvoci-editor .ProseMirror")).not.toContainText("지울 문장");
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
