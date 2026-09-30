import assert from "node:assert/strict";
import { z } from "zod";
import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login } from "./helpers";

// Validate the response fields used by this flow; retain the complete payload.
const workspaceListSchema = z
  .object({ items: z.array(z.object({ id: z.string(), slug: z.string() }).passthrough()) })
  .passthrough();
const workspaceSlugListSchema = z
  .object({ items: z.array(z.object({ slug: z.string() }).passthrough()) })
  .passthrough();
const projectListSchema = z
  .object({
    items: z.array(
      z
        .object({
          id: z.string(),
          key: z.string(),
          rootDocumentId: z.string().nullable(),
          taskCount: z.number(),
          openTaskCount: z.number(),
        })
        .passthrough(),
    ),
  })
  .passthrough();
const taskSchema = z.object({ id: z.string(), statusId: z.string() }).passthrough();
const taskListSchema = z
  .object({
    items: z.array(
      z.object({ id: z.string(), title: z.string(), statusId: z.string() }).passthrough(),
    ),
  })
  .passthrough();
const idSchema = z.object({ id: z.string(), name: z.string() }).passthrough();
const workflowSchema = z
  .object({
    id: z.string(),
    statuses: z.array(
      z.object({ id: z.string(), name: z.string(), category: z.string() }).passthrough(),
    ),
  })
  .passthrough();
const memberListSchema = z
  .object({
    items: z.array(
      z.object({ email: z.string(), userId: z.string(), role: z.string() }).passthrough(),
    ),
  })
  .passthrough();

const owner = {
  email: "tsr-owner@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "소유자",
  workspaceSlug: "tsre2e",
  workspaceName: "Stream Resync E2E",
};

const member = {
  email: "tsr-member@example.com",
  password: "memberpass1",
  givenName: "스트림",
  familyName: "멤버",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = workspaceListSchema.parse(await workspacesRes.json());
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  assert(workspace);
  return workspace.id;
}

async function ensureOwnerWorkspace(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(owner.familyName);
    await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
    await page.getByLabel("이메일").fill(owner.email);
    await page.getByLabel("비밀번호").fill(owner.password);
    await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
    await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
    await expect
      .poll(async () => {
        const res = await page.request.get("/api/v1/me/workspaces");
        if (!res.ok()) return [];
        const body = workspaceSlugListSchema.parse(await res.json());
        return body.items.map((item) => item.slug);
      })
      .toContain(owner.workspaceSlug);
    return;
  }
  if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, owner.email, owner.password);
  }
}

test("peer task create invalidates task list in another tab", async ({ browser }) => {
  test.setTimeout(120_000);

  const ownerContext = await browser.newContext();
  const memberContext = await browser.newContext();
  const ownerPage = await ownerContext.newPage();
  const memberPage = await memberContext.newPage();

  await ensureOwnerWorkspace(ownerPage);

  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });

  await ownerPage.goto(`/w/${owner.workspaceSlug}/projects`);
  await ownerPage.getByRole("button", { name: "새 프로젝트" }).click();
  await ownerPage.getByLabel("키").fill("TSR");
  await ownerPage.getByLabel("이름", { exact: true }).fill("Stream Lab");
  await ownerPage.getByLabel("공개 범위").selectOption("workspace");
  await ownerPage.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(ownerPage).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/TSR/tasks$`));

  const wsId = await workspaceId(ownerPage, owner.workspaceSlug);
  const projectsRes = await ownerPage.request.get(`/api/v1/workspaces/${wsId}/projects`);
  expect(projectsRes.ok()).toBe(true);
  const project = projectListSchema
    .parse(await projectsRes.json())
    .items.find((item: { key: string }) => item.key === "TSR");
  expect(project).toBeTruthy();
  assert(project);
  const projectId = project.id;

  const viewerTab = await ownerContext.newPage();
  const streamWait = viewerTab.waitForResponse(
    (res) =>
      res.url().includes(`/projects/${projectId}/stream`) && res.request().method() === "GET",
    { timeout: 30_000 },
  );
  await viewerTab.goto(`/w/${owner.workspaceSlug}/TSR/tasks`);
  const streamRes = await streamWait;
  expect(streamRes.status()).toBe(200);
  expect(streamRes.headers()["content-type"] ?? "").toContain("text/event-stream");
  await expect(viewerTab.getByRole("heading", { name: "Stream Lab" })).toBeVisible();
  const viewerListUrl = viewerTab.url();
  const viewerTaskList = viewerTab.locator(".task-status-list");
  await expect(viewerTab.getByText("태스크가 없습니다")).toBeVisible();
  await expect(viewerTaskList.locator("a.task-row")).toHaveCount(0);

  await login(memberPage, member.email, member.password);
  await memberPage.goto(`/w/${owner.workspaceSlug}/TSR/tasks`);
  await memberPage.getByRole("button", { name: "새 태스크" }).click();
  await memberPage.getByLabel("제목").fill("다른 탭 반영");
  await memberPage.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();

  await expect(viewerTab).toHaveURL(viewerListUrl);
  await expect(viewerTab.getByText("태스크가 없습니다")).toBeHidden({ timeout: 25_000 });
  const peerTaskTitle = viewerTaskList.locator(".task-row__title", { hasText: "다른 탭 반영" });
  await expect(peerTaskTitle).toBeVisible({ timeout: 25_000 });
  const peerTaskLink = viewerTaskList.getByRole("link", { name: /다른 탭 반영/ });
  await expect(peerTaskLink).toBeVisible();
  await expect(peerTaskLink).toHaveAttribute("href", /\/w\/tsre2e\/TSR-\d+$/);

  // Peer meta edits (title/date, then status) reach the viewer through the live stream.
  const tasksRes = await memberPage.request.get(
    `/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`,
  );
  expect(tasksRes.ok()).toBe(true);
  const peerTask = taskSchema.parse(
    taskListSchema
      .parse(await tasksRes.json())
      .items.find((item: { title: string }) => item.title === "다른 탭 반영"),
  );
  expect(peerTask).toBeTruthy();
  assert(peerTask);
  const workflowRes = await memberPage.request.get(
    `/api/v1/workspaces/${wsId}/projects/${projectId}/workflow`,
  );
  expect(workflowRes.ok()).toBe(true);
  const nextStatus = idSchema.parse(
    workflowSchema
      .parse(await workflowRes.json())
      .statuses.find((status: { id: string }) => status.id !== peerTask.statusId),
  );
  expect(nextStatus).toBeTruthy();
  assert(nextStatus);

  const metaRes = await memberPage.request.patch(
    `/api/v1/workspaces/${wsId}/tasks/${peerTask.id}`,
    {
      data: { title: "원격 수정", dueDate: "2027-01-15" },
    },
  );
  expect(metaRes.ok(), await metaRes.text()).toBe(true);
  await expect(viewerTaskList.locator(".task-row__title", { hasText: "원격 수정" })).toBeVisible({
    timeout: 25_000,
  });

  const statusRes = await memberPage.request.patch(
    `/api/v1/workspaces/${wsId}/tasks/${peerTask.id}`,
    { data: { statusId: nextStatus.id } },
  );
  expect(statusRes.ok(), await statusRes.text()).toBe(true);
  const nextSection = viewerTab
    .locator("section.task-status")
    .filter({ has: viewerTab.locator(".task-status__toggle", { hasText: nextStatus.name }) });
  await expect(nextSection.getByTestId(`task-row-${peerTask.id}`)).toBeVisible({
    timeout: 25_000,
  });
  await expect(viewerTab).toHaveURL(viewerListUrl);

  await ownerContext.close();
  await memberContext.close();
});

test("removed member is sent home after access stream closes", async ({ browser }) => {
  test.setTimeout(120_000);

  const ownerContext = await browser.newContext();
  const victimContext = await browser.newContext();
  const ownerPage = await ownerContext.newPage();
  const victimPage = await victimContext.newPage();

  await ensureOwnerWorkspace(ownerPage);

  const victimEmail = "tsr-removed@example.com";
  createE2eUser(victimEmail, "removedpass1", "제거", {
    familyName: "대상",
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });

  await login(victimPage, victimEmail, "removedpass1");
  await victimPage.goto(`/w/${owner.workspaceSlug}/projects`);
  await expect(victimPage.getByRole("heading", { name: "프로젝트" })).toBeVisible();

  const wsId = await workspaceId(ownerPage, owner.workspaceSlug);
  const membersRes = await ownerPage.request.get(`/api/v1/workspaces/${wsId}/members`);
  expect(membersRes.ok()).toBe(true);
  const victimId = memberListSchema
    .parse(await membersRes.json())
    .items.find(
      (item: { email: string }) => item.email.toLowerCase() === victimEmail.toLowerCase(),
    )?.userId;
  expect(victimId).toBeTruthy();
  assert(victimId);

  const removeRes = await ownerPage.request.delete(
    `/api/v1/workspaces/${wsId}/members/${victimId}`,
  );
  expect(removeRes.ok()).toBe(true);

  await expect(victimPage).toHaveURL(/\?denied=workspace/, { timeout: 25_000 });

  await ownerContext.close();
  await victimContext.close();
});
