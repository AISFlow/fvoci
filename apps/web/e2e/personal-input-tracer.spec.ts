import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { expect, test, type Page, type TestInfo } from "@playwright/test";
import { z } from "zod";
import { createE2eUser, login } from "./helpers";

const owner = { email: "Admin@Example.COM", password: "supersecret1" };
const resultSchema = z.object({
  documentId: z.string().uuid(),
  documentDisplayId: z.string(),
  taskId: z.string().uuid().nullable(),
  taskDisplayId: z.string().nullable(),
  projectId: z.string().uuid().nullable(),
  replayed: z.boolean(),
});
const workspaceSchema = z.object({ id: z.string().uuid(), slug: z.string() });
const listSchema = z
  .object({ items: z.array(z.object({ id: z.string(), title: z.string() }).passthrough()) })
  .passthrough();
const originSchema = z.object({
  count: z.number(),
  items: z.array(
    z
      .object({ taskId: z.string(), documentId: z.string(), anchor: z.string().nullable() })
      .passthrough(),
  ),
});
const backlinksSchema = z.object({
  items: z.array(
    z.object({
      id: z.string(),
      from: z.object({ id: z.string(), title: z.string() }).passthrough(),
    }),
  ),
});
const taskSchema = z
  .object({
    id: z.string(),
    title: z.string(),
    dueDate: z.string().nullable(),
    statusId: z.string(),
    assigneeIds: z.array(z.string()),
  })
  .passthrough();

// A genuinely new committed connection uses exactly the run's restricted app
// role. Credentials remain in the child environment and never enter evidence.
function committed(workspaceId: string, select: string): unknown {
  z.string().uuid().parse(workspaceId);
  const url = new URL(process.env.DATABASE_APP_URL ?? "");
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!container) throw new Error("isolated PostgreSQL container is required");
  try {
    const output = execFileSync(
      "docker",
      [
        "exec",
        "-i",
        "-e",
        `PGPASSWORD=${decodeURIComponent(url.password)}`,
        container,
        "psql",
        "-X",
        "-qAt",
        "-h",
        "127.0.0.1",
        "-U",
        decodeURIComponent(url.username),
        "-d",
        url.pathname.slice(1),
        "-v",
        "ON_ERROR_STOP=1",
      ],
      {
        input: `BEGIN; SET LOCAL app.tenant_id='${workspaceId}'; ${select}; ROLLBACK;`,
        stdio: "pipe",
        encoding: "utf8",
      },
    );
    return JSON.parse(output.trim()) as unknown;
  } catch {
    throw new Error("restricted committed DB observation failed (credentials omitted)");
  }
}
async function setup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if (!new URL(page.url()).pathname.endsWith("/setup")) {
    await login(page, owner.email, owner.password);
    return;
  }
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("입력");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Tracer team");
  await page.getByLabel("주소(영문)").fill("tracer");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}
async function servedAssets(page: Page, testInfo: TestInfo): Promise<void> {
  const selectors = ['script[type="module"][src]', 'link[rel="stylesheet"][href]'];
  const assets: { path: string; sha256: string }[] = [];
  for (const selector of selectors) {
    const attribute = selector.startsWith("script") ? "src" : "href";
    const value = await page.locator(selector).first().getAttribute(attribute);
    if (!value) throw new Error("missing production asset reference");
    const path = new URL(value, page.url()).pathname;
    expect(path.startsWith("/assets/")).toBe(true);
    const response = await page.request.get(path);
    expect(response.status()).toBe(200);
    const actual = createHash("sha256")
      .update(await response.body())
      .digest("hex");
    const expected = createHash("sha256")
      .update(readFileSync(new URL(`../dist${path}`, import.meta.url)))
      .digest("hex");
    expect(actual).toBe(expected);
    assets.push({ path, sha256: actual });
  }
  await testInfo.attach("actual-served-assets", {
    body: JSON.stringify({ url: page.url(), assets }),
    contentType: "application/json",
  });
}
async function saveBody(page: Page): Promise<void> {
  await page.getByRole("button", { name: "저장", exact: true }).first().click();
  await expect(page.locator('[data-collab-persisted="true"]').first()).toBeVisible();
}

test("private input survives lost response, keeps one source block/task UUID and synchronizes mounted MyTasks plus existing task views", async ({
  page,
  browser,
}, testInfo) => {
  test.setTimeout(90000);
  await setup(page);
  await page.goto("/w/tracer/wiki");
  await servedAssets(page, testInfo);
  await page.getByRole("button", { name: "개인 입력", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "개인 입력", exact: true });
  await expect(dialog.getByRole("radio", { name: "빠른 기록", exact: true })).toBeChecked();
  await dialog.getByRole("radio", { name: "메모", exact: true }).check();
  const token = `w2${String(Date.now())}`;
  await dialog.getByLabel("제목 또는 짧은 기록").fill(`${token} private note`);
  await dialog.screenshot({ path: testInfo.outputPath("private-default-dialog.png") });
  const originalViewport = page.viewportSize();
  await page.setViewportSize({ width: 320, height: 720 });
  for (const textScale of ["100%", "200%"]) {
    await page.evaluate((scale) => {
      document.documentElement.style.fontSize = scale;
    }, textScale);
    const bounds = await dialog.boundingBox();
    if (!bounds) throw new Error("personal input dialog missing at narrow viewport");
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(320);
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(
      true,
    );
    await expect(dialog.getByLabel("제목 또는 짧은 기록")).toHaveValue(`${token} private note`);
    await dialog.screenshot({
      path: testInfo.outputPath(`private-dialog-320-text-${textScale.slice(0, -1)}.png`),
    });
  }
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "";
  });
  if (originalViewport) await page.setViewportSize(originalViewport);
  await dialog.getByLabel("제목 또는 짧은 기록").focus();
  await page.keyboard.press("Tab");
  expect(await dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true);
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
  const opener = page.getByRole("button", { name: "개인 입력", exact: true });
  await expect(opener).toBeFocused();
  await opener.click();
  await expect(dialog.getByLabel("제목 또는 짧은 기록")).toHaveValue(`${token} private note`);

  let lost: z.infer<typeof resultSchema> | undefined;
  let originalCommand: unknown;
  await page.route("**/personal-input", async (route) => {
    originalCommand = route.request().postDataJSON() as unknown;
    const response = await route.fetch();
    expect(response.status()).toBe(201);
    lost = resultSchema.parse(await response.json());
    // Actual Rust transaction committed successfully; only its browser response
    // is lost. This is not a mocked happy-path result.
    await route.abort("failed");
  });
  await dialog.getByRole("button", { name: "개인 공간에 저장", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "같은 입력 다시 확인" })).toBeEnabled();
  await expect(dialog.getByRole("alert")).toBeVisible();
  expect(lost).toBeDefined();
  if (!lost) throw new Error("successful lost response was not observed");
  expect(lost.taskId).toBeNull();
  await page.unroute("**/personal-input");
  const retryResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-input") && response.request().method() === "POST",
  );
  await dialog.getByRole("button", { name: "같은 입력 다시 확인" }).click();
  const retried = await retryResponse;
  expect(retried.request().postDataJSON()).toEqual(originalCommand);
  const replay = resultSchema.parse(await retried.json());
  expect(replay.documentId).toBe(lost.documentId);
  expect(replay.replayed).toBe(true);
  await expect(dialog.getByText("개인 공간에 저장했습니다.")).toBeVisible();
  await dialog.getByRole("button", { name: "메모 열기", exact: true }).click();
  const personal = workspaceSchema.parse(
    await (await page.request.post("/api/v1/me/personal-workspace")).json(),
  );
  const documentId = replay.documentId;
  await expect(page).toHaveURL(`/w/${personal.slug}/${replay.documentDisplayId}`);
  const documentUrl = page.url();
  expect(new URL(documentUrl).pathname).toBe(`/w/${personal.slug}/${replay.documentDisplayId}`);
  const editor = page.locator(".fvoci-editor .ProseMirror").first();
  await expect(editor).toHaveAttribute("contenteditable", "true");
  const sourceText = `${token} 혼합 입력 🙂 원본 블록`;
  await editor.click();
  await page.keyboard.type(sourceText);
  await expect(page.locator('[data-collab-persisted="false"]').first()).toBeVisible();
  await saveBody(page);
  const paragraph = editor.locator(":scope > p").first();
  const anchor = await paragraph.getAttribute("data-id");
  expect(anchor).toBeTruthy();
  await paragraph.click();
  const origins = page.getByRole("region", { name: "연결 태스크" });
  await origins.getByRole("button", { name: "선택한 블록에서 할 일 만들기" }).click();
  await expect(origins.getByLabel("태스크 제목")).toHaveValue(sourceText);
  await expect(origins.getByText("나에게 할당하여 개인 할 일에 표시합니다.")).toBeVisible();
  const captureResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-input") && response.request().method() === "POST",
  );
  await origins.getByRole("button", { name: "연결 태스크 만들기", exact: true }).click();
  const captured = resultSchema.parse(await (await captureResponse).json());
  expect(captured.documentId).toBe(documentId);
  expect(captured.taskId).toBeTruthy();
  expect(captured.projectId).toBeTruthy();
  expect(captured.taskDisplayId).toBeTruthy();
  if (!captured.taskId || !captured.projectId || !captured.taskDisplayId)
    throw new Error("missing committed task identity");
  const taskId = captured.taskId,
    projectId = captured.projectId,
    displayId = captured.taskDisplayId;
  const listed = originSchema.parse(
    await (
      await page.request.get(
        `/api/v1/workspaces/${personal.id}/documents/${documentId}/task-origins`,
      )
    ).json(),
  );
  expect(listed.items).toHaveLength(1);
  expect(listed.items[0]).toMatchObject({ documentId, taskId, anchor });
  // Origin alone is not a mention backlink. Create and persist a real body
  // reference to the same task rather than synthesizing the backlink relation.
  const beforeBacklinks = await page.request.get(
    `/api/v1/workspaces/${personal.id}/tasks/${taskId}/backlinks`,
  );
  expect(beforeBacklinks.status()).toBe(200);
  expect(backlinksSchema.parse(await beforeBacklinks.json()).items).toHaveLength(0);
  await paragraph.click();
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await page.keyboard.type(`@${displayId}`);
  await page
    .locator(".fvoci-suggestion")
    .getByRole("option", { name: sourceText, exact: true })
    .click();
  await expect(editor.locator("[data-mention]")).toContainText(sourceText);
  await saveBody(page);
  await expect
    .poll(async () =>
      backlinksSchema
        .parse(
          await (
            await page.request.get(`/api/v1/workspaces/${personal.id}/tasks/${taskId}/backlinks`)
          ).json(),
        )
        .items.map((item) => item.from.id),
    )
    .toEqual([documentId]);
  const roles = z
    .object({
      superuser: z.boolean(),
      bypass: z.boolean(),
      forced: z.boolean(),
      documents: z.number(),
      tasks: z.number(),
      origins: z.number(),
      assignments: z.number(),
      anchor: z.string().nullable(),
    })
    .parse(
      committed(
        personal.id,
        `SELECT json_build_object(
    'superuser',(SELECT rolsuper FROM pg_roles WHERE rolname=current_user),'bypass',(SELECT rolbypassrls FROM pg_roles WHERE rolname=current_user),
    'forced',(SELECT relforcerowsecurity FROM pg_class WHERE oid='fvoci.personal_input_commands'::regclass),
    'documents',(SELECT count(*) FROM fvoci.documents WHERE title='${token} private note'), 'tasks',(SELECT count(*) FROM fvoci.tasks WHERE id='${taskId}'),
    'origins',(SELECT count(*) FROM fvoci.task_origins WHERE task_id='${taskId}'), 'assignments',(SELECT count(*) FROM fvoci.task_assignees WHERE task_id='${taskId}'),
    'anchor',(SELECT anchor FROM fvoci.task_origins WHERE task_id='${taskId}'))`,
      ),
    );
  expect(roles).toEqual({
    superuser: false,
    bypass: false,
    forced: true,
    documents: 1,
    tasks: 1,
    origins: 1,
    assignments: 1,
    anchor,
  });
  await testInfo.attach("committed-identity", {
    body: JSON.stringify({ ...captured, ...roles, sourceText }),
    contentType: "application/json",
  });
  await page.close();

  // Independent storage/cookies and login; no shared editor provider or cache.
  const fresh = await browser.newContext({ baseURL: testInfo.project.use.baseURL });
  const detail = await fresh.newPage();
  await login(detail, owner.email, owner.password);
  await detail.goto(documentUrl);
  await expect(detail.locator(".fvoci-editor .ProseMirror").first()).toContainText(sourceText);
  await expect(detail.locator(`[data-id="${anchor ?? ""}"]`).first()).toContainText(sourceText);
  await expect(detail.getByRole("region", { name: "연결 태스크" }).getByRole("link")).toContainText(
    displayId,
  );
  await detail.goto(`/w/${personal.slug}/${displayId}`);
  await expect(detail.getByTestId("task-edit-title")).toHaveValue(sourceText);
  const source = detail.getByRole("region", { name: "출처 문서" });
  await expect(source.getByRole("link")).toHaveAttribute(
    "href",
    new RegExp(`#block=${anchor ?? ""}$`),
  );
  // Keep MyTasks mounted during the three actual edits. Additional collection
  // views use one sequential page: HTTP/1 local Chrome has six host connections,
  // and each independent tab owns authorized access + task SSE streams.
  const mine = await fresh.newPage();
  await mine.goto(`/w/${personal.slug}/my-tasks`);
  await expect(mine.getByTestId(`my-task-${taskId}`)).toContainText(sourceText);
  const changedTitle = `${token} 한 번 수정`;
  const patchResponses: string[] = [];
  detail.on("response", (response) => {
    if (response.request().method() === "PATCH" && response.url().endsWith(`/tasks/${taskId}`))
      patchResponses.push(`PATCH:task:${String(response.status())}`);
    if (response.request().method() === "POST" && response.url().endsWith(`/tasks/${taskId}/move`))
      patchResponses.push(`POST:move:${String(response.status())}`);
  });
  await detail.getByTestId("task-edit-title").fill(changedTitle);
  await detail.getByTestId("task-edit-title").blur();
  await expect
    .poll(
      async () =>
        taskSchema.parse(
          await (
            await detail.request.get(`/api/v1/workspaces/${personal.id}/tasks/${taskId}`)
          ).json(),
        ).title,
    )
    .toBe(changedTitle);
  const due = new Date().toISOString().slice(0, 10);
  await detail.getByTestId("task-edit-due-date").fill(due);
  await detail.getByTestId("task-edit-due-date").blur();
  await expect
    .poll(
      async () =>
        taskSchema.parse(
          await (
            await detail.request.get(`/api/v1/workspaces/${personal.id}/tasks/${taskId}`)
          ).json(),
        ).dueDate,
    )
    .toBe(due);
  const workflow = z
    .object({ statuses: z.array(z.object({ id: z.string(), category: z.string() })) })
    .parse(
      await (
        await detail.request.get(`/api/v1/workspaces/${personal.id}/projects/${projectId}/workflow`)
      ).json(),
    );
  const statusOption = workflow.statuses.find((status) => status.category === "in_progress")?.id;
  if (!statusOption) throw new Error("missing non-final workflow status");
  expect(statusOption).not.toBe(await detail.getByTestId("task-edit-status").inputValue());
  await detail.getByTestId("task-edit-status").selectOption(statusOption);
  await expect
    .poll(
      async () =>
        taskSchema.parse(
          await (
            await detail.request.get(`/api/v1/workspaces/${personal.id}/tasks/${taskId}`)
          ).json(),
        ).statusId,
    )
    .toBe(statusOption);
  expect(patchResponses).toEqual(["PATCH:task:200", "PATCH:task:200", "POST:move:200"]);
  await expect(mine.getByTestId(`my-task-${taskId}`)).toContainText(changedTitle);
  await mine.close();
  const view = await fresh.newPage();
  await view.goto(`/w/${personal.slug}/INBOX/board`);
  await expect(view.getByTestId(`collection-card-${displayId}`)).toContainText(changedTitle);
  await view.goto(`/w/${personal.slug}/INBOX/calendar`);
  await expect(view.getByTestId(`collection-preview-${displayId}`)).toContainText(changedTitle);
  await view.goto(`/w/${personal.slug}/INBOX/gantt`);
  await expect(view.locator(`[data-task-id="${taskId}"]`).first()).toContainText(changedTitle);
  await view.goto(`/w/${personal.slug}/INBOX/tasks`);
  await expect(view.getByRole("link", { name: new RegExp(changedTitle) }).first()).toBeVisible();
  await view.close();
  await expect(detail.getByTestId("task-edit-title")).toHaveValue(changedTitle);
  const committedTask = committed(
    personal.id,
    `SELECT json_build_object('id',id,'title',title,'dueDate',due_date,'statusId',status_id) FROM fvoci.tasks WHERE id='${taskId}'`,
  );
  expect(committedTask).toEqual({
    id: taskId,
    title: changedTitle,
    dueDate: due,
    statusId: statusOption,
  });
  // Search convergence is the processed committed event plus permitted search
  // result, never an arbitrary cache TTL wait.
  await expect
    .poll(() =>
      committed(
        personal.id,
        `SELECT (count(DISTINCT target_id)=2 AND COALESCE(bool_and(fvoci.app_outbox_is_processed('search-index',id)),false))::text::json FROM fvoci.events WHERE target_id IN ('${taskId}','${documentId}') AND verb IN ('task.created','task.updated','document.created','document.updated')`,
      ),
    )
    .toBe(true);
  await expect
    .poll(async () => {
      const result = listSchema.parse(
        await (
          await detail.request.get(`/api/v1/workspaces/${personal.id}/search?q=${token}`)
        ).json(),
      );
      return (
        result.items.some((item) => item.id === taskId && item.title === changedTitle) &&
        result.items.some(
          (item) => item.id === documentId && item.title === `${token} private note`,
        )
      );
    })
    .toBe(true);
  const search = await fresh.newPage();
  await search.goto(`/w/${personal.slug}/search?q=${token}`);
  await expect(search.getByRole("link", { name: new RegExp(changedTitle) }).first()).toBeVisible();
  await expect(
    search.getByRole("link", { name: new RegExp(`${token} private note`) }).first(),
  ).toBeVisible();
  await testInfo.attach("single-edit-metadata", {
    body: JSON.stringify({
      taskId,
      changedTitle,
      due,
      statusOption,
      patchResponses,
      committedTask,
    }),
    contentType: "application/json",
  });

  createE2eUser("w2-outsider@example.com", "outsiderpass1", "외부");
  const outsiderContext = await browser.newContext({ baseURL: testInfo.project.use.baseURL });
  const outsider = await outsiderContext.newPage();
  await login(outsider, "w2-outsider@example.com", "outsiderpass1");
  for (const suffix of [
    `documents/${documentId}`,
    `documents/${documentId}/body`,
    `documents/${documentId}/task-origins`,
    `tasks/${taskId}`,
    `tasks/${taskId}/origin`,
    `tasks/${taskId}/backlinks`,
    `projects/${projectId}/stream`,
    `search?q=${token}`,
  ]) {
    const denied = await outsider.request.get(`/api/v1/workspaces/${personal.id}/${suffix}`);
    expect(denied.status()).toBe(404);
    const body = await denied.text();
    expect(body).not.toContain(sourceText);
    expect(body).not.toContain(anchor ?? "missing-anchor");
    expect(body).not.toContain(changedTitle);
  }
  const globalSearch = await outsider.request.get(`/api/v1/search?q=${token}`);
  expect(globalSearch.status()).toBe(200);
  expect(await globalSearch.text()).not.toContain(token);
  await outsiderContext.close();
  await fresh.close();
});

test("three actual editor hosts retain draft on Cancel, discard only on explicit route switch, and successful logout bypasses beforeunload", async ({
  page,
}, testInfo) => {
  await setup(page);
  const personal = workspaceSchema.parse(
    await (await page.request.post("/api/v1/me/personal-workspace")).json(),
  );
  const created = await page.request.post(`/api/v1/workspaces/${personal.id}/personal-input`, {
    data: { requestId: crypto.randomUUID(), intent: "task", title: "w2 guard source" },
  });
  expect(created.status()).toBe(201);
  const captured = resultSchema.parse(await created.json());
  if (!captured.taskId || !captured.projectId || !captured.taskDisplayId)
    throw new Error("missing task identity");
  const parentResponse = await page.request.post(
    `/api/v1/workspaces/${personal.id}/personal-input`,
    { data: { requestId: crypto.randomUUID(), intent: "task", title: "w2 guard parent" } },
  );
  expect(parentResponse.status()).toBe(201);
  const parent = resultSchema.parse(await parentResponse.json());
  if (!parent.taskId || !parent.taskDisplayId) throw new Error("missing parent identity");
  const hierarchy = await page.request.patch(
    `/api/v1/workspaces/${personal.id}/tasks/${captured.taskId}`,
    { data: { type: "subtask", parentId: parent.taskId } },
  );
  expect(hierarchy.status()).toBe(200);
  const project = z
    .object({ key: z.string(), rootDocumentId: z.string().uuid() })
    .parse(
      await (
        await page.request.get(`/api/v1/workspaces/${personal.id}/projects/${captured.projectId}`)
      ).json(),
    );
  const rootResponse = await page.request.get(
    `/api/v1/workspaces/${personal.id}/projects/${captured.projectId}/documents/${project.rootDocumentId}`,
  );
  expect(rootResponse.status()).toBe(200);
  const root = z.object({ number: z.number().int() }).parse(await rootResponse.json());
  const listPath = `/w/${personal.slug}/wiki`;
  const taskPath = `/w/${personal.slug}/${captured.taskDisplayId}`;
  const parentPath = `/w/${personal.slug}/${parent.taskDisplayId}`;
  const wikiPath = `/w/${personal.slug}/${captured.documentDisplayId}`;
  const targets = [
    {
      host: "wiki",
      path: wikiPath,
      body: `documents/${captured.documentId}/body`,
      leave: listPath,
    },
    { host: "task", path: taskPath, body: `tasks/${captured.taskId}`, leave: parentPath },
    {
      host: "project",
      path: `/w/${personal.slug}/${project.key}-${String(root.number)}`,
      body: `projects/${captured.projectId}/documents/${project.rootDocumentId}/body`,
      leave: listPath,
    },
  ];
  for (const target of targets) {
    await page.goto(target.path);
    await expect(page.locator(".fvoci-editor .ProseMirror").first()).toHaveAttribute(
      "contenteditable",
      "true",
    );
    await saveBody(page);
    const endpoint = `/api/v1/workspaces/${personal.id}/${target.body}`;
    const before = z
      .object({ contentJson: z.unknown() })
      .parse(await (await page.request.get(endpoint)).json()).contentJson;
    await page.locator('[data-editor-mode="markdown"]').click();
    const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
    await expect(field).toBeEditable();
    const draft = `${await field.inputValue()}\n\n${target.host} 보존할 한글 초안 🙂`;
    await field.fill(draft);
    await page.locator(`a[href="${target.leave}"]`).first().click();
    const warning = page.getByRole("dialog", { name: "Markdown 초안을 두고 이동할까요?" });
    await expect(warning).toBeVisible();
    await warning.getByRole("button", { name: "계속 편집", exact: true }).click();
    await expect(page).toHaveURL(target.path);
    await expect(field).toHaveValue(draft);
    expect(
      z.object({ contentJson: z.unknown() }).parse(await (await page.request.get(endpoint)).json())
        .contentJson,
    ).toEqual(before);
    await page.locator(`a[href="${target.leave}"]`).first().click();
    await expect(warning).toBeVisible();
    await warning.getByRole("button", { name: "초안 버리고 이동", exact: true }).click();
    await expect(page).toHaveURL(target.leave);
    expect(
      z.object({ contentJson: z.unknown() }).parse(await (await page.request.get(endpoint)).json())
        .contentJson,
    ).toEqual(before);
    await testInfo.attach(`${target.host}-draft-retained`, {
      body: JSON.stringify({ host: target.host, before, published: false }),
      contentType: "application/json",
    });
  }
  await page.goto(wikiPath);
  await expect(page.locator(".fvoci-editor .ProseMirror").first()).toHaveAttribute(
    "contenteditable",
    "true",
  );
  await saveBody(page);
  await page.locator('[data-editor-mode="markdown"]').click();
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  const draft = `${await field.inputValue()}\n\n로그아웃 실패에서 유지할 초안`;
  await field.fill(draft);
  // Actual Rust Origin rejection leaves the session live, rather than a mocked
  // successful logout. A refused logout must retain both draft and protection.
  await page.route("**/api/v1/auth/logout", async (route) => {
    await route.continue({
      headers: { ...route.request().headers(), origin: "https://not-this-instance.invalid" },
    });
  });
  const refused = page.waitForResponse((response) =>
    response.url().endsWith("/api/v1/auth/logout"),
  );
  await page.getByRole("button", { name: "로그아웃", exact: true }).click();
  expect((await refused).status()).toBe(403);
  await expect(field).toHaveValue(draft);
  await expect(page).toHaveURL(wikiPath);
  expect((await page.request.get("/api/v1/me")).status()).toBe(200);
  await page.unroute("**/api/v1/auth/logout");
  const nativeDialogs: string[] = [];
  page.on("dialog", async (dialog) => {
    nativeDialogs.push(dialog.type());
    await dialog.dismiss();
  });
  const ended = page.waitForResponse((response) => response.url().endsWith("/api/v1/auth/logout"));
  await page.getByRole("button", { name: "로그아웃", exact: true }).click();
  expect((await ended).status()).toBe(204);
  await expect(page).toHaveURL(/\/login$/);
  expect(nativeDialogs).toEqual([]);
});

test("51 real origins recover a missed hint without detail and preserve an in-flight next page", async ({
  page,
}, testInfo) => {
  await setup(page);
  const personal = workspaceSchema.parse(
    await (await page.request.post("/api/v1/me/personal-workspace")).json(),
  );
  const input = `/api/v1/workspaces/${personal.id}/personal-input`;
  const noteResponse = await page.request.post(input, {
    data: { requestId: crypto.randomUUID(), intent: "note", title: "Paged origin recovery source" },
  });
  expect(noteResponse.status()).toBe(201);
  const note = resultSchema.parse(await noteResponse.json());
  const rows: z.infer<typeof resultSchema>[] = [];
  for (let start = 0; start < 51; start += 3) {
    const group = await Promise.all(
      Array.from({ length: Math.min(3, 51 - start) }, async (_, index) => {
        const response = await page.request.post(input, {
          data: {
            requestId: crypto.randomUUID(),
            intent: "task",
            title: `Paged relation ${String(start + index)}`,
            source: { documentId: note.documentId },
          },
        });
        expect(response.status()).toBe(201);
        return resultSchema.parse(await response.json());
      }),
    );
    rows.push(...group);
  }
  expect(new Set(rows.map((row) => row.taskId)).size).toBe(51);
  const listPath = `/api/v1/workspaces/${personal.id}/documents/${note.documentId}/task-origins`;
  const pagedSchema = originSchema.extend({
    nextCursor: z.string().nullable(),
    items: z.array(
      z.object({
        taskId: z.string().uuid(),
        documentId: z.string().uuid(),
        taskTitle: z.string(),
        anchor: z.string().nullable(),
      }),
    ),
  });
  const first = pagedSchema.parse(await (await page.request.get(`${listPath}?limit=50`)).json());
  expect(first.count).toBe(51);
  expect(first.items).toHaveLength(50);
  expect(first.nextCursor).toBeTruthy();
  if (!first.nextCursor) throw new Error("missing genuine next-page cursor");
  const after = first.nextCursor;
  const next = pagedSchema.parse(
    await (await page.request.get(`${listPath}?limit=50&after=${after}`)).json(),
  );
  expect(next.count).toBe(51);
  expect(next.items).toHaveLength(1);
  const firstTask = first.items[0],
    nextTask = next.items[0];
  if (!firstTask || !nextTask) throw new Error("missing real paged rows");
  // Observe genuine native events without replacing their data or handlers.
  await page.addInitScript(() => {
    const observed = window as typeof window & { __w2Hints: string[] };
    observed.__w2Hints = [];
    const Native = window.EventSource;
    window.EventSource = class extends Native {
      constructor(url: string | URL, options?: EventSourceInit) {
        super(url, options);
        this.addEventListener("task", (event) => {
          if (event instanceof MessageEvent && typeof event.data === "string")
            observed.__w2Hints.push(event.data);
        });
      }
    };
  });
  const detailReads: string[] = [];
  page.on("request", (request) => {
    if (
      request.method() === "GET" &&
      /\/tasks\/[0-9a-f-]{36}$/.test(new URL(request.url()).pathname)
    )
      detailReads.push(request.url());
  });
  let refused = true,
    refusedCount = 0;
  await page.route("**/projects/*/stream", (route) => {
    if (refused) {
      refusedCount++;
      return route.fulfill({ status: 503, body: "transport unavailable" });
    }
    return route.continue();
  });
  await page.goto(`/w/${personal.slug}/${note.documentDisplayId}`);
  const panel = page.getByRole("region", { name: "연결 태스크" });
  await expect(panel.getByRole("heading")).toHaveText("연결 태스크 (51)");
  const mounted = await panel.elementHandle();
  await panel.getByRole("button", { name: "다음 연결 보기", exact: true }).click();
  await expect(panel.getByRole("link")).toHaveCount(1);
  await expect(panel.getByRole("link")).toContainText(nextTask.taskTitle);
  await expect.poll(() => refusedCount).toBeGreaterThan(0);
  expect(detailReads).toEqual([]);
  const taskPath = (id: string) => `/api/v1/workspaces/${personal.id}/tasks/${id}`;
  const renamed = "Peer rename while stream refused";
  const rename = await page.request.patch(taskPath(nextTask.taskId), { data: { title: renamed } });
  expect(rename.status()).toBe(200);
  const renamedRow = z
    .object({ id: z.string().uuid(), title: z.string() })
    .parse(await rename.json());
  expect(renamedRow).toEqual({ id: nextTask.taskId, title: renamed });
  expect(
    committed(
      personal.id,
      `SELECT to_jsonb(title) FROM fvoci.tasks WHERE workspace_id='${personal.id}' AND id='${nextTask.taskId}'`,
    ),
  ).toBe(renamed);
  await expect(panel.getByRole("link")).toContainText(nextTask.taskTitle);
  const opened = page.waitForResponse(
    (response) => response.url().endsWith("/stream") && response.status() === 200,
  );
  const recovered = page.waitForResponse(
    async (response) =>
      new URL(response.url()).pathname === listPath &&
      new URL(response.url()).searchParams.get("after") === after &&
      response.status() === 200 &&
      pagedSchema
        .parse(await response.json())
        .items.some((row) => row.taskId === nextTask.taskId && row.taskTitle === renamed),
  );
  refused = false;
  await opened;
  await recovered;
  await expect(panel.getByRole("link")).toContainText(renamed);
  expect(await mounted.evaluate((element) => element.isConnected)).toBe(true);
  expect(detailReads).toEqual([]);
  await panel.screenshot({ path: testInfo.outputPath("origins-missed-hint-recovered.png") });
  await panel.getByRole("button", { name: "처음 연결 보기", exact: true }).click();
  await expect(panel.getByRole("link")).toHaveCount(50);
  const hintSeen = async (id: string) => {
    const values = await page.evaluate(
      () => (window as typeof window & { __w2Hints: string[] }).__w2Hints,
    );
    return values.some(
      (raw) => z.object({ taskId: z.string() }).parse(JSON.parse(raw)).taskId === id,
    );
  };
  const firstRename = "First-page peer rename before load-more";
  expect(
    (
      await page.request.patch(taskPath(firstTask.taskId), { data: { title: firstRename } })
    ).status(),
  ).toBe(200);
  await expect.poll(() => hintSeen(firstTask.taskId)).toBe(true);
  await expect(panel.getByRole("link", { name: new RegExp(firstRename) })).toBeVisible();
  let release: () => void = () => {
    throw new Error("unbound held real response");
  };
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let resolveHeld: (value: z.infer<typeof pagedSchema>) => void = () => {
    throw new Error("unbound observed response");
  };
  const held = new Promise<z.infer<typeof pagedSchema>>((resolve) => {
    resolveHeld = resolve;
  });
  let holdOnce = true;
  await page.route(`**/documents/${note.documentId}/task-origins?*`, async (route) => {
    if (!holdOnce || new URL(route.request().url()).searchParams.get("after") !== after)
      return route.continue();
    holdOnce = false;
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    resolveHeld(pagedSchema.parse(await response.json()));
    await gate;
    await route.fulfill({ response });
  });
  try {
    await panel.getByRole("button", { name: "다음 연결 보기", exact: true }).click();
    const original = await held;
    expect(original.count).toBe(51);
    expect(original.items).toHaveLength(1);
    await expect(panel.getByRole("link")).toContainText(renamed);
    const refreshed = page.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === listPath &&
        new URL(response.url()).searchParams.get("after") === after &&
        response.status() === 200 &&
        pagedSchema.parse(await response.json()).count === 50,
    );
    // Clear observer evidence so a real deletion hint, not the earlier rename,
    // establishes the in-flight serialization barrier before releasing old200.
    await page.evaluate(() => {
      (window as typeof window & { __w2Hints: string[] }).__w2Hints = [];
    });
    const removed = await page.request.post(`${taskPath(firstTask.taskId)}/trash`);
    expect(removed.status()).toBe(200);
    expect(
      committed(
        personal.id,
        `SELECT to_jsonb(deleted_at IS NOT NULL) FROM fvoci.tasks WHERE workspace_id='${personal.id}' AND id='${firstTask.taskId}'`,
      ),
    ).toBe(true);
    await expect.poll(() => hintSeen(firstTask.taskId)).toBe(true);
    await expect(panel.getByRole("heading")).toHaveText("연결 태스크 (51)");
    release();
    await refreshed;
    await expect(panel.getByRole("heading")).toHaveText("연결 태스크 (50)");
    await expect(panel.getByRole("link")).toContainText(renamed);
    await panel.screenshot({ path: testInfo.outputPath("origins-next-page-after-delete.png") });
    await panel.getByRole("button", { name: "처음 연결 보기", exact: true }).click();
    await expect(panel.getByRole("heading")).toHaveText("연결 태스크 (50)");
    await expect(panel.getByRole("link", { name: new RegExp(firstRename) })).toHaveCount(0);
    expect(await mounted.evaluate((element) => element.isConnected)).toBe(true);
    expect(detailReads).toEqual([]);
    await testInfo.attach("actual-origin-recovery", {
      body: JSON.stringify({
        source: note.documentId,
        firstTask: firstTask.taskId,
        nextTask: nextTask.taskId,
        cursor: after,
        oldCount: original.count,
        newCount: 50,
        refusedCount,
        detailReads,
      }),
      contentType: "application/json",
    });
  } finally {
    release();
  }
});
