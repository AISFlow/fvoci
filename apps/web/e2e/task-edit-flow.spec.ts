import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tef",
  workspaceName: "Task Edit Flow",
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
    return;
  }
  if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
}

async function workflowStatusId(
  page: Page,
  workspaceIdValue: string,
  projectId: string,
  category: string,
): Promise<string> {
  const res = await page.request.get(
    `/api/v1/workspaces/${workspaceIdValue}/projects/${projectId}/workflow`,
  );
  expect(res.ok()).toBe(true);
  const body = await res.json();
  const status = body.statuses.find((item: { category: string }) => item.category === category);
  expect(status).toBeTruthy();
  return status.id;
}

test("task edit flow covers fields, conflicts, recurrence, trash and restore", async ({
  page,
  context,
}) => {
  await ensureSetup(page);
  await page.goto(`/w/${admin.workspaceSlug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("edt");
  await page.getByLabel("이름", { exact: true }).fill("Edit Lab");
  await page.getByLabel("공개 범위").selectOption("workspace");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/EDT/tasks$`));

  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("편집 대상");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/EDT-2$`));

  const wsId = await workspaceId(page, admin.workspaceSlug);
  const projectsRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  const project = (await projectsRes.json()).items.find((item: { key: string }) => item.key === "EDT");
  expect(project).toBeTruthy();
  const doneStatusId = await workflowStatusId(page, wsId, project.id, "done");
  const todoStatusId = await workflowStatusId(page, wsId, project.id, "todo");

  await page.getByTestId("task-edit-title").fill("반복 일감");
  await page.getByTestId("task-edit-title").blur();
  await expect(page.getByTestId("task-edit-title")).toHaveValue("반복 일감");

  await page.getByTestId("task-edit-priority").selectOption("high");
  await page.getByTestId("task-edit-start-date").fill("2026-01-31");
  await page.getByTestId("task-edit-start-date").blur();
  await page.getByTestId("task-edit-due-date").fill("2026-01-31");
  await page.getByTestId("task-edit-due-date").blur();
  await page.getByTestId("task-edit-estimate").fill("3.5");
  await page.getByTestId("task-edit-estimate").blur();
  await page.getByTestId("task-edit-recurrence").selectOption("daily");
  await expect.poll(async () => {
    const lookupRes = await page.request.get(`/api/v1/workspaces/${wsId}/lookup/EDT-2`);
    if (!lookupRes.ok()) return null;
    const taskEntry = (await lookupRes.json()).items.find(
      (item: { kind: string }) => item.kind === "task",
    );
    if (!taskEntry) return null;
    const detailRes = await page.request.get(
      `/api/v1/workspaces/${wsId}/tasks/${taskEntry.id}`,
    );
    if (!detailRes.ok()) return null;
    const detail = await detailRes.json();
    return detail.recurrence?.kind ?? null;
  }).toBe("daily");

  const lookup = await page.request.get(`/api/v1/workspaces/${wsId}/lookup/EDT-2`);
  const taskId = lookup
    .ok()
    ? (await lookup.json()).items.find((item: { kind: string }) => item.kind === "task")?.id
    : null;
  expect(taskId).toBeTruthy();

  const detailAfterPatch = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
  expect(detailAfterPatch.ok()).toBe(true);
  const detailBody = await detailAfterPatch.json();
  expect(detailBody.title).toBe("반복 일감");
  expect(detailBody.priority).toBe("high");
  expect(detailBody.estimate).toBe("3.5");
  expect(detailBody.recurrence).toEqual({ kind: "daily" });

  const initialStatusId = detailBody.statusId;
  const secondTab = await openSecondTab(context, page.url());
  const staleMove = await page.request.post(`/api/v1/workspaces/${wsId}/tasks/${taskId}/move`, {
    data: { statusId: todoStatusId, expectedStatusId: initialStatusId },
  });
  expect(staleMove.ok()).toBe(true);
  await secondTab.getByTestId("task-edit-status").selectOption(doneStatusId);
  await expect(secondTab.getByTestId("task-edit-action-error")).toContainText(
    "다른 곳에서 먼저 수정되었습니다",
  );
  await secondTab.close();
  await page.reload();
  await expect(page.getByTestId("task-edit-title")).toHaveValue("반복 일감");

  await page.getByTestId("task-edit-status").selectOption(doneStatusId);
  await expect.poll(async () => {
    const detailRes = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
    if (!detailRes.ok()) return "";
    return (await detailRes.json()).statusId;
  }).toBe(doneStatusId);
  await expect.poll(async () => {
    const listRes = await page.request.get(
      `/api/v1/workspaces/${wsId}/projects/${project.id}/tasks?limit=50`,
    );
    if (!listRes.ok()) return 0;
    const listBody = await listRes.json();
    return listBody.items.length;
  }).toBe(2);

  const listRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/tasks?limit=50`,
  );
  const spawned = (await listRes.json()).items.find(
    (item: { title: string; id: string }) => item.title === "반복 일감" && item.id !== taskId,
  );
  expect(spawned).toBeTruthy();

  page.once("dialog", (dialog) => dialog.accept());
  await page.getByTestId("task-edit-trash").click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/EDT/tasks$`));

  const trashedGet = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
  expect(trashedGet.status()).toBe(404);

  const restoreRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/tasks/${taskId}/restore`,
  );
  expect(restoreRes.ok()).toBe(true);

  await page.goto(`/w/${admin.workspaceSlug}/EDT-2`);
  await expect(page.getByTestId("task-edit-title")).toHaveValue("반복 일감");
});

async function openSecondTab(context: BrowserContext, url: string): Promise<Page> {
  const second = await context.newPage();
  await second.goto(url);
  await expect(second.getByTestId("task-edit-title")).toBeVisible();
  return second;
}
