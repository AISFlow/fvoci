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
    const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${project.id}/labels`, { data: { name: `Label ${index}`, color: "blue" } });
    expect(response.status(), await response.text()).toBe(201);
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
  expect((await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${old.id}/archive`)).ok()).toBe(true);
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
  await expect(rows.first()).toContainText("10. 1.");
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
  const start = Date.now();
  await page.clock.install({ time: start });
  await page.clock.pauseAt(start + 1000);
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

test("wiki tag URLs include child-only matches and project documents; unfiltered drag moves and sorts persist", async ({ page }) => {
  await login(page, owner.email, owner.password);
  const wiki = [];
  for (const title of ["Wiki parent", "Wiki second", "Wiki third"]) {
    const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, { data: { title, parentId: null } });
    expect(response.status()).toBe(201);
    wiki.push(await response.json());
  }
  const childResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, { data: { title: "Tagged child", parentId: wiki[0].id } });
  expect(childResponse.status()).toBe(201);
  const child = await childResponse.json();
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${project.id}/documents`, { data: { title: "Tagged project child", parentId: project.rootDocumentId } });
  expect(projectResponse.status()).toBe(201);
  const projectChild = await projectResponse.json();
  const tagResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/document-tags`, { data: { name: "Planning", color: "gray" } });
  expect(tagResponse.status()).toBe(201);
  const tag = await tagResponse.json();
  for (const [doc, path] of [[child, `/api/v1/workspaces/${workspaceId}/documents/${child.id}/tags`], [projectChild, `/api/v1/workspaces/${workspaceId}/projects/${project.id}/documents/${projectChild.id}/tags`]] as const) {
    expect((await page.request.post(path, { data: { tagId: tag.id } })).ok()).toBe(true);
    expect(doc.id).toBeTruthy();
  }
  await page.goto(`/w/parity/wiki?tag=${tag.id}`);
  const selected = page.getByRole("button", { name: "Planning", exact: true });
  await expect(selected).toHaveAttribute("aria-pressed", "true");
  const childLink = page.getByRole("link", { name: /Tagged child/ });
  await expect(childLink).toBeVisible();
  await expect(childLink).toHaveAttribute("draggable", "false");
  await expect(page.getByRole("link", { name: /Tagged project child/ })).toBeVisible();
  await expect(page.getByRole("link", { name: /Wiki parent/ })).toHaveCount(0);
  await page.reload();
  await expect(childLink).toBeVisible();
  await selected.click();
  await expect(page).toHaveURL(/\/w\/parity\/wiki$/);
  const parent = page.getByTestId(`wiki-doc-${wiki[0].displayId}`);
  const second = page.getByTestId(`wiki-doc-${wiki[1].displayId}`);
  const third = page.getByTestId(`wiki-doc-${wiki[2].displayId}`);
  const sorted = page.waitForResponse(response => new URL(response.url()).pathname === `/api/v1/workspaces/${workspaceId}/documents/${wiki[2].id}/sort` && response.request().method() === "POST");
  await third.dragTo(parent, { targetPosition: { x: 25, y: 1 } });
  expect((await sorted).ok()).toBe(true);
  const roots = page.locator(".wiki-home__section > ul > li > a");
  await expect(roots.first()).toContainText("Wiki third");
  const moved = page.waitForResponse(response => new URL(response.url()).pathname === `/api/v1/workspaces/${workspaceId}/documents/${wiki[1].id}/move` && response.request().method() === "POST");
  await second.dragTo(parent);
  expect((await moved).ok()).toBe(true);
  const parentBranch = parent.locator("..");
  await expect(parentBranch.getByRole("link", { name: /Wiki second/ })).toBeVisible();
  await page.reload();
  await expect(roots.first()).toContainText("Wiki third");
  await expect(parentBranch.getByRole("link", { name: /Wiki second/ })).toBeVisible();
  expect((await (await page.request.get(`/api/v1/workspaces/${workspaceId}/documents/${wiki[1].id}`)).json()).parentId).toBe(wiki[0].id);
  await page.goto("/w/parity/wiki?tag=malformed");
  await expect(page.getByRole("alert")).toBeVisible();
});

test("source tag:name search filters documents and leaves task hits in the real API", async ({ page }) => {
  await login(page, owner.email, owner.password);
  const tag = (await (await page.request.get(`/api/v1/workspaces/${workspaceId}/document-tags`)).json()).items.find((tag: { name: string }) => tag.name === "Planning");
  expect(tag).toBeTruthy();
  const created = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`, { data: { title: "Tagged task" } });
  expect(created.status()).toBe(201);
  await expect.poll(async () => {
    const result = await page.request.get(`/api/v1/workspaces/${workspaceId}/search?q=Tagged&type=all&tag=${tag.id}`);
    expect(result.ok()).toBe(true);
    return (await result.json()).items.map((item: { title: string }) => item.title).sort();
  }).toEqual(["Tagged child", "Tagged project child", "Tagged task"]);
  await page.goto("/w/parity/search?q=tag%3Aplanning%20Tagged");
  await expect(page.getByRole("link", { name: /Tagged child/ })).toBeVisible();
  await expect(page.getByRole("link", { name: /Tagged task/ })).toBeVisible();
  await page.getByRole("tab", { name: "댓글", exact: true }).click();
  await expect(page.getByText("결과가 없습니다", { exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "태스크", exact: true }).click();
  await expect(page.getByRole("link", { name: /Tagged task/ })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("link", { name: /Tagged task/ })).toBeVisible();
});
