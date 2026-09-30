import { expect, test } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });
const owner = { email: "parity@example.com", password: "paritypass123" };
let workspaceId: string;
let project: { id: string; key: string; rootDocumentId: string };
let tasks: { id: string; title: string }[] = [];

test("workspace landing has authorized projects, counts, and eight due-ordered assigned rows", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("동등");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Parity workspace");
  await page.getByLabel("주소(영문)").fill("parity");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  workspaceId = (await (await page.request.get("/api/v1/me/workspaces")).json()).items[0].id;
  const me = await (await page.request.get("/api/v1/auth/me")).json();
  expect((await page.request.patch("/api/v1/auth/me", { data: { givenName: me.givenName, timezone: "Pacific/Honolulu" } })).ok()).toBe(true);
  const created = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, { data: { key: "META", name: "Metadata project", visibility: "workspace" } });
  expect(created.status()).toBe(201);
  project = await created.json();
  const labels = [];
  for (let index = 0; index < 3; index++) {
    const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${project.id}/labels`, { data: { name: `Label ${index}`, color: "#0284c7" } });
    expect(response.status()).toBe(201);
    labels.push((await response.json()).id);
  }
  // Reverse insertion order witnesses the actual server due-date ordering and limit.
  for (let index = 9; index >= 1; index--) {
    const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`, { data: { title: `Due ${index}`, dueDate: `2026-10-${String(index).padStart(2, "0")}`, type: "bug", priority: "high" } });
    expect(response.status()).toBe(201);
    const task = await response.json();
    tasks.push(task);
    expect((await page.request.patch(`/api/v1/workspaces/${workspaceId}/tasks/${task.id}`, { data: { assigneeIds: [me.userId], labelIds: labels } })).ok()).toBe(true);
  }
  const archived = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, { data: { key: "OLD", name: "Archived project", visibility: "workspace" } });
  expect(archived.status()).toBe(201);
  const old = await archived.json();
  expect((await page.request.patch(`/api/v1/workspaces/${workspaceId}/projects/${old.id}`, { data: { status: "archived" } })).ok()).toBe(true);
  const preview = page.waitForResponse(response => {
    const url = new URL(response.url());
    return url.pathname === `/api/v1/workspaces/${workspaceId}/tasks` && url.searchParams.get("limit") === "8";
  });
  await page.goto("/w/parity");
  expect((await preview).ok()).toBe(true);
  const rows = page.getByTestId("workspace-assigned").locator("a.task-row");
  await expect(rows).toHaveCount(8);
  await expect(rows.first()).toContainText("Due 1");
  await expect(rows.last()).toContainText("Due 8");
  await expect(rows.first()).toContainText("버그");
  await expect(rows.first()).toContainText("높음");
  await expect(rows.first()).toContainText("Label 0");
  await expect(rows.first()).toContainText("Label 1");
  await expect(rows.first()).toContainText("+1");
  await expect(rows.first().getByTitle("김동등")).toBeVisible();
  const counts = await (await page.request.get("/api/v1/me/workspaces")).json();
  await expect(page.getByTestId("workspace-totals")).toContainText(`문서 ${counts.items[0].documentCount}개`);
  await expect(page.getByTestId("workspace-totals")).toContainText("프로젝트 2개");
  await expect(page.getByRole("link", { name: /OLD Archived project/ })).toBeVisible();
  await page.reload();
  await expect(rows).toHaveCount(8);
});

test("full my-tasks keeps metadata and navigation after reload", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await page.goto("/w/parity/my-tasks");
  const list = page.getByTestId("my-tasks");
  await expect(list.locator("a.task-row")).toHaveCount(9);
  const task = tasks.find(task => task.title === "Due 1")!;
  const row = list.getByTestId(`my-task-${task.id}`);
  await expect(row).toContainText("Label 0");
  await expect(row).toContainText("높음");
  await expect(row.getByTitle("김동등")).toBeVisible();
  await page.reload();
  await expect(row).toContainText("버그");
  await row.click();
  await expect(page.getByRole("heading", { name: "Due 1", exact: true })).toBeVisible();
});

test("search draft commits after 300ms and project scope can clear and restore", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await page.goto(`/w/parity/search?q=old&tab=task&projectId=${project.id}`);
  const input = page.locator("#workspace-search-q");
  await expect(input).toBeVisible();
  await page.clock.install();
  await input.fill("Due");
  await page.clock.runFor(299);
  await expect(page).toHaveURL(/q=old/);
  await page.clock.runFor(1);
  await expect(page).toHaveURL(/q=Due/);
  await expect(page.getByRole("radio", { name: /이 프로젝트만/ })).toBeChecked();
  await page.getByRole("radio", { name: "워크스페이스 전체" }).check();
  expect(new URL(page.url()).searchParams.has("projectId")).toBe(false);
  await expect(page.getByRole("radio", { name: /이 프로젝트만/ })).toBeVisible();
  await page.getByRole("radio", { name: /이 프로젝트만/ }).check();
  await expect(page).toHaveURL(new RegExp(`projectId=${project.id}`));
  await page.reload();
  await expect(input).toHaveValue("Due");
  await expect(page.getByRole("radio", { name: /이 프로젝트만/ })).toBeChecked();
});

test("trash timestamp follows the saved user zone instead of the browser zone", async ({ page }) => {
  await login(page, owner.email, owner.password);
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, { data: { title: "Zone trash", parentId: null } });
  expect(response.status()).toBe(201);
  const doc = await response.json();
  expect((await page.request.post(`/api/v1/workspaces/${workspaceId}/documents/${doc.id}/trash`)).ok()).toBe(true);
  await page.goto("/w/parity/trash");
  const row = page.locator(".trash-page__row").filter({ hasText: "Zone trash" });
  const time = row.locator("time");
  await expect(time).toBeVisible();
  const iso = await time.getAttribute("datetime");
  const expected = await page.evaluate(value => new Date(value!).toLocaleString("ko-KR", { hour12: false, timeZone: "Pacific/Honolulu", year: "numeric", month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }), iso);
  await expect(time).toHaveText(expected);
  await page.reload();
  await expect(time).toHaveText(expected);
});
