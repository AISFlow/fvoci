import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "ops@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "운영",
  workspaceSlug: "tops",
  workspaceName: "Task Ops",
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

async function workspaceId(page: Page): Promise<string> {
  const res = await page.request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const ws = (await res.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  expect(ws).toBeTruthy();
  return ws.id;
}

async function createProject(page: Page, wsId: string, key: string): Promise<string> {
  const res = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key, name: `${key} Lab`, visibility: "workspace" },
  });
  expect(res.status(), await res.text()).toBe(201);
  return (await res.json()).id;
}

async function createTask(
  page: Page,
  wsId: string,
  projectId: string,
  body: Record<string, unknown>,
): Promise<{ id: string; number: number }> {
  const res = await page.request.post(`/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`, {
    data: body,
  });
  expect(res.status(), await res.text()).toBe(201);
  return res.json();
}

test("time entries, clone and permanent delete from the task detail", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const projectId = await createProject(page, wsId, "OPS");
  await createTask(page, wsId, projectId, { title: "시간 기록 대상" });

  await page.goto(`/w/${admin.workspaceSlug}/OPS-2`);
  await expect(page.getByRole("heading", { name: "시간 기록 대상" })).toBeVisible();

  const entries = page.getByTestId("task-time-entries");
  await entries.getByRole("button", { name: "기록 추가" }).click();
  await entries.getByLabel("시작").fill("2026-09-01T09:00");
  await entries.getByLabel("종료").fill("2026-09-01T10:30");
  await entries.getByLabel("메모").fill("리뷰");
  await entries.getByRole("button", { name: "기록 추가" }).click();
  await expect(page.getByTestId("task-time-total")).toHaveText("합계 1시간 30분");
  await expect(entries.getByText(/리뷰/)).toBeVisible();

  // A reversed range is refused before any request.
  await entries.getByRole("button", { name: "기록 추가" }).click();
  await entries.getByLabel("시작").fill("2026-09-02T10:00");
  await entries.getByLabel("종료").fill("2026-09-02T09:00");
  await entries.getByRole("button", { name: "기록 추가" }).click();
  await expect(entries.getByRole("alert")).toHaveText("종료가 시작보다 뒤여야 합니다.");
  await entries.getByRole("button", { name: "취소" }).click();

  await page.getByTestId("task-clone").click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/OPS-3$`));
  await expect(page.getByRole("heading", { name: "시간 기록 대상 복사" })).toBeVisible();
  // Time entries are not copied.
  await expect(page.getByTestId("task-time-total")).toHaveCount(0);

  await page.getByRole("button", { name: "태스크 삭제" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "태스크 삭제" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/OPS/tasks$`));
  const lookup = await page.request.get(`/api/v1/workspaces/${wsId}/lookup/OPS-3`);
  const items = lookup.ok() ? (await lookup.json()).items : [];
  expect(items.filter((item: { kind: string }) => item.kind === "task")).toHaveLength(0);
});

test("parent picker lists epics from the parents endpoint", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const projectId = await createProject(page, wsId, "PAR");
  await createTask(page, wsId, projectId, { title: "큰 묶음", type: "epic" });
  await createTask(page, wsId, projectId, { title: "작은 일" });

  await page.goto(`/w/${admin.workspaceSlug}/PAR-3`);
  await expect(page.getByRole("heading", { name: "작은 일" })).toBeVisible();
  await page.getByTestId("task-edit-parent").click();
  const panel = page.locator(".task-parent-select__panel");
  await expect(panel.getByRole("option", { name: "PAR-2 큰 묶음" })).toBeVisible();
  await page.getByTestId("task-edit-parent-search").fill("par-2");
  await expect(panel.getByRole("option", { name: "PAR-2 큰 묶음" })).toBeVisible();
  await page.getByTestId("task-edit-parent-search").fill("없는 제목");
  await expect(panel.getByText("선택할 수 있는 상위 태스크가 없습니다")).toBeVisible();
});

test("workflow settings add, rename and delete statuses", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const projectId = await createProject(page, wsId, "WFL");

  await page.goto(`/w/${admin.workspaceSlug}/WFL/settings/workflow`);
  const section = page.getByTestId("project-workflow");
  await expect(section.getByLabel("상태 이름")).toHaveCount(6);
  await section.getByLabel("워크플로 상태", { exact: true }).fill("QA");
  await section.getByRole("button", { name: "추가" }).click();
  await expect(section.getByLabel("상태 이름")).toHaveCount(7);
  await expect(section.getByLabel("상태 이름").nth(6)).toHaveValue("QA");

  await section.getByLabel("상태 이름").nth(6).fill("품질 확인");
  await section.getByRole("button", { name: "저장" }).nth(6).click();
  await expect
    .poll(async () => {
      const res = await page.request.get(
        `/api/v1/workspaces/${wsId}/projects/${projectId}/workflow`,
      );
      return (await res.json()).statuses.map((s: { name: string }) => s.name).at(-1);
    })
    .toBe("품질 확인");

  // A status in use cannot be deleted.
  const workflow = await (
    await page.request.get(`/api/v1/workspaces/${wsId}/projects/${projectId}/workflow`)
  ).json();
  const backlog = workflow.statuses[0];
  await createTask(page, wsId, projectId, { title: "사용 중", statusId: backlog.id });
  await section.getByRole("button", { name: "삭제" }).first().click();
  await expect(section.getByRole("alert")).toHaveText("태스크가 남아 있는 상태는 삭제할 수 없습니다");
  await expect(section.getByLabel("상태 이름")).toHaveCount(7);

  await section.getByRole("button", { name: "삭제" }).nth(6).click();
  await expect(section.getByLabel("상태 이름")).toHaveCount(6);
});

test("my tasks lists open assigned tasks across projects", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const me = await (await page.request.get("/api/v1/auth/me")).json();
  const alpha = await createProject(page, wsId, "MYA");
  const beta = await createProject(page, wsId, "MYB");
  const mine = await createTask(page, wsId, alpha, { title: "내 일 하나", dueDate: "2026-10-01" });
  const mine2 = await createTask(page, wsId, beta, { title: "내 일 둘" });
  await createTask(page, wsId, beta, { title: "남의 일" });
  for (const task of [mine, mine2]) {
    const res = await page.request.patch(`/api/v1/workspaces/${wsId}/tasks/${task.id}`, {
      data: { assigneeIds: [me.userId] },
    });
    expect(res.ok()).toBe(true);
  }

  await page.goto(`/w/${admin.workspaceSlug}/projects`);
  await page.getByRole("link", { name: "내 태스크" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/my-tasks$`));
  const list = page.getByTestId("my-tasks");
  await expect(list.getByTestId(`my-task-${mine.id}`)).toContainText("MYA-2");
  await expect(list.getByTestId(`my-task-${mine.id}`)).toContainText("백로그");
  await expect(list.getByTestId(`my-task-${mine2.id}`)).toContainText("MYB-2");
  await expect(list.getByText("남의 일")).toHaveCount(0);

  await list.getByTestId(`my-task-${mine2.id}`).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/MYB-2$`));
});
