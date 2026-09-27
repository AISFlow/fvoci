import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "prevs",
  workspaceName: "Project Revisions E2E",
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

test("project document: save a revision, edit, restore through the room", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: "PREV", name: "리비전 프로젝트", visibility: "private" },
  });
  expect(projectRes.status()).toBe(201);
  const project = await projectRes.json();
  const docRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/documents`,
    { data: { parentId: project.rootDocumentId, title: "리비전 문서" } },
  );
  expect(docRes.status()).toBe(201);
  const doc = await docRes.json();
  const bodyUrl = `/api/v1/workspaces/${wsId}/projects/${project.id}/documents/${doc.id}/body`;
  const revisionsUrl = `/api/v1/workspaces/${wsId}/projects/${project.id}/documents/${doc.id}/revisions`;

  // 1. Edit the project document in the collaborative editor.
  await page.goto(`/w/${admin.workspaceSlug}/${doc.displayId}`);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  await editor.click();
  await page.keyboard.type("첫 번째 버전 😀");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });

  // 2. Save a revision from the version panel (project revision route).
  const created = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().endsWith(`/projects/${project.id}/documents/${doc.id}/revisions`),
  );
  await page.getByTestId("revision-history").click();
  await page.getByTestId("revision-save").click();
  expect((await created).status()).toBe(201);
  await expect(page.getByTestId("revision-item")).toHaveCount(1);

  // 3. Keep editing past the saved revision (panel closed: its save button is also "저장").
  await page.getByTestId("revision-history").click();
  await expect(page.getByTestId("revision-save")).toHaveCount(0);
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.type("두 번째 버전");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect
    .poll(async () => JSON.stringify((await (await page.request.get(bodyUrl)).json()).contentJson))
    .toContain("두 번째 버전");
  await expect(editor).not.toContainText("첫 번째 버전");

  // 4. Restore the saved revision: the open editor and the durable body follow.
  await page.getByTestId("revision-history").click();
  await expect(page.getByTestId("revision-item")).toHaveCount(1);
  await page.getByTestId("revision-restore").first().click();
  await page.getByTestId("revision-restore-confirm").click();
  await expect(editor).toContainText("첫 번째 버전 😀", { timeout: 15_000 });
  await expect
    .poll(async () => JSON.stringify((await (await page.request.get(bodyUrl)).json()).contentJson))
    .toContain("첫 번째 버전");

  // 5. Reload: the restored body comes back from the persisted room.
  await page.reload();
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(editor).toContainText("첫 번째 버전 😀");

  // 6. The wiki revision path does not expose the project document.
  const wiki = await page.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}/revisions`);
  expect(wiki.status()).toBe(404);

  // 7. Archived project: history stays readable, writes are refused and hidden.
  const archive = await page.request.post(`/api/v1/workspaces/${wsId}/projects/${project.id}/archive`);
  expect(archive.status()).toBe(200);
  const refused = await page.request.post(revisionsUrl);
  expect(refused.status()).toBe(409);
  expect((await refused.json()).code).toBe("project_archived");
  await page.reload();
  await page.getByTestId("revision-history").click();
  await expect(page.getByTestId("revision-item").first()).toBeVisible();
  await expect(page.getByTestId("revision-save")).toHaveCount(0);
  await expect(page.getByTestId("revision-restore")).toHaveCount(0);
});
