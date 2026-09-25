import { expect, test, type Page } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

test("owner adds a holiday and copies an ICS feed that lists dated tasks", async ({ page }) => {
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
  await page.getByLabel("키").fill("cal");
  await page.getByLabel("이름", { exact: true }).fill("일정");
  await page.getByLabel("공개 범위").selectOption("workspace");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/CAL/tasks$`));

  const wsId = await workspaceId(page, owner.workspaceSlug);
  const projects = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  expect(projects.ok()).toBe(true);
  const projectId = (await projects.json()).items.find((item: { key: string }) => item.key === "CAL")
    ?.id as string;
  expect(projectId).toBeTruthy();
  const createdTask = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`,
    { data: { title: "공개 일정", dueDate: "2026-08-26" } },
  );
  expect(createdTask.status()).toBe(201);
  const taskId = ((await createdTask.json()) as { id: string }).id;
  // The feed exports the owner's assigned tasks, as in the source.
  const me = (await (await page.request.get("/api/v1/auth/me")).json()) as { userId: string };
  const assigned = await page.request.patch(`/api/v1/workspaces/${wsId}/tasks/${taskId}`, {
    data: { assigneeIds: [me.userId] },
  });
  expect(assigned.ok(), await assigned.text()).toBe(true);
  const other = await page.request.post(`/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`, {
    data: { title: "담당 없는 일정", dueDate: "2026-08-27" },
  });
  expect(other.status()).toBe(201);

  await page.goto(`/w/${owner.workspaceSlug}/settings`);
  await page.locator("summary").filter({ hasText: /달력 구독/ }).click();
  await expect(page.getByText("등록된 공휴일이 없습니다")).toBeVisible();
  await page.getByLabel("공휴일 날짜").fill("2026-09-01");
  await page.getByRole("button", { name: "공휴일 추가" }).click();
  await expect(page.getByRole("status").filter({ hasText: "공휴일 변경을 저장했습니다" })).toBeVisible();
  await expect(page.locator("time").filter({ hasText: "2026-09-01" })).toBeVisible();

  const tokenResp = page.waitForResponse(
    (response) => response.url().includes("/ics-token") && response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "구독 URL 복사" }).click();
  const created = await tokenResp;
  expect(created.ok()).toBe(true);
  const { url } = (await created.json()) as { url: string };
  expect(url).toContain("/api/v1/ics/");

  const feed = await page.request.get(new URL(url).pathname);
  expect(feed.ok()).toBe(true);
  expect(feed.headers()["content-type"] ?? "").toContain("text/calendar");
  const ics = await feed.text();
  expect(ics).toContain("BEGIN:VCALENDAR");
  expect(ics).toContain(`UID:${taskId}@fvoci`);
  expect(ics).toContain("SUMMARY:공개 일정");
  expect(ics).toContain("DTSTART;VALUE=DATE:20260826");
  expect(ics).not.toContain("담당 없는 일정");

  const holidays = await page.request.get(`/api/v1/workspaces/${wsId}/holidays`);
  expect(holidays.ok()).toBe(true);
  const body = await holidays.json();
  expect(body.canEdit).toBe(true);
  expect(body.items).toContain("2026-09-01");
});
