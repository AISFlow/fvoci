import { expect, test, type Page } from "@playwright/test";
import { FIELD_TYPES, fieldTakesOptions } from "../src/lib/collection-values";
import { createE2eUser, login, logout, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });
const slug = "psvue";
const owner = { email: "settings-owner@example.com", password: "settingspass123" };

async function setup(page: Page): Promise<string> {
  await page.goto("/");
  await expect(page.getByRole("button", { name: "시작하기" }).or(page.getByRole("button", { name: "로그아웃" })).or(page.getByRole("button", { name: "로그인", exact: true }))).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("설정");
    await page.getByLabel("이메일").fill(owner.email);
    await page.getByLabel("비밀번호").fill(owner.password);
    await page.getByLabel("워크스페이스 이름").fill("Project settings Vue");
    await page.getByLabel("주소(영문)").fill(slug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else if (await page.getByRole("button", { name: "로그인", exact: true }).count()) {
    await login(page, owner.email, owner.password);
  }
  const response = await page.request.get("/api/v1/me/workspaces");
  expect(response.ok()).toBe(true);
  const workspace = (await response.json()).items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

async function createProject(page: Page, workspaceId: string, key: string, visibility = "workspace") {
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, { data: { key, name: `Settings ${key}`, visibility } });
  expect(response.status()).toBe(201);
  return (await response.json()) as { id: string };
}

async function collectionId(page: Page, base: string, projectId: string): Promise<string> {
  const response = await page.request.get(`${base}/projects/${projectId}/collection`);
  expect(response.ok()).toBe(true);
  return (await response.json()).id;
}

type Field = { id: string; key: string; name: string; type: string; description: string | null; version: number; deletedAt: string | null; options: { id: string; label: string; deletedAt: string | null }[] };
async function fields(page: Page, base: string, collection: string): Promise<Field[]> {
  const response = await page.request.get(`${base}/collections/${collection}/fields`);
  expect(response.ok()).toBe(true);
  return (await response.json()).items;
}

test("field definitions, options, labels and milestones save and reload in the Vue settings route", async ({ page }) => {
  const csp = watchCspViolations(page);
  const workspaceId = await setup(page);
  const base = `/api/v1/workspaces/${workspaceId}`;
  const project = await createProject(page, workspaceId, "FIELD");
  const collection = await collectionId(page, base, project.id);
  await page.goto(`/w/${slug}/FIELD/tasks`);
  await expect(page.getByRole("heading", { name: "Settings FIELD" })).toBeVisible();
  await page.evaluate(() => { (window as unknown as { settingsMarker: string }).settingsMarker = "same-runtime"; });
  await page.getByRole("navigation", { name: "프로젝트 관리 메뉴" }).getByRole("link", { name: "속성 설정" }).click();
  await expect(page).toHaveURL(/\/FIELD\/settings\/fields$/);
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  expect(await page.evaluate(() => (window as unknown as { settingsMarker?: string }).settingsMarker)).toBe("same-runtime");
  const manager = page.getByTestId("collection-field-manager");
  await manager.getByLabel("속성 이름", { exact: true }).fill("단계");
  await manager.getByLabel("속성 키", { exact: true }).fill("stage");
  await manager.getByLabel("속성 유형").selectOption("select");
  await manager.getByLabel("선택지 (한 줄에 하나)").fill("설계\n구현\n설계");
  await manager.getByRole("button", { name: "속성 추가" }).click();
  let field = page.getByTestId("field-settings-stage");
  await expect(field).toContainText("단계 · 단일 선택");
  await field.locator("summary").click();
  await field.getByLabel("속성 이름", { exact: true }).fill("개발 단계");
  await field.getByLabel("설명", { exact: true }).fill("  단계 설명  ");
  await field.getByLabel("선택지 이름 1", { exact: true }).fill("기획");
  await field.getByTestId("field-option").nth(1).getByRole("button", { name: "위로" }).click();
  await field.getByTestId("field-option").nth(1).getByRole("button", { name: "보관", exact: true }).click();
  await field.getByRole("button", { name: "선택지 추가" }).click();
  await field.getByLabel("선택지 이름 3", { exact: true }).fill("검증");
  await field.getByRole("button", { name: "저장", exact: true }).click();
  await expect.poll(async () => (await fields(page, base, collection)).find(item => item.key === "stage")?.name).toBe("개발 단계");
  let stored = (await fields(page, base, collection)).find(item => item.key === "stage")!;
  expect(stored.description).toBe("단계 설명");
  expect(stored.options.map(option => option.label)).toEqual(["구현", "기획", "검증"]);
  expect(stored.options[1]!.deletedAt).not.toBeNull();
  await page.reload();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  field = page.getByTestId("field-settings-stage");
  await field.locator("summary").click();
  await expect(field.getByLabel("설명", { exact: true })).toHaveValue("단계 설명");
  await field.getByTestId("field-option").nth(1).getByRole("button", { name: "복원", exact: true }).click();
  await field.getByRole("button", { name: "저장", exact: true }).click();
  await expect.poll(async () => (await fields(page, base, collection)).find(item => item.key === "stage")?.options[1]?.deletedAt).toBeNull();
  await field.locator("summary").click();
  await field.locator("div.flex.flex-wrap.gap-2").getByRole("button", { name: "보관", exact: true }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "보관", exact: true }).click();
  await expect.poll(async () => (await fields(page, base, collection)).find(item => item.key === "stage")?.deletedAt).not.toBeNull();
  await field.locator("summary").click();
  await field.locator("div.flex.flex-wrap.gap-2").getByRole("button", { name: "복원", exact: true }).click();
  await expect.poll(async () => (await fields(page, base, collection)).find(item => item.key === "stage")?.deletedAt).toBeNull();

  const labels = page.getByTestId("project-labels-settings");
  await labels.getByLabel("라벨", { exact: true }).fill("긴급");
  await labels.getByLabel("색", { exact: true }).selectOption("red");
  await labels.getByRole("button", { name: "추가", exact: true }).click();
  await expect(labels.getByText("긴급 (red)")).toBeVisible();
  await page.getByTestId("project-milestone-name").fill("출시");
  await page.getByTestId("project-milestone-add").click();
  await expect(page.locator('[data-testid^="project-milestone-name-"]')).toHaveValue("출시");
  await page.reload();
  await expect(labels.getByText("긴급 (red)")).toBeVisible();
  await expect(page.locator('[data-testid^="project-milestone-name-"]')).toHaveValue("출시");
  await labels.getByRole("button", { name: "삭제", exact: true }).click();
  await expect(labels.getByText("긴급 (red)")).toHaveCount(0);
  await page.locator('[data-testid^="project-milestone-delete-"]').click();
  await expect(page.locator('[data-testid^="project-milestone-name-"]')).toHaveCount(0);
  await page.reload();
  await expect(labels.getByText("긴급 (red)")).toHaveCount(0);
  await expect(page.locator('[data-testid^="project-milestone-name-"]')).toHaveCount(0);
  expect(csp).toEqual([]);
});

test("every existing field type and real version conflict are handled without losing the refusal", async ({ page }) => {
  const workspaceId = await setup(page);
  const base = `/api/v1/workspaces/${workspaceId}`;
  const project = await createProject(page, workspaceId, "TYPES");
  const collection = await collectionId(page, base, project.id);
  await page.goto(`/w/${slug}/TYPES/settings/fields`);
  const manager = page.getByTestId("collection-field-manager");
  for (const type of FIELD_TYPES) {
    await manager.getByLabel("속성 이름", { exact: true }).fill(`Field ${type}`);
    await manager.getByLabel("속성 키", { exact: true }).fill(type);
    await manager.getByLabel("속성 유형").selectOption(type);
    if (fieldTakesOptions(type)) await manager.getByLabel("선택지 (한 줄에 하나)").fill("A\nB");
    await manager.getByRole("button", { name: "속성 추가" }).click();
    await expect(page.getByTestId(`field-settings-${type}`)).toBeVisible();
  }
  expect((await fields(page, base, collection)).map(field => field.type)).toEqual([...FIELD_TYPES]);
  await page.reload();
  for (const type of FIELD_TYPES) await expect(page.getByTestId(`field-settings-${type}`)).toBeVisible();
  const field = page.getByTestId("field-settings-text");
  await field.locator("summary").click();
  await field.getByLabel("속성 이름", { exact: true }).fill("Stale local draft");
  const stored = (await fields(page, base, collection)).find(item => item.key === "text")!;
  const response = await page.request.patch(`${base}/collections/${collection}/fields/${stored.id}`, { data: { expectedVersion: stored.version, name: "Remote field name" } });
  expect(response.ok()).toBe(true);
  const refused = page.waitForResponse(res => res.request().method() === "PATCH" && res.url().endsWith(`/fields/${stored.id}`));
  await field.getByRole("button", { name: "저장", exact: true }).click();
  expect((await refused).status()).toBe(409);
  await expect(manager.getByRole("alert")).toBeVisible();
  await expect(field.locator("summary")).toContainText("Remote field name");
  expect((await fields(page, base, collection)).find(item => item.id === stored.id)?.name).toBe("Remote field name");
  await page.reload();
  await expect(field.locator("summary")).toContainText("Remote field name");
});

test("workflow create, rename, all categories and delete persist; an occupied status refuses deletion", async ({ page }) => {
  const workspaceId = await setup(page);
  const base = `/api/v1/workspaces/${workspaceId}`;
  const project = await createProject(page, workspaceId, "FLOW");
  await page.goto(`/w/${slug}/FLOW/settings/fields`);
  await expect(page.getByTestId("collection-field-manager")).toBeVisible();
  await page.evaluate(() => { (window as unknown as { settingsMarker: string }).settingsMarker = "same-runtime"; });
  await page.getByRole("navigation", { name: "프로젝트 관리 메뉴" }).getByRole("link", { name: "워크플로", exact: true }).click();
  await expect(page).toHaveURL(/\/FLOW\/settings\/workflow$/);
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  expect(await page.evaluate(() => (window as unknown as { settingsMarker?: string }).settingsMarker)).toBe("same-runtime");
  const section = page.getByTestId("project-workflow");
  const creation = section.locator("form").last();
  await creation.getByRole("textbox").fill("검토");
  await creation.locator("select").selectOption("in_progress");
  await creation.getByRole("button").click();
  const workflow = async () => {
    const response = await page.request.get(`${base}/projects/${project.id}/workflow`);
    expect(response.ok()).toBe(true);
    return (await response.json()) as { id: string; statuses: { id: string; name: string; category: string }[] };
  };
  await expect.poll(async () => (await workflow()).statuses.some(status => status.name === "검토")).toBe(true);
  const added = (await workflow()).statuses.find(status => status.name === "검토")!;
  const row = page.getByTestId(`workflow-status-${added.id}`);
  await row.getByRole("textbox").fill("품질 검토");
  await row.locator("select").selectOption("done");
  await row.getByRole("button", { name: "저장", exact: true }).click();
  await expect.poll(async () => (await workflow()).statuses.find(status => status.id === added.id)).toMatchObject({ name: "품질 검토", category: "done" });
  await page.reload();
  await expect(row.getByRole("textbox")).toHaveValue("품질 검토");
  await expect(row.locator("select")).toHaveValue("done");
  for (const category of ["backlog", "todo", "in_progress", "canceled"]) {
    await row.locator("select").selectOption(category);
    await row.getByRole("button", { name: "저장", exact: true }).click();
    await expect.poll(async () => (await workflow()).statuses.find(status => status.id === added.id)?.category).toBe(category);
  }
  const task = await page.request.post(`${base}/projects/${project.id}/tasks`, { data: { title: "상태 사용", type: "task", statusId: added.id } });
  expect(task.status()).toBe(201);
  const taskId = (await task.json()).id;
  const refused = page.waitForResponse(res => res.request().method() === "DELETE" && res.url().endsWith(`/statuses/${added.id}`));
  await row.getByRole("button", { name: "삭제", exact: true }).click();
  expect((await refused).status()).toBe(409);
  await expect(section.getByRole("alert")).toBeVisible();
  await expect(row).toBeVisible();
  expect((await workflow()).statuses.some(status => status.id === added.id)).toBe(true);
  const fallback = (await workflow()).statuses.find(status => status.id !== added.id)!;
  expect((await page.request.patch(`${base}/tasks/${taskId}`, { data: { statusId: fallback.id } })).ok()).toBe(true);
  await row.getByRole("button", { name: "삭제", exact: true }).click();
  await expect(row).toHaveCount(0);
  await page.reload();
  await expect(row).toHaveCount(0);
  expect((await workflow()).statuses.some(status => status.id === added.id)).toBe(false);
});

test("viewer, revoked private access and archived projects cannot change settings", async ({ page, browser }) => {
  const workspaceId = await setup(page);
  const base = `/api/v1/workspaces/${workspaceId}`;
  const project = await createProject(page, workspaceId, "ACL", "private");
  const collection = await collectionId(page, base, project.id);
  const created = await page.request.post(`${base}/collections/${collection}/fields`, { data: { key: "protected", name: "Protected", type: "text" } });
  expect(created.status()).toBe(201);
  const field = (await created.json()) as Field;
  const flowResponse = await page.request.get(`${base}/projects/${project.id}/workflow`);
  expect(flowResponse.ok()).toBe(true);
  const workflow = (await flowResponse.json()) as { id: string; statuses: { id: string }[] };
  const viewer = { email: "settings-viewer@example.com", password: "viewerpass123" };
  createE2eUser(viewer.email, viewer.password, "뷰어", { workspaceSlug: slug, membershipRole: "guest" });
  const members = await page.request.get(`${base}/members`);
  expect(members.ok()).toBe(true);
  const viewerId = (await members.json()).items.find((member: { email: string }) => member.email === viewer.email).userId;
  expect((await page.request.post(`${base}/projects/${project.id}/members`, { data: { userId: viewerId, role: "viewer" } })).status()).toBe(201);
  const context = await browser.newContext();
  const denied = await context.newPage();
  try {
    await login(denied, viewer.email, viewer.password);
    await denied.goto(`/w/${slug}/ACL/settings/fields`);
    await expect(denied.locator("#root[data-v-app]")).toHaveCount(1);
    await expect(denied.getByTestId("field-settings-protected")).toBeVisible();
    await expect(denied.getByRole("button", { name: "속성 추가" })).toHaveCount(0);
    await expect(denied.getByTestId("project-milestone-add")).toHaveCount(0);
    const writeField = `${base}/collections/${collection}/fields/${field.id}`;
    expect((await denied.request.patch(writeField, { data: { expectedVersion: field.version, name: "Denied" } })).status()).toBe(403);
    expect((await denied.request.post(`${base}/collections/${collection}/fields`, { data: { name: "Denied", type: "text" } })).status()).toBe(403);
    expect((await denied.request.post(`${base}/projects/${project.id}/labels`, { data: { name: "Denied", color: "gray" } })).status()).toBe(404);
    expect((await denied.request.post(`${base}/projects/${project.id}/milestones`, { data: { name: "Denied" } })).status()).toBe(404);
    await denied.goto(`/w/${slug}/ACL/settings/workflow`);
    await expect(denied.getByTestId("project-workflow")).toBeVisible();
    await expect(denied.getByTestId("project-workflow").getByRole("button")).toHaveCount(0);
    const statusesUrl = `${base}/workflows/${workflow.id}/statuses`;
    expect((await denied.request.post(statusesUrl, { data: { name: "Denied", category: "todo" } })).status()).toBe(404);
    expect((await denied.request.patch(`${statusesUrl}/${workflow.statuses[0]!.id}`, { data: { name: "Denied" } })).status()).toBe(404);
    expect((await denied.request.delete(`${statusesUrl}/${workflow.statuses[0]!.id}`)).status()).toBe(404);
    expect((await page.request.delete(`${base}/projects/${project.id}/members/${viewerId}`)).ok()).toBe(true);
    for (const view of ["fields", "workflow"]) {
      await denied.goto(`/w/${slug}/ACL/settings/${view}`);
      await expect(denied.getByRole("alert")).toContainText("프로젝트를 찾을 수 없습니다");
      await expect(denied.getByRole("heading", { name: "Settings ACL" })).toHaveCount(0);
    }
    expect((await denied.request.get(`${base}/collections/${collection}/fields`)).status()).toBe(404);
    expect((await denied.request.get(`${base}/projects/${project.id}/workflow`)).status()).toBe(404);
  } finally {
    await context.close();
  }
  expect((await page.request.post(`${base}/projects/${project.id}/archive`)).ok()).toBe(true);
  await page.goto(`/w/${slug}/ACL/settings/fields`);
  await expect(page.getByTestId("field-settings-protected")).toBeVisible();
  await expect(page.getByRole("button", { name: "속성 추가" })).toHaveCount(0);
  await expect(page.getByTestId("project-milestone-add")).toHaveCount(0);
  await page.goto(`/w/${slug}/ACL/settings/workflow`);
  await expect(page.getByTestId("project-workflow")).toBeVisible();
  await expect(page.getByTestId("project-workflow").getByRole("button")).toHaveCount(0);
  expect((await page.request.patch(`${base}/collections/${collection}/fields/${field.id}`, { data: { expectedVersion: field.version, name: "Denied" } })).status()).toBe(409);
  expect((await page.request.post(`${base}/workflows/${workflow.id}/statuses`, { data: { name: "Denied", category: "todo" } })).status()).toBe(409);
  expect((await fields(page, base, collection)).find(item => item.id === field.id)?.name).toBe("Protected");
  await logout(page);
});
