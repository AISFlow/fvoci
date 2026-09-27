import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tori",
  workspaceName: "Task Origin Flow",
};

async function ensureSetup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(admin.familyName);
    await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
    await page.getByLabel("이메일").fill(admin.email);
    await page.getByLabel("비밀번호").fill(admin.password);
    await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
    await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
    return;
  }
  if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
}

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === slug,
  );
  expect(workspace).toBeTruthy();
  return workspace.id;
}

async function documentIdFromOpenWikiPage(page: Page, wsId: string): Promise<string> {
  const match = /\/WIKI-(\d+)$/.exec(new URL(page.url()).pathname);
  expect(match).toBeTruthy();
  const treeRes = await page.request.get(`/api/v1/workspaces/${wsId}/tree`);
  expect(treeRes.ok()).toBe(true);
  const document = (await treeRes.json()).items.find(
    (item: { number: number }) => item.number === Number(match![1]),
  );
  expect(document).toBeTruthy();
  return document.id;
}

test("document create form makes a task origin visible on both screens", async ({ page }) => {
  await ensureSetup(page);
  await page.goto(`/w/${admin.workspaceSlug}/wiki`);
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/WIKI-\\d+$`));

  const wsId = await workspaceId(page, admin.workspaceSlug);
  const documentId = await documentIdFromOpenWikiPage(page, wsId);
  const documentUrl = page.url();

  const origins = page.getByRole("region", { name: "연결 태스크" });
  await expect(origins.getByRole("heading", { name: /연결 태스크/ })).toBeVisible();
  await expect(origins.getByText("태스크를 관리할 프로젝트를 먼저 만드세요.")).toBeVisible();

  await origins.getByLabel("프로젝트 이름").fill("출처 프로젝트");
  await origins.getByLabel("키").fill("tori");
  await origins.getByRole("button", { name: "새 프로젝트" }).click();
  await expect(origins.getByLabel("태스크 프로젝트")).toBeVisible();
  await origins.getByLabel("태스크 제목").fill("문서에서 만든 연결");
  await origins.getByRole("button", { name: "연결 태스크 만들기" }).click();

  const created = origins.getByRole("link", { name: /TORI-\d+ · 문서에서 만든 연결/ });
  await expect(created).toBeVisible();
  await expect(origins.getByRole("heading", { name: /연결 태스크 \(1\)/ })).toBeVisible();

  await created.click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/TORI-\\d+$`));
  const source = page.getByRole("region", { name: "출처 문서" });
  await expect(source.getByRole("heading", { name: /출처 문서 \(1\)/ })).toBeVisible();
  await expect(source.getByRole("link", { name: /WIKI-\d+/ })).toBeVisible();

  const listRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/task-origins`,
  );
  expect(listRes.ok()).toBe(true);
  const listed = (await listRes.json()) as { items: { taskId: string }[]; count: number };
  expect(listed.count).toBe(1);
  const taskId = listed.items[0].taskId;
  const trashRes = await page.request.post(`/api/v1/workspaces/${wsId}/tasks/${taskId}/trash`);
  expect(trashRes.ok()).toBe(true);

  await page.goto(documentUrl);
  const afterTrash = page.getByRole("region", { name: "연결 태스크" });
  await expect(afterTrash.getByRole("heading", { name: /연결 태스크 \(0\)/ })).toBeVisible();
  await expect(afterTrash.getByRole("link", { name: /TORI-/ })).toHaveCount(0);

  const restoreRes = await page.request.post(`/api/v1/workspaces/${wsId}/tasks/${taskId}/restore`);
  expect(restoreRes.ok()).toBe(true);
  await page.reload();
  const afterRestore = page.getByRole("region", { name: "연결 태스크" });
  await expect(afterRestore.getByRole("heading", { name: /연결 태스크 \(1\)/ })).toBeVisible();
  await expect(afterRestore.getByRole("link", { name: /TORI-\d+ · 문서에서 만든 연결/ })).toBeVisible();
});
