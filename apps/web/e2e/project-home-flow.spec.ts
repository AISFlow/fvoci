import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login, logout } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "phome",
  workspaceName: "Project Home E2E",
};

const member = {
  email: "ph-member@example.com",
  password: "memberpass1",
  givenName: "홈",
  familyName: "멤버",
};

const leadCandidate = {
  email: "ph-lead@example.com",
  password: "leadpass1",
  givenName: "리드",
  familyName: "후보",
};

const viewer = {
  email: "ph-viewer@example.com",
  password: "viewerpass1",
  givenName: "뷰어",
  familyName: "관찰",
};

const outsider = {
  email: "ph-outsider@example.com",
  password: "outsiderpass1",
  givenName: "외부",
  familyName: "인",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === slug,
  );
  expect(workspace).toBeTruthy();
  return workspace.id;
}

async function projectByKey(
  page: Page,
  wsId: string,
  key: string,
): Promise<{ id: string; rootDocumentId: string }> {
  const res = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  expect(res.ok()).toBe(true);
  const project = (await res.json()).items.find((item: { key: string }) => item.key === key);
  expect(project).toBeTruthy();
  expect(project.rootDocumentId).toBeTruthy();
  return { id: project.id, rootDocumentId: project.rootDocumentId };
}

async function memberUserId(page: Page, wsId: string, email: string): Promise<string> {
  const res = await page.request.get(`/api/v1/workspaces/${wsId}/members`);
  expect(res.ok()).toBe(true);
  const userId = (await res.json()).items.find(
    (item: { email: string }) => item.email.toLowerCase() === email.toLowerCase(),
  )?.userId;
  expect(userId).toBeTruthy();
  return userId;
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

async function openProjectTasks(page: Page, key: string): Promise<void> {
  await page.goto(`/w/${admin.workspaceSlug}/${key}/tasks`);
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/${key}/tasks$`));
}

test("project home shows wiki, documents can be created and moved in the tree", async ({ page }) => {
  await ensureSetup(page);
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
  createE2eUser(leadCandidate.email, leadCandidate.password, leadCandidate.givenName, {
    familyName: leadCandidate.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });

  await logout(page);
  await login(page, member.email, member.password);

  await page.goto(`/w/${admin.workspaceSlug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("home");
  await expect(page.getByLabel("키")).toHaveValue("HOME");
  await page.getByLabel("이름", { exact: true }).fill("Home Wiki");
  await page.getByLabel("공개 범위").selectOption("private");
  const wsId = await workspaceId(page, admin.workspaceSlug);
  const leadUserId = await memberUserId(page, wsId, leadCandidate.email);
  await page.locator("#project-lead").selectOption(leadUserId);
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();

  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/HOME$`));
  await expect(page.getByRole("heading", { name: "Home Wiki" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByText("비공개")).toBeVisible();

  const homeProject = await projectByKey(page, wsId, "HOME");
  const membersRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/members`,
  );
  expect(membersRes.ok()).toBe(true);
  const leadMember = (await membersRes.json()).items.find(
    (row: { userId: string; role: string }) => row.userId === leadUserId,
  );
  expect(leadMember?.role).toBe("lead");

  const firstCreate = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().includes(`/projects/${homeProject.id}/documents`) &&
      response.status() === 201,
  );
  await page.getByRole("button", { name: "새 문서" }).click();
  await firstCreate;
  await expect(page.getByTestId("project-doc-HOME-2")).toBeVisible();

  const secondCreate = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().includes(`/projects/${homeProject.id}/documents`) &&
      response.status() === 201,
  );
  await page.getByRole("button", { name: "새 문서" }).click();
  await secondCreate;
  await expect(page.getByTestId("project-doc-HOME-3")).toBeVisible();

  const treeRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/documents`,
  );
  expect(treeRes.ok()).toBe(true);
  const treeItems = (await treeRes.json()).items as { id: string; number: number }[];
  const parentDoc = treeItems.find((item) => item.number === 2);
  const childDoc = treeItems.find((item) => item.number === 3);
  expect(parentDoc).toBeTruthy();
  expect(childDoc).toBeTruthy();

  const moveRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/documents/${childDoc!.id}/move`,
    { data: { newParentId: parentDoc!.id } },
  );
  expect(moveRes.ok()).toBe(true);

  await page.reload();
  const branch = page.locator(".wiki-tree__branch").filter({ has: page.getByTestId("project-doc-HOME-2") });
  await expect(branch.getByTestId("project-doc-HOME-3")).toBeVisible();
  await expect(branch.locator(".wiki-tree--nested")).toBeVisible();
});

test("clone copies workflow labels and milestones but not tasks", async ({ page }) => {
  await login(page, leadCandidate.email, leadCandidate.password);
  const wsId = await workspaceId(page, admin.workspaceSlug);
  const homeProject = await projectByKey(page, wsId, "HOME");

  const labelRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/labels`,
    { data: { name: "bug", color: "red" } },
  );
  expect(labelRes.status()).toBe(201);
  const labelId = (await labelRes.json()).id as string;

  await openProjectTasks(page, "HOME");
  await expect(page.getByTestId("project-milestones")).toBeVisible();
  await page.getByTestId("project-milestone-name").fill("출시");
  await page.getByTestId("project-milestone-add").click();
  await expect(page.locator('[data-testid^="project-milestone-name-"]')).toHaveValue("출시");

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("복제 원본 태스크");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page.getByRole("heading", { name: "복제 원본 태스크" })).toBeVisible();

  await page.goto(`/w/${admin.workspaceSlug}/projects`);
  await page
    .locator(".project-list__row")
    .filter({ hasText: "Home Wiki" })
    .getByRole("button", { name: "복제" })
    .click();
  await page.getByLabel("키").fill("cpy");
  await expect(page.getByLabel("키")).toHaveValue("CPY");
  await page.getByLabel("이름", { exact: true }).fill("Home Wiki (복사)");
  await page.getByRole("dialog").getByRole("button", { name: "복제" }).click();

  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/CPY$`));
  await expect(page.getByRole("heading", { name: "Home Wiki (복사)" })).toBeVisible();

  await openProjectTasks(page, "CPY");
  await expect(page.locator('[data-testid^="project-milestone-name-"]')).toHaveValue("출시");
  await expect(page.locator(".task-row")).toHaveCount(0);

  const cpyProject = await projectByKey(page, wsId, "CPY");
  const sourceWorkflow = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/workflow`,
  );
  const cloneWorkflow = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${cpyProject.id}/workflow`,
  );
  expect(sourceWorkflow.ok()).toBe(true);
  expect(cloneWorkflow.ok()).toBe(true);
  const sourceNames = (await sourceWorkflow.json()).statuses.map((row: { name: string }) => row.name);
  const cloneNames = (await cloneWorkflow.json()).statuses.map((row: { name: string }) => row.name);
  expect(cloneNames).toEqual(sourceNames);

  const cloneLabels = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${cpyProject.id}/labels`,
  );
  expect(cloneLabels.ok()).toBe(true);
  expect(
    (await cloneLabels.json()).items.some((row: { name: string }) => row.name === "bug"),
  ).toBe(true);

  const sourceTasks = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/tasks`,
  );
  const cloneTasks = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${cpyProject.id}/tasks`,
  );
  expect((await sourceTasks.json()).items.length).toBeGreaterThan(0);
  expect((await cloneTasks.json()).items).toEqual([]);

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("복제 검증");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page.getByRole("heading", { name: "복제 검증" })).toBeVisible();
  await expect(page.getByTestId(`task-edit-label-${labelId}`)).toHaveCount(0);
  const cloneLabelsAfter = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${cpyProject.id}/labels`,
  );
  expect(cloneLabelsAfter.ok()).toBe(true);
  const cloneLabel = (await cloneLabelsAfter.json()).items.find(
    (row: { name: string }) => row.name === "bug",
  );
  expect(cloneLabel).toBeTruthy();
  await expect(page.getByTestId(`task-edit-label-${cloneLabel.id}`)).toBeVisible();
});

test("viewer and non-member cannot create project documents", async ({ page }) => {
  createE2eUser(viewer.email, viewer.password, viewer.givenName, {
    familyName: viewer.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
  createE2eUser(outsider.email, outsider.password, outsider.givenName, {
    familyName: outsider.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });

  await login(page, leadCandidate.email, leadCandidate.password);
  const wsId = await workspaceId(page, admin.workspaceSlug);
  const homeProject = await projectByKey(page, wsId, "HOME");
  const viewerId = await memberUserId(page, wsId, viewer.email);

  const grant = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/members`,
    { data: { userId: viewerId, role: "viewer" } },
  );
  expect(grant.status()).toBe(201);

  await logout(page);
  await login(page, viewer.email, viewer.password);
  await page.goto(`/w/${admin.workspaceSlug}/HOME`);
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  const beforeCount = await page.getByTestId(/^project-doc-HOME-/).count();
  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page.getByTestId(/^project-doc-HOME-/)).toHaveCount(beforeCount);

  const viewerCreate = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/documents`,
    { data: { parentId: homeProject.rootDocumentId, title: "뷰어 차단" } },
  );
  expect(viewerCreate.status()).toBe(404);

  await logout(page);
  await login(page, outsider.email, outsider.password);
  await page.goto(`/w/${admin.workspaceSlug}/projects`);
  await expect(page.getByRole("link", { name: /Home Wiki/ })).toHaveCount(0);

  const outsiderCreate = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${homeProject.id}/documents`,
    { data: { parentId: homeProject.rootDocumentId, title: "외부 차단" } },
  );
  expect(outsiderCreate.status()).toBe(404);
});
