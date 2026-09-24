import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, createTasksViaApi, login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "pt-member@example.com",
  password: "memberpass1",
  givenName: "멤버",
  familyName: "박",
};

const guest = {
  email: "pt-guest@example.com",
  password: "guestpass1",
  givenName: "게스트",
  familyName: "최",
};

const other = {
  email: "pt-other@example.com",
  password: "otherpass1",
  givenName: "다른",
  familyName: "한",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
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
    await expect.poll(async () => {
      const res = await page.request.get("/api/v1/me/workspaces");
      if (!res.ok()) return [];
      const body = (await res.json()) as { items: { slug: string }[] };
      return body.items.map((item) => item.slug);
    }).toContain(admin.workspaceSlug);
    return;
  }
  if (page.url().includes("/login") || (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0) {
    await login(page, admin.email, admin.password);
  }
}

test("member creates a workspace project, task, and sees counts after reload", async ({ page }) => {
  await ensureSetup(page);
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });

  if ((await page.getByRole("button", { name: "로그아웃" }).count()) > 0) {
    await page.getByRole("button", { name: "로그아웃" }).click();
    await expect(page).toHaveURL(/\/login$/);
  }
  await login(page, member.email, member.password);

  await page.goto("/w/acme/projects");
  await expect(page.getByRole("heading", { name: "프로젝트" })).toBeVisible();
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("lab");
  await expect(page.getByLabel("키")).toHaveValue("LAB");
  await page.getByLabel("이름", { exact: true }).fill("Lab");
  await page.getByLabel("공개 범위").selectOption("workspace");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();

  await expect(page).toHaveURL(/\/w\/acme\/LAB\/tasks$/);
  await expect(page.getByRole("heading", { name: "Lab" })).toBeVisible();

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("첫 일");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();

  await expect(page).toHaveURL(/\/w\/acme\/LAB-2$/);
  const idAfterCreate = await workspaceId(page, "acme");
  const labLookup = await page.request.get(
    `/api/v1/workspaces/${idAfterCreate}/lookup/LAB-2`,
  );
  expect(labLookup.status()).toBe(200);
  const labLookupBody = await labLookup.json();
  expect(
    labLookupBody.items.some(
      (entry: { kind: string; displayId: string }) =>
        entry.kind === "task" && entry.displayId === "LAB-2",
    ),
  ).toBe(true);
  await expect(page.getByRole("heading", { name: "첫 일" })).toBeVisible();
  await expect(page.getByText("LAB-2")).toBeVisible();

  await page.goto("/w/acme/LAB-1");
  const labRootLookup = await page.request.get(
    `/api/v1/workspaces/${idAfterCreate}/lookup/LAB-1`,
  );
  expect(labRootLookup.status()).toBe(200);
  const labRootBody = await labRootLookup.json();
  expect(
    labRootBody.items.some(
      (entry: { kind: string; displayId: string; projectId: string | null }) =>
        entry.kind === "document" && entry.displayId === "LAB-1" && entry.projectId,
    ),
  ).toBe(true);
  await expect(
    page.getByText("프로젝트 문서는 이 슬라이스에서 아직 지원하지 않습니다."),
  ).toBeVisible();
  await expect(page.getByText("태스크를 찾을 수 없습니다")).toHaveCount(0);
  await expect(page).toHaveURL(/\/w\/acme\/LAB-1$/);

  await page.goto("/w/acme/x");
  await expect(page.getByRole("alert")).toContainText("요청한 항목을 찾을 수 없습니다");
  await expect(page).toHaveURL(/\/w\/acme\/x$/);
  await expect(page.getByRole("heading", { name: "위키" })).toHaveCount(0);

  await page.goto("/w/acme/LAB-2");
  await expect(page.getByRole("heading", { name: "첫 일" })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { name: "첫 일" })).toBeVisible();
  await expect(page.getByText("LAB-2")).toBeVisible();

  await page.goto("/w/acme/projects");
  await expect(page.getByRole("link", { name: /Lab/ })).toBeVisible();
  await expect(page.getByText("미완료 태스크 1개")).toBeVisible();

  const id = await workspaceId(page, "acme");
  const listRes = await page.request.get(`/api/v1/workspaces/${id}/projects`);
  expect(listRes.ok()).toBe(true);
  const lab = (await listRes.json()).items.find((item: { key: string }) => item.key === "LAB");
  expect(lab).toBeTruthy();
  expect(lab.taskCount).toBe(1);
  expect(lab.openTaskCount).toBe(1);

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
});

test("guest create is rejected with a visible error and wiki still loads", async ({ page }) => {
  createE2eUser(guest.email, guest.password, guest.givenName, {
    familyName: guest.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "guest",
  });

  await login(page, guest.email, guest.password);
  await page.goto("/w/acme/projects");
  await expect(page.getByRole("heading", { name: "프로젝트" })).toBeVisible();
  await expect(page.getByRole("link", { name: /Lab/ })).toHaveCount(0);

  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("GST");
  await page.getByLabel("이름", { exact: true }).fill("Guest project");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page.getByRole("alert")).toContainText("찾을 수 없습니다");
  await expect(page).toHaveURL(/\/w\/acme\/projects$/);

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
});

test("private project is absent for non-members and viewer writes fail visibly", async ({ page }) => {
  createE2eUser(other.email, other.password, other.givenName, {
    familyName: other.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });

  await login(page, member.email, member.password);
  await page.goto("/w/acme/projects");
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("HID");
  await page.getByLabel("이름", { exact: true }).fill("Hidden");
  await page.getByLabel("공개 범위").selectOption("private");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/HID\/tasks$/);

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("비밀 초안");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/HID-2$/);

  const id = await workspaceId(page, "acme");

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, admin.email, admin.password);
  const maskedLookup = await page.request.get(`/api/v1/workspaces/${id}/lookup/HID-2`);
  expect(maskedLookup.status()).toBe(200);
  expect((await maskedLookup.json()).items).toEqual([]);
  await page.goto("/w/acme/HID-2");
  await expect(page.getByRole("alert")).toContainText("태스크를 찾을 수 없습니다");
  await expect(page.getByRole("heading", { name: "비밀 초안" })).toHaveCount(0);
  const adminMe = await page.request.get("/api/v1/auth/me");
  expect(adminMe.ok()).toBe(true);
  const adminId = (await adminMe.json()).userId;

  await page.goto("/w/acme/projects");
  await expect(page.getByRole("heading", { name: "프로젝트" })).toBeVisible();
  await expect(page.getByText("Hidden")).toHaveCount(0);
  await page.goto("/w/acme/HID/tasks");
  await expect(page.getByRole("alert")).toContainText("프로젝트를 찾을 수 없습니다");

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, other.email, other.password);
  await page.goto("/w/acme/projects");
  await expect(page.getByText("Hidden")).toHaveCount(0);

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, member.email, member.password);
  const projectsRes = await page.request.get(`/api/v1/workspaces/${id}/projects`);
  const hid = (await projectsRes.json()).items.find((item: { key: string }) => item.key === "HID");
  expect(hid).toBeTruthy();
  const addRes = await page.request.post(`/api/v1/workspaces/${id}/projects/${hid.id}/members`, {
    data: { userId: adminId, role: "viewer" },
  });
  expect(addRes.status()).toBe(201);

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/projects");
  await expect(page.getByRole("link", { name: /Hidden/ })).toBeVisible();
  await page.getByRole("link", { name: /Hidden/ }).click();
  await expect(page).toHaveURL(/\/w\/acme\/HID\/tasks$/);
  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("비밀 일");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText("찾을 수 없습니다");
  await expect(page).toHaveURL(/\/w\/acme\/HID\/tasks$/);
});

test("duplicate and reserved keys keep the form and show errors", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/projects");
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("LAB");
  await page.getByLabel("이름", { exact: true }).fill("Lab copy");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText(
    "다른 곳에서 먼저 수정되었습니다",
  );
  await expect(page).toHaveURL(/\/w\/acme\/projects$/);

  await page.getByLabel("키").fill("WIKI");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText("예약된 키");
});

test("wiki shell and foreign workspace denial still hold after project flow", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByText("현재 역할: 멤버")).toBeVisible();

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, guest.email, guest.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByText("현재 역할: 게스트")).toBeVisible();
  await expect(page.getByRole("button", { name: "새 문서" })).toHaveCount(0);

  const foreign = {
    email: "pt-foreign@example.com",
    password: "foreignpass1",
    givenName: "외부",
    familyName: "정",
  };
  createE2eUser(foreign.email, foreign.password, foreign.givenName, {
    familyName: foreign.familyName,
  });
  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, foreign.email, foreign.password);
  await page.goto("/w/acme/settings");
  await expect(page).toHaveURL(/\?denied=workspace$/);
  await expect(page.getByRole("alert")).toContainText("접근 권한");
  await page.getByRole("button", { name: "닫기" }).click();
  await expect(page).not.toHaveURL(/\?denied=workspace/);

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
});

test("task list uses server statusCounts and paginates without duplicate rows", async ({
  page,
}) => {
  await login(page, member.email, member.password);
  const id = await workspaceId(page, "acme");
  const createProject = await page.request.post(`/api/v1/workspaces/${id}/projects`, {
    data: { key: "PAG", name: "Pages", visibility: "workspace" },
  });
  expect(
    createProject.status(),
    `create PAG project failed: ${createProject.status()} ${await createProject.text()}`,
  ).toBe(201);
  const project = await createProject.json();

  const workflowRes = await page.request.get(
    `/api/v1/workspaces/${id}/projects/${project.id}/workflow`,
  );
  expect(
    workflowRes.ok(),
    `workflow failed: ${workflowRes.status()} ${await workflowRes.text()}`,
  ).toBe(true);
  const workflow = await workflowRes.json();
  const backlog =
    workflow.statuses.find((status: { category: string }) => status.category === "backlog") ??
    workflow.statuses[0];
  expect(backlog).toBeTruthy();

  const created = 55;
  const titles = Array.from({ length: created }, (_, index) => `페이지 일 ${index + 1}`);
  await createTasksViaApi(page, id, project.id, titles, backlog.id);

  const listUrl = `/api/v1/workspaces/${id}/projects/${project.id}/tasks`;
  const listRes = await page.request.get(listUrl);
  const listText = await listRes.text();
  expect(listRes.ok(), `list tasks failed: ${listRes.status()} ${listText}`).toBe(true);
  const firstPage = JSON.parse(listText) as {
    items: { id: string }[];
    nextCursor: string | null;
    statusCounts: { statusId: string; count: number }[];
  };
  expect(firstPage.items.length).toBeGreaterThan(0);
  expect(firstPage.nextCursor).toBeTruthy();
  const serverCount = firstPage.statusCounts.find((row) => row.statusId === backlog.id)?.count;
  expect(serverCount).toBe(created);

  await page.goto("/w/acme/PAG/tasks");
  await expect(page.getByRole("heading", { name: "Pages" })).toBeVisible();
  const countBadge = page.locator(".task-status__count").first();
  await expect(countBadge).toHaveText(String(serverCount));
  await expect(page.locator(".task-row")).toHaveCount(firstPage.items.length);

  await page.getByRole("button", { name: "더 보기" }).click();
  await expect(page.locator(".task-row")).toHaveCount(created);
  const ids = await page.locator(".task-row__id").allTextContents();
  expect(new Set(ids).size).toBe(ids.length);
  expect(ids).toHaveLength(created);
});
