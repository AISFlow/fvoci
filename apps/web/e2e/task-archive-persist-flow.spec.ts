import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tap",
  workspaceName: "Task Archive Persist",
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

async function taskJson(page: Page, wsId: string, taskId: string) {
  const res = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
  expect(res.ok()).toBe(true);
  return res.json() as Promise<{ contentJson: unknown; archivedAt: string | null }>;
}

test("archive persists collaborative body then restores after unarchive", async ({ page }) => {
  await ensureSetup(page);
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  const wsId = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  ).id as string;

  const projectKey = `ZT${(Date.now() % 900) + 100}`;
  const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: projectKey, name: "Archive Persist", visibility: "workspace" },
  });
  expect(projectRes.status(), await projectRes.text()).toBe(201);
  const project = (await projectRes.json()) as { id: string };
  const taskRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/tasks`,
    { data: { title: "보관 전 본문" } },
  );
  expect(taskRes.status()).toBe(201);
  const task = (await taskRes.json()) as { id: string; number: number };

  await page.goto(`/w/${admin.workspaceSlug}/${projectKey}-${task.number}`);
  await expect(page.getByRole("heading", { name: "보관 전 본문" })).toBeVisible({ timeout: 15_000 });
  const body = page.getByTestId("task-body");
  await expect(body).toBeVisible({ timeout: 15_000 });
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 30_000 });
  const editor = body.locator(".fvoci-editor .ProseMirror");
  await editor.click();
  const bodyText = "한글 본문 🎯 보관 테스트";
  await page.keyboard.type(bodyText);

  await page.getByRole("button", { name: "보관", exact: true }).click();
  await expect(page.getByText("보관된 태스크입니다")).toBeVisible({ timeout: 15_000 });
  await expect(editor).toHaveAttribute("contenteditable", "false");

  await expect
    .poll(async () => {
      const res = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${task.id}`);
      const json = await res.json();
      return JSON.stringify(json.contentJson);
    })
    .toContain("한글 본문");
  await expect
    .poll(async () => JSON.stringify((await taskJson(page, wsId, task.id)).contentJson))
    .toContain("bullseye");

  await page.reload();
  await expect(body).toBeVisible({ timeout: 15_000 });
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 30_000 });
  await expect(editor).toContainText(bodyText);

  await page.getByRole("button", { name: "복원", exact: true }).click();
  await expect.poll(async () => (await taskJson(page, wsId, task.id)).archivedAt).toBeNull();
  await page.reload();
  await expect(body).toBeVisible({ timeout: 15_000 });
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 30_000 });
  await expect(editor).toHaveAttribute("contenteditable", "true");
  await expect(editor).toContainText(bodyText);
});
