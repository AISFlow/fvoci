import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "ptrash",
  workspaceName: "Project Trash E2E",
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
  if ((await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0) {
    await login(page, admin.email, admin.password);
  }
}

async function workspaceId(page: Page): Promise<string> {
  const res = await page.request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const workspace = (await res.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  expect(workspace).toBeTruthy();
  return workspace.id;
}

test("project document: edit, trash, restore; project delete and admin restore", async ({
  page,
}) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: "TRSH", name: "휴지통 프로젝트", visibility: "workspace" },
  });
  expect(projectRes.status()).toBe(201);
  const project = await projectRes.json();
  const listRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  const listed = (await listRes.json()).items.find(
    (item: { id: string }) => item.id === project.id,
  );
  expect(listed).toMatchObject({ canEdit: true, canManage: true });
  const docRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/documents`,
    { data: { parentId: project.rootDocumentId, title: "프로젝트 기획서" } },
  );
  expect(docRes.status()).toBe(201);
  const doc = await docRes.json();

  // The project document opens in the collaborative editor.
  await page.goto(`/w/${admin.workspaceSlug}/${doc.displayId}`);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  // Attachment uploads are not supported here yet: a notice outside the editor.
  await editor.click();
  await page.keyboard.type("기획 본문");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });

  // Trash from the document page, then restore from the workspace trash.
  await page.evaluate(() => {
    window.confirm = () => true;
  });
  const trashed = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().includes(`/projects/${project.id}/documents/${doc.id}/trash`) &&
      response.ok(),
  );
  await page.getByRole("button", { name: "휴지통으로 이동" }).click();
  await trashed;
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/trash$`));
  await expect(page.getByText("30일이 지나면 영구 삭제")).toBeVisible();
  await expect(page.getByText("프로젝트 기획서")).toBeVisible();
  await page.getByRole("button", { name: "복원 프로젝트 기획서" }).click();
  await expect(page.getByText("휴지통이 비었습니다")).toBeVisible();

  await page.goto(`/w/${admin.workspaceSlug}/TRSH`);
  await expect(page.getByTestId(`project-doc-${doc.displayId}`)).toBeVisible();
  const body = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/documents/${doc.id}/body`,
  );
  expect(JSON.stringify((await body.json()).contentJson)).toContain("기획 본문");

  // Archive makes the project read-only; unarchive restores writes.
  await page.evaluate(() => {
    window.confirm = () => true;
  });
  await page.getByTestId("project-lifecycle").getByRole("button", { name: "보관" }).click();
  await expect(page.getByText("보관됨").first()).toBeVisible();
  await page
    .getByTestId("project-lifecycle")
    .getByRole("button", { name: "보관 해제" })
    .click();
  await expect(
    page.getByTestId("project-lifecycle").getByRole("button", { name: "보관", exact: true }),
  ).toBeVisible();

  // Delete the project, then an admin restores it with its documents.
  await page.getByTestId("project-lifecycle").getByRole("button", { name: "프로젝트 삭제" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/projects$`));
  await expect(page.getByRole("link", { name: /휴지통 프로젝트/ })).toHaveCount(0);

  await page.goto(`/w/${admin.workspaceSlug}/settings`);
  await page.evaluate(() => {
    window.confirm = () => true;
  });
  const deleted = page.getByTestId("deleted-projects");
  await expect(deleted).toContainText("TRSH");
  await deleted.getByRole("button", { name: "복원 휴지통 프로젝트" }).click();
  await expect(page.getByText("삭제된 프로젝트가 없습니다")).toBeVisible();

  await page.goto(`/w/${admin.workspaceSlug}/TRSH`);
  await expect(page.getByTestId(`project-doc-${doc.displayId}`)).toBeVisible();
});
