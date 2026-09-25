import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login, logout } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tcu",
  workspaceName: "Task Comments UI",
};

const member = {
  email: "tcu-member@example.com",
  password: "memberpass1",
  givenName: "댓글",
  familyName: "멤버",
};

const viewer = {
  email: "tcu-viewer@example.com",
  password: "viewerpass1",
  givenName: "보기",
  familyName: "전용",
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

test("member comments on a task, viewer is read-only, parent picker searches", async ({ page }) => {
  test.setTimeout(90_000);

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

  await page.goto(`/w/${owner.workspaceSlug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("tcu");
  await page.getByLabel("이름", { exact: true }).fill("Comments Lab");
  await page.getByLabel("공개 범위").selectOption("private");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/TCU/tasks$`));

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("검색용 부모");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/TCU-2$`));

  await page.goto(`/w/${owner.workspaceSlug}/TCU/tasks`);
  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("댓글 대상");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/TCU-3$`));
  const childPath = page.url();

  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });
  createE2eUser(viewer.email, viewer.password, viewer.givenName, {
    familyName: viewer.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });

  const wsId = await workspaceId(page, owner.workspaceSlug);
  const projectsRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  expect(projectsRes.ok()).toBe(true);
  const project = (await projectsRes.json()).items.find((item: { key: string }) => item.key === "TCU");
  expect(project).toBeTruthy();
  const membersRes = await page.request.get(`/api/v1/workspaces/${wsId}/members`);
  expect(membersRes.ok()).toBe(true);
  const members = (await membersRes.json()).items as Array<{ email: string; userId: string }>;
  const memberId = members.find((item) => item.email.toLowerCase() === member.email.toLowerCase())?.userId;
  const viewerId = members.find((item) => item.email.toLowerCase() === viewer.email.toLowerCase())?.userId;
  expect(memberId).toBeTruthy();
  expect(viewerId).toBeTruthy();
  const memberGrant = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/members`,
    { data: { userId: memberId, role: "member" } },
  );
  expect(memberGrant.status()).toBe(201);
  const viewerGrant = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/members`,
    { data: { userId: viewerId, role: "viewer" } },
  );
  expect(viewerGrant.status()).toBe(201);

  await logout(page);
  await login(page, member.email, member.password);
  await page.goto(childPath);
  await expect(page.getByRole("heading", { name: "댓글 대상" })).toBeVisible();

  const panel = page.getByTestId("task-comments");
  await expect(panel.getByRole("heading", { name: "댓글" })).toBeVisible();

  const compose = panel.locator("[data-comment-compose] textarea");
  await compose.fill("태스크 댓글입니다");
  await panel.getByRole("button", { name: "등록" }).click();
  await expect(panel.getByText("태스크 댓글입니다")).toBeVisible();

  await panel.getByRole("button", { name: "해결" }).click();
  await expect(panel.getByRole("button", { name: "다시 열기" })).toBeVisible();

  await panel.getByRole("button", { name: "반응 👍" }).click();
  await expect(panel.getByRole("button", { name: "반응 👍", pressed: true })).toContainText("1");

  await panel.getByRole("button", { name: "답글" }).click();
  const reply = panel.locator("[data-comment-reply] textarea");
  await reply.fill("답글입니다");
  await expect(compose).toHaveValue("");
  await panel.locator("[data-comment-reply]").getByRole("button", { name: "등록" }).click();
  await expect(panel.getByText("답글입니다")).toBeVisible();

  await page.getByTestId("task-edit-type").selectOption("subtask");
  await page.getByTestId("task-edit-parent").click();
  await page.getByTestId("task-edit-parent-search").fill("검색용 부모");
  const parentPanel = page.locator(".task-parent-select__panel");
  await expect(parentPanel.getByRole("option", { name: /검색용 부모/ })).toBeVisible();
  await expect(parentPanel.getByRole("option", { name: /댓글 대상/ })).toHaveCount(0);
  await parentPanel.getByRole("option", { name: /검색용 부모/ }).click();
  await page.getByTestId("task-edit-hierarchy-save").click();

  const parentTaskId = await taskIdFor(page, wsId, "TCU-2");
  const childTaskId = await taskIdFor(page, wsId, "TCU-3");
  await expect.poll(async () => {
    const detailRes = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${childTaskId}`);
    expect(detailRes.ok()).toBe(true);
    const detail = await detailRes.json();
    return `${detail.type}:${detail.parentId}`;
  }).toBe(`subtask:${parentTaskId}`);

  await logout(page);
  await login(page, viewer.email, viewer.password);
  await page.goto(childPath);
  await expect(page.getByRole("heading", { name: "댓글 대상" })).toBeVisible();
  const viewerPanel = page.getByTestId("task-comments");
  await expect(viewerPanel.getByText("태스크 댓글입니다")).toBeVisible();
  await expect(viewerPanel.locator("[data-comment-compose]")).toHaveCount(0);
  await expect(viewerPanel.getByRole("button", { name: "답글" })).toHaveCount(0);
  await expect(viewerPanel.getByRole("button", { name: "반응 👍" }).first()).toBeDisabled();
  await expect(page.getByTestId("task-edit-parent")).toBeDisabled();
});
