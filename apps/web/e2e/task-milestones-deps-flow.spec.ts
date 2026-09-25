import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tmd",
  workspaceName: "Task Milestones Deps",
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

test("milestone picker and task dependency round-trip through the edit UI", async ({ page }) => {
  await ensureSetup(page);
  await page.goto(`/w/${admin.workspaceSlug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("tmd");
  await page.getByLabel("이름", { exact: true }).fill("마일스톤 의존");
  await page.getByLabel("공개 범위").selectOption("workspace");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/TMD/tasks$`));

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("선행");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page.getByRole("heading", { name: "선행" })).toBeVisible();
  const blockerPath = new URL(page.url()).pathname;
  const blockerDisplay = blockerPath.split("/").pop() ?? "";

  await page.goto(`/w/${admin.workspaceSlug}/TMD/tasks`);
  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("후행");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page.getByRole("heading", { name: "후행" })).toBeVisible();
  const blockedPath = new URL(page.url()).pathname;
  const blockedDisplay = blockedPath.split("/").pop() ?? "";

  await page.goto(`/w/${admin.workspaceSlug}/TMD/tasks`);
  await expect(page.getByTestId("project-milestones")).toBeVisible();
  await page.getByTestId("project-milestone-name").fill("출시");
  await page.getByTestId("project-milestone-add").click();
  await expect(page.getByText("출시", { exact: true })).toBeVisible();

  const wsId = await workspaceId(page, admin.workspaceSlug);
  const projectsRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  const project = (await projectsRes.json()).items.find((item: { key: string }) => item.key === "TMD");
  expect(project).toBeTruthy();
  const milestonesRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/milestones`,
  );
  expect(milestonesRes.ok()).toBe(true);
  const milestoneId = (await milestonesRes.json()).items.find(
    (item: { name: string }) => item.name === "출시",
  )?.id as string;
  expect(milestoneId).toBeTruthy();

  const blockerId = await taskIdFor(page, wsId, blockerDisplay);
  const blockedId = await taskIdFor(page, wsId, blockedDisplay);

  await page.goto(blockerPath);
  await expect(page.getByTestId("task-edit-milestone")).toBeVisible();
  await page.getByTestId("task-edit-milestone").selectOption(milestoneId);
  await expect.poll(async () => {
    const detailRes = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${blockerId}`);
    expect(detailRes.ok()).toBe(true);
    return (await detailRes.json()).milestoneId;
  }).toBe(milestoneId);

  await page.getByTestId("task-edit-dependency-open").click();
  await page.getByTestId("task-edit-dependency-target").selectOption(blockedId);
  await page.getByTestId("task-edit-dependency-add").click();
  await expect.poll(async () => {
    const detailRes = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${blockerId}`);
    expect(detailRes.ok()).toBe(true);
    const detail = await detailRes.json();
    const deps = detail.dependencies ?? [];
    return deps.some(
      (edge: { blockerId: string; blockedId: string; type: string }) =>
        edge.blockerId === blockerId && edge.blockedId === blockedId && edge.type === "FS",
    );
  }).toBe(true);
});
