import { execFileSync } from "node:child_process";
import { z } from "zod";
import { expect, test, type Page } from "@playwright/test";
import { createE2eUser } from "./helpers";
import { admin, newSignedInPage, setupInstance, workspaceId } from "./workspace-wiki-vue-editor";

const taskSchema = z
  .object({
    id: z.string().uuid(),
    number: z.number(),
    version: z.number(),
    startDate: z.string().nullable(),
    dueDate: z.string().nullable(),
    dueAt: z.string().nullable(),
    title: z.string(),
    statusId: z.string(),
  })
  .passthrough();
const projectSchema = z.object({ id: z.string().uuid() }).passthrough();
const workflowSchema = z.object({ statuses: z.array(z.object({ id: z.string().uuid() })) });
const problemSchema = z.object({ code: z.string() }).passthrough();
test.describe.configure({ mode: "serial" });
test.beforeAll(async ({ browser, baseURL }) => setupInstance(browser, baseURL));

async function fixture(page: Page, key: string, dates: object) {
  const ws = await workspaceId(page.request);
  const base = `/api/v1/workspaces/${ws}`;
  const projectRes = await page.request.post(`${base}/projects`, {
    data: { key, name: key, visibility: "workspace" },
  });
  expect(projectRes.status()).toBe(201);
  const project = projectSchema.parse(await projectRes.json());
  const workflow = workflowSchema.parse(
    await (await page.request.get(`${base}/projects/${project.id}/workflow`)).json(),
  );
  const status = workflow.statuses[0];
  if (!status) throw new Error("Fixture requires workflow status");
  const created = await page.request.post(`${base}/projects/${project.id}/tasks`, {
    data: { title: `${key} task`, statusId: status.id, type: "task", ...dates },
  });
  expect(created.status()).toBe(201);
  const task = taskSchema.parse(await created.json());
  const endpoint = `${base}/tasks/${task.id}`;
  const stored = async () => {
    const response = await page.request.get(endpoint);
    expect(response.status()).toBe(200);
    return taskSchema.parse(await response.json());
  };
  return {
    base,
    projectId: project.id,
    task,
    endpoint,
    stored,
    path: `/w/${admin.workspaceSlug}/${key}`,
    displayId: `${key}-${String(task.number)}`,
  };
}

function datesOf(task: z.infer<typeof taskSchema>) {
  return { startDate: task.startDate, dueDate: task.dueDate, dueAt: task.dueAt };
}

test("real PATCH omission/null/date/version and 409 preserve the committed row", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  const page = signed.page;
  try {
    const f = await fixture(page, "TPC", { startDate: "2027-03-12", dueDate: "2027-03-13" });
    const before = await f.stored();
    let response = await page.request.patch(f.endpoint, { data: { title: "Metadata only" } });
    expect(response.status()).toBe(200);
    const renamed = taskSchema.parse(await response.json());
    expect(datesOf(renamed)).toEqual(datesOf(before));
    // `version` is the body version, not a metadata optimistic lock counter.
    expect(renamed.version).toBe(before.version);
    response = await page.request.patch(f.endpoint, {
      data: { dueDate: "", expectedDates: datesOf(renamed) },
    });
    expect(response.status()).toBe(400);
    expect(datesOf(await f.stored())).toEqual(datesOf(renamed));
    response = await page.request.patch(f.endpoint, {
      data: { dueDate: null, dueAt: "2027-03-14T13:30:00Z", expectedDates: datesOf(renamed) },
    });
    expect(response.status()).toBe(200);
    const timed = taskSchema.parse(await response.json());
    expect(datesOf(timed)).toEqual({
      startDate: "2027-03-12",
      dueDate: null,
      dueAt: "2027-03-14T13:30:00Z",
    });
    response = await page.request.patch(f.endpoint, {
      data: { dueDate: "2027-03-15", dueAt: null, expectedDates: datesOf(renamed) },
    });
    expect(response.status()).toBe(409);
    expect(problemSchema.parse(await response.json()).code).toBe("document_version_mismatch");
    expect(datesOf(await f.stored())).toEqual(datesOf(timed));
    const database = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
    if (!database) throw new Error("Actual DB tracer requires wrapper-owned database");
    const sql = `SELECT json_build_object('startDate',start_date,'dueDate',due_date,'dueAt',due_at,'version',version) FROM fvoci.tasks WHERE id='${f.task.id}'::uuid`;
    const row = JSON.parse(
      execFileSync(
        "docker",
        [
          "exec",
          process.env.FVOCI_TEST_PG_CONTAINER ?? "",
          "psql",
          "-U",
          "postgres",
          "-d",
          new URL(database).pathname.slice(1),
          "-X",
          "-At",
          "-v",
          "ON_ERROR_STOP=1",
          "-c",
          sql,
        ],
        {
          encoding: "utf8",
        },
      ),
    ) as { startDate: string; dueDate: null; dueAt: string; version: number };
    expect(row.startDate).toBe(timed.startDate);
    expect(row.dueDate).toBeNull();
    expect(Date.parse(row.dueAt)).toBe(Date.parse(timed.dueAt ?? ""));
    expect(row.version).toBe(timed.version);
  } finally {
    await signed.context.close();
  }
});

for (const returnToOrigin of [false, true]) {
  test(`a late metadata conflict retires across ${returnToOrigin ? "A-B-A" : "A-B"} task selection`, async ({
    browser,
    baseURL,
  }) => {
    const signed = await newSignedInPage(browser, baseURL, admin);
    const page = signed.page;
    let release = () => {};
    try {
      const key = returnToOrigin ? "TLA" : "TLB";
      const f = await fixture(page, key, { dueDate: "2027-03-13" });
      const created = await page.request.post(`${f.base}/projects/${f.projectId}/tasks`, {
        data: {
          title: "Selected parent",
          type: "epic",
          statusId: f.task.statusId,
          dueDate: "2027-03-10",
        },
      });
      expect(created.status()).toBe(201);
      const parent = taskSchema.parse(await created.json());
      expect(
        (await page.request.patch(f.endpoint, { data: { parentId: parent.id } })).status(),
      ).toBe(200);
      await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
      const draft = page.getByTestId("task-edit-due-date");
      await expect(draft).toHaveValue("2027-03-13");
      await draft.fill("2027-03-20");
      const refreshed = page.waitForResponse(
        async (r) =>
          r.url().endsWith(f.endpoint) &&
          r.request().method() === "GET" &&
          r.status() === 200 &&
          taskSchema.parse(await r.json()).dueDate === "2027-03-17",
      );
      expect(
        (await page.request.patch(f.endpoint, { data: { dueDate: "2027-03-17" } })).status(),
      ).toBe(200);
      expect(taskSchema.parse(await (await refreshed).json()).dueDate).toBe("2027-03-17");
      const gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      let serverAnswered = () => {};
      const answered = new Promise<void>((resolve) => {
        serverAnswered = resolve;
      });
      await page.route(`**${f.endpoint}`, async (route) => {
        if (route.request().method() !== "PATCH") return route.continue();
        const response = await route.fetch();
        serverAnswered();
        await gate;
        await route.fulfill({ response });
      });
      const late = page.waitForResponse(
        (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
      );
      await draft.blur();
      await answered;
      await page
        .locator(`a[href="/w/${admin.workspaceSlug}/${key}-${String(parent.number)}"]`)
        .click();
      await expect(
        page.getByRole("heading", { name: "Selected parent", exact: true }),
      ).toBeVisible();
      await expect(draft).toHaveValue("2027-03-10");
      if (returnToOrigin) {
        await page
          .locator(".task-home__crumb")
          .getByRole("link", { name: key, exact: true })
          .click();
        await page.getByTestId(`task-row-${f.task.id}`).click();
        await expect(draft).toHaveValue("2027-03-17");
      }
      release();
      expect((await late).status()).toBe(409);
      await expect(draft).toBeEnabled();
      await expect(page.getByRole("alert")).toHaveCount(0);
      await expect(draft).toHaveValue(returnToOrigin ? "2027-03-17" : "2027-03-10");
      expect((await f.stored()).dueDate).toBe("2027-03-17");
    } finally {
      release();
      await signed.context.close();
    }
  });
}

test("conflict recovery finishing after task navigation preserves the selected draft", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  const page = signed.page;
  let release = () => {};
  try {
    const f = await fixture(page, "TCR", { dueDate: "2027-03-13" });
    const created = await page.request.post(`${f.base}/projects/${f.projectId}/tasks`, {
      data: { title: "Recovery parent", type: "epic", statusId: f.task.statusId },
    });
    expect(created.status()).toBe(201);
    const parent = taskSchema.parse(await created.json());
    expect((await page.request.patch(f.endpoint, { data: { parentId: parent.id } })).status()).toBe(
      200,
    );
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    const draft = page.getByTestId("task-edit-due-date");
    await expect(draft).toHaveValue("2027-03-13");
    await draft.fill("2027-03-20");
    const refreshed = page.waitForResponse(
      async (r) =>
        r.url().endsWith(f.endpoint) &&
        r.request().method() === "GET" &&
        r.status() === 200 &&
        taskSchema.parse(await r.json()).dueDate === "2027-03-17",
    );
    expect(
      (await page.request.patch(f.endpoint, { data: { dueDate: "2027-03-17" } })).status(),
    ).toBe(200);
    expect(taskSchema.parse(await (await refreshed).json()).dueDate).toBe("2027-03-17");
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let serverAnswered = () => {};
    const answered = new Promise<void>((resolve) => {
      serverAnswered = resolve;
    });
    await page.route(`**${f.endpoint}`, async (route) => {
      if (route.request().method() !== "GET") return route.continue();
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      serverAnswered();
      await gate;
      await route.fulfill({ response });
    });
    const conflict = page.waitForResponse(
      (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
    );
    await draft.blur();
    expect((await conflict).status()).toBe(409);
    await answered; // Recovery has begun and awaits a real Rust GET response.
    await page.locator(`a[href="/w/${admin.workspaceSlug}/TCR-${String(parent.number)}"]`).click();
    await expect(page.getByRole("heading", { name: "Recovery parent", exact: true })).toBeVisible();
    const title = page.getByTestId("task-edit-title");
    await title.fill("Unsaved selected draft");
    await expect(title).toHaveValue("Unsaved selected draft");
    const recovery = page.waitForResponse(
      (r) => r.url().endsWith(f.endpoint) && r.request().method() === "GET",
    );
    release();
    expect(taskSchema.parse(await (await recovery).json()).dueDate).toBe("2027-03-17");
    // Let the response completion and Vue DOM flush run before checking the draft.
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() =>
            requestAnimationFrame(() => {
              resolve();
            }),
          ),
        ),
    );
    await expect(title).toHaveValue("Unsaved selected draft");
    const storedParent = await page.request.get(`${f.base}/tasks/${parent.id}`);
    expect(storedParent.status()).toBe(200);
    expect(taskSchema.parse(await storedParent.json()).title).toBe("Recovery parent");
  } finally {
    release();
    await signed.context.close();
  }
});

test("a late clone completion preserves subsequent task navigation", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  const page = signed.page;
  let release = () => {};
  try {
    const f = await fixture(page, "TCL", {});
    const created = await page.request.post(`${f.base}/projects/${f.projectId}/tasks`, {
      data: { title: "Clone parent", type: "epic", statusId: f.task.statusId },
    });
    expect(created.status()).toBe(201);
    const parent = taskSchema.parse(await created.json());
    expect((await page.request.patch(f.endpoint, { data: { parentId: parent.id } })).status()).toBe(
      200,
    );
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    await expect(page.getByTestId("task-clone")).toBeEnabled();
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let serverAnswered = () => {};
    const answered = new Promise<void>((resolve) => {
      serverAnswered = resolve;
    });
    await page.route(`**${f.endpoint}/clone`, async (route) => {
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      serverAnswered();
      await gate;
      await route.fulfill({ response });
    });
    const cloned = page.waitForResponse((r) => r.url().endsWith(`${f.endpoint}/clone`));
    await page.getByTestId("task-clone").click();
    await answered;
    const selectedPath = `/w/${admin.workspaceSlug}/TCL-${String(parent.number)}`;
    await page.locator(`a[href="${selectedPath}"]`).click();
    await expect(page.getByRole("heading", { name: "Clone parent", exact: true })).toBeVisible();
    const title = page.getByTestId("task-edit-title");
    release();
    const response = await cloned;
    expect(response.status()).toBe(200);
    const clone = taskSchema.parse(await response.json());
    const cloneRead = await page.request.get(`${f.base}/tasks/${clone.id}`);
    expect(cloneRead.status()).toBe(200);
    expect(taskSchema.parse(await cloneRead.json()).id).toBe(clone.id);
    await expect(page.getByTestId("task-clone")).toBeEnabled();
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() =>
            requestAnimationFrame(() => {
              resolve();
            }),
          ),
        ),
    );
    await expect(page).toHaveURL(new RegExp(`${selectedPath}$`));
    await expect(title).toHaveValue("Clone parent");
    await expect(page.getByRole("alert")).toHaveCount(0);
    const parentRead = await page.request.get(`${f.base}/tasks/${parent.id}`);
    expect(taskSchema.parse(await parentRead.json()).title).toBe("Clone parent");
    // A current operation still completes and navigates to its accepted clone.
    const currentClone = page.waitForResponse((r) =>
      r.url().endsWith(`${f.base}/tasks/${parent.id}/clone`),
    );
    await page.getByTestId("task-clone").click();
    const currentResponse = await currentClone;
    expect(currentResponse.status()).toBe(200);
    const current = taskSchema.parse(await currentResponse.json());
    await expect(page).toHaveURL(
      new RegExp(`/w/${admin.workspaceSlug}/TCL-${String(current.number)}$`),
    );
    await expect(page.getByRole("heading", { name: current.title, exact: true })).toBeVisible();
  } finally {
    release();
    await signed.context.close();
  }
});

for (const archivedTarget of [false, true]) {
  test(`a late dependency ${archivedTarget ? "409" : "400"} stays with its originating task and ordinary edges still work`, async ({
    browser,
    baseURL,
  }) => {
    const signed = await newSignedInPage(browser, baseURL, admin);
    const page = signed.page;
    let release = () => {};
    try {
      const key = archivedTarget ? "TDA" : "TDP";
      const f = await fixture(page, key, {});
      const created = await page.request.post(`${f.base}/projects/${f.projectId}/tasks`, {
        data: { title: "Dependency parent", type: "epic", statusId: f.task.statusId },
      });
      expect(created.status()).toBe(201);
      const parent = taskSchema.parse(await created.json());
      expect(
        (await page.request.patch(f.endpoint, { data: { parentId: parent.id } })).status(),
      ).toBe(200);
      const parentEndpoint = `${f.base}/tasks/${parent.id}`;
      expect(
        (
          await page.request.post(`${parentEndpoint}/dependencies`, {
            data: { blockedId: f.task.id, type: "FS", lagDays: 0 },
          })
        ).status(),
      ).toBe(200);
      await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
      await page.getByTestId("task-edit-dependency-open").click();
      await page.getByTestId("task-edit-dependency-target").selectOption(parent.id);
      if (archivedTarget) {
        expect(
          (await page.request.patch(parentEndpoint, { data: { archived: true } })).status(),
        ).toBe(200);
      }
      const gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      let serverAnswered = () => {};
      const answered = new Promise<void>((resolve) => {
        serverAnswered = resolve;
      });
      await page.route(`**${f.endpoint}/dependencies`, async (route) => {
        const response = await route.fetch();
        expect(response.status()).toBe(archivedTarget ? 409 : 400);
        expect(problemSchema.parse(await response.json()).code).toBe(
          archivedTarget ? "task_archived" : "dependency_cycle",
        );
        serverAnswered();
        await gate;
        await route.fulfill({ response });
      });
      const failed = page.waitForResponse((r) => r.url().endsWith(`${f.endpoint}/dependencies`));
      await page.getByTestId("task-edit-dependency-add").click();
      await answered;
      await page
        .locator(`a[href="/w/${admin.workspaceSlug}/${key}-${String(parent.number)}"]`)
        .click();
      await expect(
        page.getByRole("heading", { name: "Dependency parent", exact: true }),
      ).toBeVisible();
      release();
      expect((await failed).status()).toBe(archivedTarget ? 409 : 400);
      await page.evaluate(
        () =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() =>
              requestAnimationFrame(() => {
                resolve();
              }),
            ),
          ),
      );
      await expect(page.getByRole("alert")).toHaveCount(0);
      if (archivedTarget) {
        await expect(page.getByTestId("task-edit-title")).toBeDisabled();
        await expect(page.getByTestId("task-edit-dependency-open")).toHaveCount(0);
        const retainedUrl = page.url();
        const timeOrigin = await page.evaluate(() => performance.timeOrigin);
        const body = page.getByTestId("task-body");
        await expect(body.locator('[data-collab-status="connected"]')).toBeVisible();
        await expect(body.locator(".ProseMirror")).toHaveAttribute("contenteditable", "false");
        expect(
          (await page.request.patch(parentEndpoint, { data: { archived: false } })).status(),
        ).toBe(200);
        const restored = await page.request.get(parentEndpoint);
        expect(restored.status()).toBe(200);
        expect(
          z.object({ archivedAt: z.null(), canEdit: z.literal(true) }).parse(await restored.json())
            .archivedAt,
        ).toBeNull();
        await expect(page.getByTestId("task-edit-title")).toBeEnabled();
        await expect(page).toHaveURL(retainedUrl);
        expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin);
        // HTTP restore does not promote the already admitted readonly body lease.
        await expect(body.locator(".ProseMirror")).toHaveAttribute("contenteditable", "false");
        await expect(body.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
      }
      const removed = page.waitForResponse(
        (r) =>
          r.url().endsWith(`${parentEndpoint}/dependencies/${f.task.id}`) &&
          r.request().method() === "DELETE",
      );
      await page.getByTestId(`task-edit-dependency-remove-${f.task.id}`).click();
      expect((await removed).status()).toBe(200);
      await expect(page.getByTestId(`task-edit-dependency-${parent.id}-${f.task.id}`)).toHaveCount(
        0,
      );
      await page.getByTestId("task-edit-dependency-open").click();
      await page.getByTestId("task-edit-dependency-target").selectOption(f.task.id);
      const added = page.waitForResponse(
        (r) =>
          r.url().endsWith(`${parentEndpoint}/dependencies`) && r.request().method() === "POST",
      );
      await page.getByTestId("task-edit-dependency-add").click();
      expect((await added).status()).toBe(200);
      await expect(
        page.getByTestId(`task-edit-dependency-${parent.id}-${f.task.id}`),
      ).toBeVisible();
      const detail = await page.request.get(parentEndpoint);
      const edges = z
        .object({
          dependencies: z.array(z.object({ blockerId: z.string(), blockedId: z.string() })),
        })
        .parse(await detail.json());
      expect(edges.dependencies).toEqual([
        expect.objectContaining({ blockerId: parent.id, blockedId: f.task.id }),
      ]);
    } finally {
      release();
      await signed.context.close();
    }
  });
}

test("HTTP viewer rights keep task metadata disabled and reject direct writes", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  const viewer = { email: "task-metadata-viewer@example.com", password: "viewerpass1" };
  let viewing: Awaited<ReturnType<typeof newSignedInPage>> | undefined;
  try {
    const f = await fixture(signed.page, "TMV", {});
    const created = await signed.page.request.post(`${f.base}/projects/${f.projectId}/tasks`, {
      data: { title: "Viewer dependency target", statusId: f.task.statusId, type: "task" },
    });
    expect(created.status()).toBe(201);
    const target = taskSchema.parse(await created.json());
    createE2eUser(viewer.email, viewer.password, "Viewer", {
      workspaceSlug: admin.workspaceSlug,
      membershipRole: "guest",
    });
    const members = await signed.page.request.get(`${f.base}/members`);
    expect(members.status()).toBe(200);
    const viewerId = z
      .object({ items: z.array(z.object({ email: z.string(), userId: z.string() })) })
      .parse(await members.json())
      .items.find((item) => item.email === viewer.email)?.userId;
    if (!viewerId) throw new Error("Fixture requires viewer membership");
    expect(
      (
        await signed.page.request.post(`${f.base}/projects/${f.projectId}/members`, {
          data: { userId: viewerId, role: "viewer" },
        })
      ).status(),
    ).toBe(201);
    viewing = await newSignedInPage(browser, baseURL, viewer);
    const page = viewing.page;
    const detail = await page.request.get(f.endpoint);
    expect(detail.status()).toBe(200);
    expect(z.object({ canEdit: z.literal(false) }).parse(await detail.json()).canEdit).toBe(false);
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    await expect(page.getByTestId("task-edit-title")).toBeDisabled();
    await expect(page.getByTestId("task-edit-due-date")).toBeDisabled();
    await expect(page.getByTestId("task-edit-dependency-open")).toHaveCount(0);
    expect(
      (await page.request.patch(f.endpoint, { data: { title: "Denied title" } })).status(),
    ).toBe(404);
    expect(
      (
        await page.request.post(`${f.endpoint}/dependencies`, {
          data: { blockedId: target.id, type: "FS", lagDays: 0 },
        })
      ).status(),
    ).toBe(404);
    expect((await f.stored()).title).toBe(f.task.title);
    const committed = await signed.page.request.get(f.endpoint);
    expect(committed.status()).toBe(200);
    expect(
      z.object({ dependencies: z.array(z.unknown()) }).parse(await committed.json()).dependencies,
    ).toEqual([]);
  } finally {
    await viewing?.context.close();
    await signed.context.close();
  }
});

test("Calendar commit refreshes retained detail and Gantt before 30s even when stream hints are unavailable", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  const page = signed.page;
  try {
    const month = new Date().toISOString().slice(0, 7);
    const f = await fixture(page, "TCC", { startDate: `${month}-05`, dueDate: `${month}-07` });
    // Transport fault only; every task mutation/read still reaches real Rust/DB.
    await page.route("**/projects/*/stream", (route) =>
      route.fulfill({ status: 503, body: "transport unavailable" }),
    );
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue(`${month}-07`);
    const mountedAt = await page.evaluate(() => performance.timeOrigin);
    const started = Date.now();
    await page.locator(".task-home__crumb").getByRole("link", { name: "TCC", exact: true }).click();
    await page.locator(`a[href="${f.path}/gantt"]`).click();
    const bar = page.locator(`g[data-task-id="${f.task.id}"]`);
    await expect(bar).toHaveAttribute("data-end", `${month}-07`);
    await page.locator(`a[href="${f.path}/calendar"]`).click();
    await page.getByTestId(`collection-preview-${f.displayId}`).click();
    const editor = page.getByRole("form", { name: "Calendar event editor" });
    await editor.locator('input[type="date"]').fill(`${month}-09`);
    const savedResponse = page.waitForResponse(
      (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
    );
    await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
    const response = await savedResponse;
    expect(response.status()).toBe(200);
    const accepted = taskSchema.parse(await response.json());
    await expect(editor).toHaveCount(0);
    await expect(
      page.locator(`td[data-date="${month}-09"]`).getByTestId(`collection-preview-${f.displayId}`),
    ).toBeVisible();
    await page.locator(`a[href="${f.path}/gantt"]`).click();
    await expect(bar).toHaveAttribute("data-end", `${month}-09`);
    await page.locator(`a[href="${f.path}/tasks"]`).click();
    await page.getByTestId(`task-row-${f.task.id}`).click();
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue(`${month}-09`);
    await page.locator(".task-home__crumb").getByRole("link", { name: "TCC", exact: true }).click();
    await page.locator(`a[href="${f.path}/gantt"]`).click();
    const ganttResponse = page.waitForResponse(
      (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
    );
    await bar.focus();
    await bar.press("ArrowRight");
    const ganttSaved = await ganttResponse;
    expect(ganttSaved.status()).toBe(200);
    const rescheduled = taskSchema.parse(await ganttSaved.json());
    expect(rescheduled.startDate).toBe(`${month}-06`);
    expect(rescheduled.dueDate).toBe(`${month}-10`);
    expect(rescheduled.dueAt).toBeNull();
    await expect(bar).toHaveAttribute("data-end", `${month}-10`);
    await page.locator(`a[href="${f.path}/calendar"]`).click();
    await expect(
      page.locator(`td[data-date="${month}-10"]`).getByTestId(`collection-preview-${f.displayId}`),
    ).toBeVisible();
    await page.locator(`a[href="${f.path}/tasks"]`).click();
    await page.getByTestId(`task-row-${f.task.id}`).click();
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue(`${month}-10`);
    expect(await page.evaluate(() => performance.timeOrigin)).toBe(mountedAt);
    expect(Date.now() - started).toBeLessThan(30_000);
    expect(datesOf(await f.stored())).toEqual(datesOf(rescheduled));
    expect((await f.stored()).version).toBe(rescheduled.version);
    expect(rescheduled.version).toBe(accepted.version);
    await page.reload();
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue(`${month}-10`);
    await page.goto(`${f.path}/gantt`);
    await expect(bar).toHaveAttribute("data-start", `${month}-06`);
    await expect(bar).toHaveAttribute("data-end", `${month}-10`);
  } finally {
    await signed.context.close();
  }
});

test("authorized stream reopen requeries a mounted detail after missed peer metadata", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  const page = signed.page;
  try {
    const f = await fixture(page, "TRC", { dueDate: "2027-03-13" });
    let refused = true;
    await page.route("**/projects/*/stream", (route) =>
      refused ? route.fulfill({ status: 503, body: "transport unavailable" }) : route.continue(),
    );
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue("2027-03-13");
    const response = await page.request.patch(f.endpoint, {
      data: { title: "Peer rename", dueDate: "2027-03-15" },
    });
    expect(response.status()).toBe(200);
    const committed = taskSchema.parse(await response.json());
    const opened = page.waitForResponse((r) => r.url().endsWith("/stream") && r.status() === 200);
    refused = false;
    await opened;
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue(committed.dueDate ?? "");
    await expect(page.getByTestId("task-edit-title")).toHaveValue("Peer rename");
    expect(datesOf(await f.stored())).toEqual(datesOf(committed));
    // Keep a dirty date draft through a later real peer hint/refetch. Escape
    // then selects the newest committed value without writing the draft.
    const draft = page.getByTestId("task-edit-due-date");
    await draft.fill("2027-03-20");
    const refreshed = page.waitForResponse(
      async (r) =>
        r.url().endsWith(f.endpoint) &&
        r.request().method() === "GET" &&
        r.status() === 200 &&
        taskSchema.parse(await r.json()).dueDate === "2027-03-17",
    );
    expect(
      (await page.request.patch(f.endpoint, { data: { dueDate: "2027-03-17" } })).status(),
    ).toBe(200);
    await refreshed;
    await expect(draft).toHaveValue("2027-03-20");
    await draft.press("Escape");
    await expect(draft).toHaveValue("2027-03-17");
    expect((await f.stored()).dueDate).toBe("2027-03-17");
  } finally {
    await signed.context.close();
  }
});

test("a dirty detail date retains its original conflict baseline after a peer stream refresh", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  const page = signed.page;
  try {
    const f = await fixture(page, "TDC", { dueDate: "2027-03-13" });
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    const draft = page.getByTestId("task-edit-due-date");
    await expect(draft).toHaveValue("2027-03-13");
    await draft.fill("2027-03-20");
    const refreshed = page.waitForResponse(
      async (r) =>
        r.url().endsWith(f.endpoint) &&
        r.request().method() === "GET" &&
        r.status() === 200 &&
        taskSchema.parse(await r.json()).dueDate === "2027-03-17",
    );
    expect(
      (await page.request.patch(f.endpoint, { data: { dueDate: "2027-03-17" } })).status(),
    ).toBe(200);
    const peer = taskSchema.parse(await (await refreshed).json());
    expect(peer.dueDate).toBe("2027-03-17");
    await expect(draft).toHaveValue("2027-03-20");
    const attempted = page.waitForResponse(
      (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
    );
    await draft.blur();
    const conflict = await attempted;
    expect(conflict.status()).toBe(409);
    expect(conflict.request().postDataJSON()).toMatchObject({
      expectedDates: { dueDate: "2027-03-13" },
    });
    expect(problemSchema.parse(await conflict.json()).code).toBe("document_version_mismatch");
    expect((await f.stored()).dueDate).toBe("2027-03-17");
    await expect(draft).toHaveValue("2027-03-17");
    for (const day of ["2027-03-18", "2027-03-19"]) {
      await draft.fill(day);
      const saved = page.waitForResponse(
        (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
      );
      await draft.blur();
      expect((await saved).status()).toBe(200);
      await expect(draft).toBeEnabled();
      expect((await f.stored()).dueDate).toBe(day);
    }
  } finally {
    await signed.context.close();
  }
});
