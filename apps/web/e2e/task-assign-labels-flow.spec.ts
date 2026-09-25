import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tal",
  workspaceName: "Task Assign Labels",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

async function taskIdFor(page: Page, wsId: string, displayId: string): Promise<string> {
  const lookup = await page.request.get(`/api/v1/workspaces/${wsId}/lookup/${displayId}`);
  expect(lookup.ok()).toBe(true);
  const taskId = (await lookup.json()).items.find((item: { kind: string }) => item.kind === "task")?.id;
  expect(taskId).toBeTruthy();
  return taskId;
}

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

test("task assignee and label pickers round-trip through the edit UI", async ({ page }) => {
  await ensureSetup(page);
  await page.goto(`/w/${admin.workspaceSlug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("tal");
  await page.getByLabel("이름", { exact: true }).fill("Assign Lab");
  await page.getByLabel("공개 범위").selectOption("workspace");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/TAL/tasks$`));

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("담당 라벨");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/TAL-2$`));
  await expect(page.getByRole("heading", { name: "담당 라벨" })).toBeVisible();

  const wsId = await workspaceId(page, admin.workspaceSlug);
  const meRes = await page.request.get("/api/v1/auth/me");
  expect(meRes.ok()).toBe(true);
  const userId = (await meRes.json()).userId as string;
  const projectsRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  const project = (await projectsRes.json()).items.find((item: { key: string }) => item.key === "TAL");
  expect(project).toBeTruthy();
  const taskId = await taskIdFor(page, wsId, "TAL-2");

  const createLabel = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/labels`,
    { data: { name: "긴급", color: "red" } },
  );
  expect(createLabel.status()).toBe(201);
  const labelId = (await createLabel.json()).id as string;

  await page.reload();
  await expect(page.getByTestId("task-edit-assignees")).toBeVisible();
  await expect(page.getByTestId(`task-edit-assignee-${userId}`)).toBeVisible();
  await page.getByTestId(`task-edit-assignee-${userId}`).check();
  await page.getByTestId(`task-edit-label-${labelId}`).check();

  await expect.poll(async () => {
    const detailRes = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
    expect(detailRes.ok()).toBe(true);
    const detail = await detailRes.json();
    return `${(detail.assigneeIds ?? []).join(",")}:${(detail.labelIds ?? []).join(",")}`;
  }).toBe(`${userId}:${labelId}`);

  await page.getByTestId(`task-edit-assignee-${userId}`).uncheck();
  await expect.poll(async () => {
    const detailRes = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
    expect(detailRes.ok()).toBe(true);
    const detail = await detailRes.json();
    return (detail.assigneeIds ?? []).length;
  }).toBe(0);
});
