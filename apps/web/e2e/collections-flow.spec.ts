import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "colx",
  workspaceName: "Collections Flow",
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
  if ((await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0) {
    await login(page, admin.email, admin.password);
  }
}

async function workspaceId(page: Page): Promise<string> {
  const res = await page.request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const body = (await res.json()) as { items: { id: string; slug: string }[] };
  const workspace = body.items.find((item) => item.slug === admin.workspaceSlug);
  expect(workspace).toBeTruthy();
  return workspace!.id;
}

test("document tags, project collection fields/views and saved task views round-trip through the UI", async ({
  page,
}) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const slug = admin.workspaceSlug;

  // 1. Workspace settings → 문서 태그: create a tag.
  const docRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: "태그 문서" },
  });
  expect(docRes.status()).toBe(201);
  const doc = (await docRes.json()) as { id: string; number: number };

  await page.goto(`/w/${slug}/settings`);
  await page.getByRole("link", { name: "문서 태그" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/settings/document-tags$`));
  await expect(page.getByRole("heading", { name: "문서 태그" })).toBeVisible();
  await expect(page.getByText("등록한 문서 태그가 없습니다")).toBeVisible();
  await page.getByLabel("이름", { exact: true }).fill("기획");
  await page.getByLabel("색", { exact: true }).selectOption("blue");
  await page.getByRole("button", { name: "만들기", exact: true }).click();
  const tagRow = page.getByTestId("document-tag-row-기획");
  await expect(tagRow).toBeVisible();
  await expect(tagRow.getByRole("cell").nth(2)).toHaveText("0");

  // 2. Assign it on the wiki document through the tags bar.
  await page.goto(`/w/${slug}/WIKI-${doc.number}`);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  const tagsBar = page.getByTestId("document-tags-bar");
  await tagsBar.getByRole("button", { name: "+ 태그" }).click();
  await tagsBar.getByRole("button", { name: "기획" }).click();
  await expect(tagsBar.getByRole("button", { name: "태그 제거: 기획" })).toBeVisible();
  await expect
    .poll(async () => {
      const res = await page.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}/tags`);
      return ((await res.json()) as { items: { name: string }[] }).items.map((tag) => tag.name);
    })
    .toEqual(["기획"]);
  await page.goto(`/w/${slug}/settings/document-tags`);
  await expect(page.getByTestId("document-tag-row-기획").getByRole("cell").nth(2)).toHaveText("1");

  // 3. Project with one task, then a select field on its task collection.
  await page.goto(`/w/${slug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("col");
  await page.getByLabel("이름", { exact: true }).fill("컬렉션 프로젝트");
  await page.getByLabel("공개 범위").selectOption("workspace");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/COL/tasks$`));

  const projectsRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  const project = ((await projectsRes.json()) as { items: { id: string; key: string }[] }).items.find(
    (item) => item.key === "COL",
  )!;
  const workflowRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/workflow`,
  );
  const statuses = ((await workflowRes.json()) as { statuses: { id: string; category: string }[] })
    .statuses;
  const openStatus = statuses.find((status) => status.category !== "done")!;
  const taskRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/tasks`,
    { data: { title: "속성 태스크", type: "task", statusId: openStatus.id } },
  );
  expect(taskRes.status()).toBe(201);
  const task = (await taskRes.json()) as { id: string; number: number };
  const displayId = `COL-${task.number}`;

  await page.goto(`/w/${slug}/COL/tasks`);
  await page.getByRole("navigation", { name: "프로젝트 관리 메뉴" }).getByRole("link", { name: "속성 설정" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/COL/settings/fields$`));
  const manager = page.getByTestId("collection-field-manager");
  await manager.getByLabel("속성 이름").fill("단계");
  await manager.getByLabel("속성 키").fill("stage");
  await manager.getByLabel("속성 유형").selectOption({ label: "단일 선택" });
  await manager.getByLabel("선택지 (한 줄에 하나)").fill("설계\n구현");
  await manager.getByRole("button", { name: "속성 추가" }).click();
  await expect(page.getByTestId("field-settings-stage")).toContainText("단계 · 단일 선택");

  const collectionRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/collection`,
  );
  const collection = (await collectionRes.json()) as { id: string };
  const fieldsRes = await page.request.get(
    `/api/v1/workspaces/${wsId}/collections/${collection.id}/fields`,
  );
  const field = (
    (await fieldsRes.json()) as {
      items: { id: string; key: string; options: { id: string; label: string }[] }[];
    }
  ).items.find((item) => item.key === "stage")!;
  const buildOption = field.options.find((option) => option.label === "구현")!;

  // 4. Table: set the value inline; it survives a reload and shows on the task page.
  await page.getByRole("link", { name: "표", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/COL/table$`));
  const row = page.getByTestId(`collection-row-${displayId}`);
  await expect(row).toContainText("속성 태스크");
  await row.getByLabel(`단계 · ${displayId}`).selectOption({ label: "구현" });
  await expect(row.getByLabel(`단계 · ${displayId}`)).toHaveValue(buildOption.id);
  await page.reload();
  await expect(
    page.getByTestId(`collection-row-${displayId}`).getByLabel(`단계 · ${displayId}`),
  ).toHaveValue(buildOption.id);

  // 5. Board grouped by the select field shows the task under "구현".
  // Cards use aria-label "그룹 기준 · COL-n"; match the toolbar control exactly.
  await page.getByRole("link", { name: "보드", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/COL/board$`));
  const boardPanel = page.locator('section[data-testid="collection-board"]');
  await expect(boardPanel).toBeVisible();
  await expect(boardPanel.getByTestId(`collection-card-${displayId}`)).toBeVisible();
  await boardPanel
    .locator(":scope > .collection-toolbar")
    .getByLabel("그룹 기준", { exact: true })
    .selectOption({ label: "단계" });
  const buildColumn = page.getByRole("region", { name: "구현" });
  await expect(buildColumn.getByTestId(`collection-card-${displayId}`)).toBeVisible();
  await expect(buildColumn.getByTestId(`collection-card-${displayId}`)).toContainText("단계: 구현");
  await expect(page.getByRole("region", { name: "설계" }).getByTestId(`collection-card-${displayId}`)).toHaveCount(0);

  // Calendar by due date: month window, day counts, previews and the day list.
  const dueRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/tasks`,
    { data: { title: "달력 태스크", type: "task", statusId: openStatus.id, dueDate: "2027-03-15" } },
  );
  expect(dueRes.status()).toBe(201);
  const dueTask = (await dueRes.json()) as { number: number };
  await page.getByRole("link", { name: "달력", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/COL/calendar$`));
  await page.locator('input[type="month"]').fill("2027-03");
  await expect(page.getByLabel("2027년 3월", { exact: true }).first()).toBeVisible();
  const calendar = page.getByTestId("collection-calendar");
  await expect(calendar.getByRole("link", { name: "달력 태스크" })).toBeVisible();
  await calendar.getByRole("button", { name: "2027-03-15 · 전체 1개" }).click();
  await expect(page.getByRole("heading", { name: "2027-03-15 항목" })).toBeVisible();
  await expect(page.getByTestId(`collection-row-COL-${dueTask.number}`)).toBeVisible();

  await page.goto(`/w/${slug}/${displayId}`);
  const properties = page.getByTestId("task-properties");
  await expect(properties.getByRole("heading", { name: "속성" })).toBeVisible();
  await expect(properties.getByLabel("단계", { exact: true })).toHaveValue(buildOption.id);

  // 6. Task list: save the current filters as a view, clear, and re-apply it.
  await page.goto(`/w/${slug}/COL/tasks`);
  await page.getByLabel("열린 태스크만").click();
  await expect(page.getByLabel("열린 태스크만")).toBeChecked();
  await expect(page).toHaveURL(/openOnly/);
  await page.getByRole("button", { name: "새 보기로 저장" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("태스크 보기 이름").fill("열린 태스크");
  await dialog.getByRole("button", { name: "태스크 보기 저장" }).click();
  await expect(dialog).toHaveCount(0);
  const viewSelect = page.getByLabel("저장한 태스크 보기 선택");
  await expect(viewSelect.locator("option:checked")).toHaveText("열린 태스크 · 백로그");
  await expect
    .poll(async () => {
      const res = await page.request.get(`/api/v1/workspaces/${wsId}/projects/${project.id}/views`);
      return ((await res.json()) as { items: { name: string; config: unknown }[] }).items;
    })
    .toEqual([
      expect.objectContaining({ name: "열린 태스크", config: { filters: { openOnly: true }, sort: [] } }),
    ]);

  await page.getByRole("button", { name: "필터 해제" }).click();
  await expect(page.getByLabel("열린 태스크만")).not.toBeChecked();
  await expect(page.getByText("저장되지 않은 변경")).toBeVisible();
  await viewSelect.selectOption({ label: "현재 검색" });
  await viewSelect.selectOption({ label: "열린 태스크 · 백로그" });
  await expect(page.getByLabel("열린 태스크만")).toBeChecked();
  await expect(page.getByTestId(`task-row-${task.id}`)).toBeVisible();

  // Update the saved view's config (compare-and-swap on expectedConfig).
  await page.getByLabel("정렬").selectOption({ label: "제목" });
  await page.getByRole("button", { name: "이 보기에 변경 저장" }).click();
  await expect(page.getByText("저장되지 않은 변경")).toHaveCount(0);
  await expect
    .poll(async () => {
      const res = await page.request.get(`/api/v1/workspaces/${wsId}/projects/${project.id}/views`);
      return ((await res.json()) as { items: { config: unknown }[] }).items[0]?.config;
    })
    .toEqual({ filters: { openOnly: true }, sort: [{ field: "title", direction: "asc" }] });
});
