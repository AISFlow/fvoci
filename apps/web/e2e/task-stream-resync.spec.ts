import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login } from "./helpers";

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
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
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
    await expect.poll(async () => {
      const res = await page.request.get("/api/v1/me/workspaces");
      if (!res.ok()) return [];
      const body = (await res.json()) as { items: { slug: string }[] };
      return body.items.map((item) => item.slug);
    }).toContain(owner.workspaceSlug);
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
  const project = (await projectsRes.json()).items.find((item: { key: string }) => item.key === "TSR");
  expect(project).toBeTruthy();
  const projectId = project.id as string;

  const viewerTab = await ownerContext.newPage();
  const streamWait = viewerTab.waitForResponse(
    (res) =>
      res.url().includes(`/projects/${projectId}/stream`) &&
      res.request().method() === "GET",
    { timeout: 30_000 },
  );
  await viewerTab.goto(`/w/${owner.workspaceSlug}/TSR/tasks`);
  const streamRes = await streamWait;
  expect(streamRes.status()).toBe(200);
  expect(streamRes.headers()["content-type"] ?? "").toContain("text/event-stream");
  await expect(viewerTab.getByRole("heading", { name: "Stream Lab" })).toBeVisible();

  await login(memberPage, member.email, member.password);
  await memberPage.goto(`/w/${owner.workspaceSlug}/TSR/tasks`);
  await memberPage.getByRole("button", { name: "새 태스크" }).click();
  await memberPage.getByLabel("제목").fill("다른 탭 반영");
  await memberPage.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();

  const peerTaskLink = viewerTab.getByRole("link", { name: /다른 탭 반영/ });
  await expect(peerTaskLink).toBeVisible({ timeout: 25_000 });
  await expect(peerTaskLink).toHaveAttribute("href", /\/w\/tsre2e\/TSR-\d+$/);

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
  const victimId = (await membersRes.json()).items.find(
    (item: { email: string }) => item.email.toLowerCase() === victimEmail.toLowerCase(),
  )?.userId;
  expect(victimId).toBeTruthy();

  const removeRes = await ownerPage.request.delete(
    `/api/v1/workspaces/${wsId}/members/${victimId}`,
  );
  expect(removeRes.ok()).toBe(true);

  await expect(victimPage).toHaveURL(/\?denied=workspace/, { timeout: 25_000 });

  await ownerContext.close();
  await victimContext.close();
});
