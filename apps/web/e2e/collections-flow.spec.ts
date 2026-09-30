import { expect, test, type Page } from "@playwright/test";
import { createTasksViaApi, login } from "./helpers";

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
  await expect(calendar.getByRole("button", { name: "달력 태스크", exact: true })).toBeVisible();
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

test("grouped board pages each column and moves cards by drag or select through the API", async ({
  page,
}) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const slug = admin.workspaceSlug;
  const base = `/api/v1/workspaces/${wsId}`;

  const projectRes = await page.request.post(`${base}/projects`, {
    data: { key: "BRD", name: "보드 프로젝트", visibility: "workspace" },
  });
  expect(projectRes.status()).toBe(201);
  const project = (await projectRes.json()) as { id: string };
  const workflow = (await (await page.request.get(`${base}/projects/${project.id}/workflow`)).json()) as {
    id: string;
    statuses: { id: string; name: string }[];
  };
  const [statusA, statusB] = workflow.statuses;
  expect(statusA && statusB).toBeTruthy();
  const titlesA = Array.from({ length: 60 }, (_, index) => `보드 A ${String(index + 1).padStart(3, "0")}`);
  await createTasksViaApi(page, wsId, project.id, titlesA, statusA!.id);
  await createTasksViaApi(page, wsId, project.id, ["보드 B 001", "보드 B 002"], statusB!.id);

  const collection = (await (
    await page.request.get(`${base}/projects/${project.id}/collection`)
  ).json()) as { id: string };
  const fieldRes = await page.request.post(`${base}/collections/${collection.id}/fields`, {
    data: { name: "단계", key: "stage", type: "select", options: ["설계", "구현"] },
  });
  expect(fieldRes.status()).toBe(201);
  const field = (await fieldRes.json()) as {
    id: string;
    version: number;
    options: { id: string; label: string }[];
  };
  const design = field.options.find((option) => option.label === "설계")!;
  const build = field.options.find((option) => option.label === "구현")!;

  type Row = { id: string; displayId: string; statusId: string | null; values: Record<string, unknown>; version: number };
  async function itemRow(displayId: string): Promise<Row> {
    const res = await page.request.post(`${base}/collections/${collection.id}/query`, {
      data: { config: { query: { filters: {}, sort: [] }, groupBy: null, dateBy: null }, limit: 100 },
    });
    expect(res.ok()).toBe(true);
    const row = ((await res.json()) as { items: Row[] }).items.find((item) => item.displayId === displayId);
    expect(row).toBeTruthy();
    return row!;
  }
  // Grab the card by its padding (its centre is the keyboard select) and drop on the column head.
  const dragPoints = { sourcePosition: { x: 4, y: 4 }, targetPosition: { x: 20, y: 10 } };
  const cards = (column: ReturnType<Page["getByRole"]>) => column.locator('[data-testid^="collection-card-"]');
  async function cardIds(column: ReturnType<Page["getByRole"]>): Promise<string[]> {
    return cards(column).evaluateAll((nodes) =>
      nodes.map((node) => node.getAttribute("data-testid")!.replace("collection-card-", "")),
    );
  }
  const querySpy: { group: unknown; cursor: boolean }[] = [];
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith(`/collections/${collection.id}/query`)) {
      const body = request.postDataJSON() as { group?: unknown; cursor?: string };
      querySpy.push({ group: "group" in body ? body.group : "(all)", cursor: Boolean(body.cursor) });
    }
  });

  // Hold the project stream until the column's "load more" is in flight, and that
  // page until the stream has opened, so the stream's `open` resync lands while the
  // page loads: the order that dropped the requested page.
  const streamPath = `${base}/projects/${project.id}/stream`;
  let loadMoreSent!: () => void;
  const loadMoreInFlight = new Promise<void>((resolve) => {
    loadMoreSent = resolve;
  });
  await page.route((url) => url.pathname === streamPath, async (route) => {
    await loadMoreInFlight;
    await route.continue();
  });
  let nextPageHeld = false;
  await page.route(
    (url) => url.pathname === `${base}/collections/${collection.id}/query`,
    async (route) => {
      const body = route.request().postDataJSON() as { cursor?: string };
      if (nextPageHeld || !body.cursor) return route.continue();
      nextPageHeld = true;
      const streamOpened = page.waitForResponse(
        (response) => new URL(response.url()).pathname === streamPath,
      );
      loadMoreSent();
      await streamOpened;
      await route.continue();
    },
  );

  // 1. Status board: each column pages its own group; loading more is per column.
  // A viewport taller than a 60-card column keeps real mouse drags free of scrolling.
  await page.setViewportSize({ width: 1600, height: 7000 });
  await page.goto(`/w/${slug}/BRD/board`);
  const columnA = page.getByRole("region", { name: statusA!.name, exact: true });
  const columnB = page.getByRole("region", { name: statusB!.name, exact: true });
  await expect(cards(columnA)).toHaveCount(50);
  await expect(columnA.locator(".collection-board__head")).toContainText("60");
  await expect(cards(columnB)).toHaveCount(2);
  await expect(columnB.getByRole("button", { name: `${statusB!.name} · 더 보기` })).toHaveCount(0);
  await columnA.getByRole("button", { name: `${statusA!.name} · 더 보기` }).click();
  await expect(cards(columnA)).toHaveCount(60);
  expect(new Set(await cardIds(columnA)).size).toBe(60);
  await expect(columnA.getByRole("button", { name: `${statusA!.name} · 더 보기` })).toHaveCount(0);
  await expect(cards(columnB)).toHaveCount(2);
  // One catalog request without a group; every other request names its column.
  expect(querySpy.filter((entry) => entry.group === "(all)" && !entry.cursor).length).toBeGreaterThan(0);
  expect(querySpy.filter((entry) => entry.cursor).every((entry) => entry.group === statusA!.id)).toBe(true);

  // 2. Drag a second-page card to another status; it persists and survives reload.
  const movedId = (await cardIds(columnA)).at(-1)!;
  await columnA.getByTestId(`collection-card-${movedId}`).dragTo(columnB, dragPoints);
  await expect(columnB.getByTestId(`collection-card-${movedId}`)).toBeVisible();
  await expect(columnA.getByTestId(`collection-card-${movedId}`)).toHaveCount(0);
  await expect(cards(columnA)).toHaveCount(59);
  await expect.poll(async () => (await itemRow(movedId)).statusId).toBe(statusB!.id);
  await page.reload();
  await expect(cards(columnB)).toHaveCount(3);
  await expect(columnB.getByTestId(`collection-card-${movedId}`)).toBeVisible();
  await expect(page.getByTestId(`collection-card-${movedId}`)).toHaveCount(1);
  await expect(columnA.locator(".collection-board__head")).toContainText("59");

  // 2b. Ordinary 1280×720 viewport: scroll a second-page card into view and drag it with the
  //     real pointer onto the visible part of the destination column. Columns stretch to the
  //     tallest one, so the target column is under the pointer without scrolling mid-drag.
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.reload();
  await columnA.getByRole("button", { name: `${statusA!.name} · 더 보기` }).click();
  await expect(cards(columnA)).toHaveCount(59);
  const pointerId = (await cardIds(columnA))[55]!;
  const pointerCard = columnA.getByTestId(`collection-card-${pointerId}`);
  await pointerCard.scrollIntoViewIfNeeded();
  expect(await page.evaluate(() => window.scrollY)).toBeGreaterThan(0);
  const from = (await pointerCard.boundingBox())!;
  const to = (await columnB.boundingBox())!;
  expect(from.y + 4).toBeGreaterThanOrEqual(0);
  expect(from.y + 4).toBeLessThan(720);
  expect(to.y).toBeLessThan(from.y);
  expect(to.y + to.height).toBeGreaterThan(from.y + 4);
  await page.mouse.move(from.x + 4, from.y + 4);
  await page.mouse.down();
  await page.mouse.move(from.x + 40, from.y + 4, { steps: 4 });
  await page.mouse.move(to.x + 20, from.y + 4, { steps: 8 });
  await page.mouse.up();
  await expect(columnB.getByTestId(`collection-card-${pointerId}`)).toBeVisible();
  await expect(cards(columnA)).toHaveCount(58);
  await expect.poll(async () => (await itemRow(pointerId)).statusId).toBe(statusB!.id);
  await page.reload();
  await expect(columnB.getByTestId(`collection-card-${pointerId}`)).toBeVisible();
  await expect(page.getByTestId(`collection-card-${pointerId}`)).toHaveCount(1);
  await expect(cards(columnB)).toHaveCount(4);
  await page.setViewportSize({ width: 1600, height: 7000 });

  // 3. A rejected move (WIP limit) shows the server error and leaves the card in place.
  const wipRes = await page.request.patch(
    `${base}/workflows/${workflow.id}/statuses/${statusB!.id}`,
    { data: { wipLimit: 4 } },
  );
  expect(wipRes.ok()).toBe(true);
  const blockedId = (await cardIds(columnA))[0]!;
  await columnA.getByTestId(`collection-card-${blockedId}`).dragTo(columnB, dragPoints);
  await expect(page.getByRole("alert").filter({ hasText: "진행 중 제한" })).toBeVisible();
  await expect(columnA.getByTestId(`collection-card-${blockedId}`)).toBeVisible();
  await expect(columnB.getByTestId(`collection-card-${blockedId}`)).toHaveCount(0);
  expect((await itemRow(blockedId)).statusId).toBe(statusA!.id);
  expect(
    (await page.request.patch(`${base}/workflows/${workflow.id}/statuses/${statusB!.id}`, {
      data: { wipLimit: null },
    })).ok(),
  ).toBe(true);

  // 4. Group by the select field: option columns plus the unassigned column, each paged.
  await page
    .locator('section[data-testid="collection-board"] > .collection-toolbar')
    .getByLabel("그룹 기준", { exact: true })
    .selectOption({ label: "단계" });
  const designColumn = page.getByRole("region", { name: "설계", exact: true });
  const buildColumn = page.getByRole("region", { name: "구현", exact: true });
  const noneColumn = page.getByRole("region", { name: "미지정", exact: true });
  await expect(cards(noneColumn)).toHaveCount(50);
  await expect(noneColumn.locator(".collection-board__head")).toContainText("62");
  await expect(designColumn.getByText("현재 결과에 표시할 자료가 없습니다")).toBeVisible();
  await noneColumn.getByRole("button", { name: "미지정 · 더 보기" }).click();
  await expect(cards(noneColumn)).toHaveCount(62);
  expect(new Set(await cardIds(noneColumn)).size).toBe(62);

  // Drag from the unassigned second page into an option.
  const dragged = (await cardIds(noneColumn)).at(-1)!;
  await noneColumn.getByTestId(`collection-card-${dragged}`).dragTo(buildColumn, dragPoints);
  await expect(buildColumn.getByTestId(`collection-card-${dragged}`)).toBeVisible();
  await expect(cards(noneColumn)).toHaveCount(61);
  await expect.poll(async () => (await itemRow(dragged)).values[field.id]).toEqual({ options: [build.id] });

  // Keyboard alternative: the per-card select moves into an option and back to unassigned.
  const keyed = (await cardIds(noneColumn))[0]!;
  await noneColumn.getByLabel(`그룹 기준 · ${keyed}`, { exact: true }).selectOption({ label: "설계" });
  await expect(designColumn.getByTestId(`collection-card-${keyed}`)).toBeVisible();
  await expect.poll(async () => (await itemRow(keyed)).values[field.id]).toEqual({ options: [design.id] });
  await buildColumn.getByLabel(`그룹 기준 · ${dragged}`, { exact: true }).selectOption({ label: "미지정" });
  await expect(noneColumn.getByTestId(`collection-card-${dragged}`)).toBeVisible();
  await expect(buildColumn.getByText("현재 결과에 표시할 자료가 없습니다")).toBeVisible();
  await expect.poll(async () => (await itemRow(dragged)).values[field.id] ?? null).toBeNull();
  await expect(page.getByTestId(`collection-card-${dragged}`)).toHaveCount(1);

  // 5. Stale version (changed elsewhere): the move is rejected, the error stays visible
  //    and the board shows the server value instead of a false success.
  const stale = await itemRow(keyed);
  const elsewhere = await page.request.put(`${base}/collections/${collection.id}/items/${stale.id}/values`, {
    data: {
      fieldId: field.id,
      expectedVersion: stale.version,
      expectedFieldVersion: field.version,
      value: { options: [build.id] },
    },
  });
  expect(elsewhere.ok()).toBe(true);
  await designColumn.getByTestId(`collection-card-${keyed}`).dragTo(noneColumn, dragPoints);
  await expect(page.getByRole("alert").filter({ hasText: "다른 곳에서 먼저 수정되었습니다" })).toBeVisible();
  await expect(buildColumn.getByTestId(`collection-card-${keyed}`)).toBeVisible();
  await expect(noneColumn.getByTestId(`collection-card-${keyed}`)).toHaveCount(0);
  expect((await itemRow(keyed)).values[field.id]).toEqual({ options: [build.id] });

  // Reload: select grouping persisted server side.
  await page.reload();
  await page
    .locator('section[data-testid="collection-board"] > .collection-toolbar')
    .getByLabel("그룹 기준", { exact: true })
    .selectOption({ label: "단계" });
  await expect(buildColumn.getByTestId(`collection-card-${keyed}`)).toBeVisible();
  await expect(noneColumn.locator(".collection-board__head")).toContainText("61");

  // 6. Archived project: cards are read-only (no drag, no select) and a drag changes nothing.
  expect((await page.request.post(`${base}/projects/${project.id}/archive`)).ok()).toBe(true);
  await page.reload();
  await expect(cards(columnA)).toHaveCount(50);
  await expect(page.locator('[data-testid^="collection-card-"][draggable="true"]')).toHaveCount(0);
  await expect(page.getByLabel(/^그룹 기준 · BRD-/)).toHaveCount(0);
  const frozen = (await cardIds(columnA))[0]!;
  await columnA.getByTestId(`collection-card-${frozen}`).dragTo(columnB, dragPoints);
  await expect(columnA.getByTestId(`collection-card-${frozen}`)).toBeVisible();
  expect((await itemRow(frozen)).statusId).toBe(statusA!.id);
});

test("calendar moves previews and day-list rows to another day or unassigned through the API", async ({
  page,
}) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page);
  const slug = admin.workspaceSlug;
  const base = `/api/v1/workspaces/${wsId}`;
  const me = (await (await page.request.get("/api/v1/auth/me")).json()) as { timezone: string };
  const dayIn = (iso: string) => new Intl.DateTimeFormat("en-CA", { timeZone: me.timezone }).format(new Date(iso));

  const projectRes = await page.request.post(`${base}/projects`, {
    data: { key: "CAL", name: "달력 프로젝트", visibility: "workspace" },
  });
  expect(projectRes.status()).toBe(201);
  const project = (await projectRes.json()) as { id: string };
  const workflow = (await (await page.request.get(`${base}/projects/${project.id}/workflow`)).json()) as {
    statuses: { id: string }[];
  };
  async function createTask(title: string, dueDate: string | null): Promise<{ id: string; displayId: string }> {
    const res = await page.request.post(`${base}/projects/${project.id}/tasks`, {
      // Create rejects an explicit null date; omit the field for an undated task.
      data: { title, type: "task", statusId: workflow.statuses[0]!.id, ...(dueDate ? { dueDate } : {}) },
    });
    expect(res.status()).toBe(201);
    const body = (await res.json()) as { id: string; number: number };
    return { id: body.id, displayId: `CAL-${body.number}` };
  }
  const previewed = await createTask("미리보기 이동", "2027-05-10");
  const listed = await createTask("목록 이동", "2027-05-11");
  const timed = await createTask("시각 마감", null);
  const stale = await createTask("충돌 이동", "2027-05-12");
  expect(
    (await page.request.patch(`${base}/tasks/${timed.id}`, { data: { dueAt: "2027-05-05T03:00:00Z" } })).ok(),
  ).toBe(true);

  const collection = (await (
    await page.request.get(`${base}/projects/${project.id}/collection`)
  ).json()) as { id: string };
  async function createField(name: string, key: string, type: string) {
    const res = await page.request.post(`${base}/collections/${collection.id}/fields`, {
      data: { name, key, type },
    });
    expect(res.status()).toBe(201);
    return (await res.json()) as { id: string; version: number };
  }
  const dateField = await createField("기준일", "base_day", "date");
  const timeField = await createField("시각", "at_time", "datetime");

  type Row = {
    id: string;
    displayId: string;
    dueDate: string | null;
    dueAt: string | null;
    values: Record<string, unknown>;
    version: number;
  };
  async function itemRow(displayId: string): Promise<Row> {
    const res = await page.request.post(`${base}/collections/${collection.id}/query`, {
      data: { config: { query: { filters: {}, sort: [] }, groupBy: null, dateBy: null }, limit: 100 },
    });
    expect(res.ok()).toBe(true);
    const row = ((await res.json()) as { items: Row[] }).items.find((item) => item.displayId === displayId);
    expect(row).toBeTruthy();
    return row!;
  }
  async function putValue(displayId: string, fieldId: string, fieldVersion: number, value: unknown) {
    const row = await itemRow(displayId);
    const res = await page.request.put(`${base}/collections/${collection.id}/items/${row.id}/values`, {
      data: { fieldId, expectedVersion: row.version, expectedFieldVersion: fieldVersion, value },
    });
    expect(res.ok()).toBe(true);
  }
  const instant = "2027-05-08T14:30:00Z";
  await putValue(previewed.displayId, timeField.id, timeField.version, { datetime: instant });
  await putValue(listed.displayId, dateField.id, dateField.version, { date: "2027-05-03" });

  // A viewport tall enough for the month and the day list keeps real drags free of scrolling.
  await page.setViewportSize({ width: 1600, height: 2400 });
  const calendar = page.getByTestId("collection-calendar");
  const cell = (date: string) => calendar.locator(`td[data-date="${date}"]`);
  const preview = (displayId: string) => calendar.getByTestId(`collection-preview-${displayId}`);
  const unassigned = page.getByRole("button", { name: /^미지정 · \d+$/ });
  async function openMonth() {
    await page.goto(`/w/${slug}/CAL/calendar`);
    await page.locator('input[type="month"]').fill("2027-05");
    await expect(page.getByLabel("2027년 5월", { exact: true }).first()).toBeVisible();
  }
  async function dateBy(label: string) {
    await page.getByLabel("날짜 기준", { exact: true }).selectOption({ label });
  }

  // 1. Due basis: drag a preview to another day; dueDate changes and survives reload.
  await openMonth();
  await expect(cell("2027-05-10").getByTestId(`collection-preview-${previewed.displayId}`)).toBeVisible();
  await preview(previewed.displayId).dragTo(cell("2027-05-14"));
  await expect(cell("2027-05-14").getByTestId(`collection-preview-${previewed.displayId}`)).toBeVisible();
  await expect(cell("2027-05-10").getByTestId(`collection-preview-${previewed.displayId}`)).toHaveCount(0);
  await expect.poll(async () => (await itemRow(previewed.displayId)).dueDate).toBe("2027-05-14");

  // A timed due (dueAt) moves to a plain due date, as the source calendar does.
  await expect(cell(dayIn("2027-05-05T03:00:00Z")).getByTestId(`collection-preview-${timed.displayId}`)).toBeVisible();
  await preview(timed.displayId).dragTo(cell("2027-05-20"));
  await expect(cell("2027-05-20").getByTestId(`collection-preview-${timed.displayId}`)).toBeVisible();
  await expect
    .poll(async () => {
      const row = await itemRow(timed.displayId);
      return [row.dueDate, row.dueAt];
    })
    .toEqual(["2027-05-20", null]);

  // 2. Day list: drag a row onto the unassigned button, then from unassigned back onto a day.
  await calendar.getByRole("button", { name: "2027-05-11 · 전체 1개" }).click();
  await expect(page.getByRole("heading", { name: "2027-05-11 항목" })).toBeVisible();
  const listedHandle = page.getByTestId(`collection-drag-${listed.displayId}`);
  await expect(listedHandle).toHaveAttribute("draggable", "true");
  await listedHandle.dragTo(unassigned);
  await expect(page.getByTestId(`collection-row-${listed.displayId}`)).toHaveCount(0);
  await expect.poll(async () => (await itemRow(listed.displayId)).dueDate).toBeNull();
  await unassigned.click();
  await expect(page.getByRole("heading", { name: "미지정 항목" })).toBeVisible();
  await expect(page.getByTestId(`collection-row-${listed.displayId}`)).toBeVisible();
  // Real pointer drag (mouse down, move, up) from the list onto a day cell above it.
  const from = (await page.getByTestId(`collection-drag-${listed.displayId}`).boundingBox())!;
  const to = (await cell("2027-05-25").boundingBox())!;
  await page.mouse.move(from.x + 4, from.y + 4);
  await page.mouse.down();
  await page.mouse.move(from.x + 40, from.y + 4, { steps: 4 });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height - 8, { steps: 8 });
  await page.mouse.up();
  await expect(cell("2027-05-25").getByTestId(`collection-preview-${listed.displayId}`)).toBeVisible();
  await expect.poll(async () => (await itemRow(listed.displayId)).dueDate).toBe("2027-05-25");

  await page.reload();
  await page.locator('input[type="month"]').fill("2027-05");
  await expect(cell("2027-05-14").getByTestId(`collection-preview-${previewed.displayId}`)).toBeVisible();
  await expect(cell("2027-05-20").getByTestId(`collection-preview-${timed.displayId}`)).toBeVisible();
  await expect(cell("2027-05-25").getByTestId(`collection-preview-${listed.displayId}`)).toBeVisible();

  // 3. Stale dates (changed elsewhere): expectedDates rejects the move and the calendar
  //    shows the server value instead of a false success. The drag starts first so the
  //    dragged row is the pre-edit snapshot whether or not the task stream refreshes the
  //    calendar before the drop; the target's drop-over marker shows dragstart ran.
  await expect(cell("2027-05-12").getByTestId(`collection-preview-${stale.displayId}`)).toBeVisible();
  const staleFrom = (await preview(stale.displayId).boundingBox())!;
  const staleTo = (await cell("2027-05-15").boundingBox())!;
  await page.mouse.move(staleFrom.x + staleFrom.width / 2, staleFrom.y + staleFrom.height / 2);
  await page.mouse.down();
  await page.mouse.move(staleFrom.x + staleFrom.width / 2 + 40, staleFrom.y + staleFrom.height / 2, { steps: 4 });
  await page.mouse.move(staleTo.x + staleTo.width / 2, staleTo.y + staleTo.height - 8, { steps: 8 });
  await expect(cell("2027-05-15")).toHaveAttribute("data-drop-over", "true");
  expect(
    (await page.request.patch(`${base}/tasks/${stale.id}`, { data: { dueDate: "2027-05-13" } })).ok(),
  ).toBe(true);
  // The task stream refreshes the calendar mid-drag: the preview moves to 05-13, so the
  // drag-source element in 05-12 is unmounted before the drop.
  await expect(cell("2027-05-13").getByTestId(`collection-preview-${stale.displayId}`)).toBeVisible();
  await expect(cell("2027-05-12").getByTestId(`collection-preview-${stale.displayId}`)).toHaveCount(0);
  const staleMove = page.waitForResponse(
    (res) => res.request().method() === "PATCH" && new URL(res.url()).pathname === `${base}/tasks/${stale.id}`,
  );
  await page.mouse.move(staleTo.x + staleTo.width / 2 + 4, staleTo.y + staleTo.height - 8, { steps: 2 });
  await page.mouse.up();
  const staleResponse = await staleMove;
  expect(staleResponse.status()).toBe(409);
  expect(staleResponse.request().postDataJSON()).toMatchObject({
    dueDate: "2027-05-15",
    expectedDates: { dueDate: "2027-05-12" },
  });
  await expect(page.getByRole("alert").filter({ hasText: "다른 곳에서 먼저 수정되었습니다" })).toBeVisible();
  await expect(cell("2027-05-13").getByTestId(`collection-preview-${stale.displayId}`)).toBeVisible();
  await expect(cell("2027-05-15").getByTestId(`collection-preview-${stale.displayId}`)).toHaveCount(0);
  expect((await itemRow(stale.displayId)).dueDate).toBe("2027-05-13");

  // 4. Datetime field: the day changes, the wall time in the user's zone stays.
  await dateBy("시각");
  const sourceDay = dayIn(instant);
  await expect(cell(sourceDay).getByTestId(`collection-preview-${previewed.displayId}`)).toBeVisible();
  const targetDay = `2027-05-${String(Number(sourceDay.slice(8, 10)) + 2).padStart(2, "0")}`;
  await preview(previewed.displayId).dragTo(cell(targetDay));
  await expect(cell(targetDay).getByTestId(`collection-preview-${previewed.displayId}`)).toBeVisible();
  await expect
    .poll(async () => {
      const value = (await itemRow(previewed.displayId)).values[timeField.id] as { datetime: string } | undefined;
      return value ? Date.parse(value.datetime) : null;
    })
    .toBe(Date.parse(instant) + 2 * 86_400_000);

  // 5. Date field: stays a plain date; the keyboard editor in the day list still saves.
  await dateBy("기준일");
  await preview(listed.displayId).dragTo(cell("2027-05-04"));
  await expect(cell("2027-05-04").getByTestId(`collection-preview-${listed.displayId}`)).toBeVisible();
  await expect.poll(async () => (await itemRow(listed.displayId)).values[dateField.id]).toEqual({ date: "2027-05-04" });
  await calendar.getByRole("button", { name: "2027-05-04 · 전체 1개" }).click();
  const editor = page.getByLabel(`기준일 · ${listed.displayId}`, { exact: true });
  await editor.fill("2027-05-06");
  await page
    .getByTestId(`collection-row-${listed.displayId}`)
    .getByTestId("value-editor-base_day")
    .getByRole("button", { name: "저장" })
    .click();
  await expect.poll(async () => (await itemRow(listed.displayId)).values[dateField.id]).toEqual({ date: "2027-05-06" });
  await expect(cell("2027-05-06").getByTestId(`collection-preview-${listed.displayId}`)).toBeVisible();

  // 6. Archived project: previews and rows are read-only and a drag writes nothing.
  expect((await page.request.post(`${base}/projects/${project.id}/archive`)).ok()).toBe(true);
  await openMonth();
  await expect(preview(stale.displayId)).toBeVisible();
  await expect(calendar.locator('[data-testid^="collection-preview-"][draggable="true"]')).toHaveCount(0);
  await preview(stale.displayId).dragTo(cell("2027-05-16"));
  await expect(cell("2027-05-13").getByTestId(`collection-preview-${stale.displayId}`)).toBeVisible();
  expect((await itemRow(stale.displayId)).dueDate).toBe("2027-05-13");
  await calendar.getByRole("button", { name: "2027-05-13 · 전체 1개" }).click();
  await expect(page.getByTestId(`collection-row-${stale.displayId}`)).toBeVisible();
  await expect(page.locator('[data-testid^="collection-drag-"][draggable="true"]')).toHaveCount(0);
});
