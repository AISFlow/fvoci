import { z } from "zod";
import {
  expect,
  test,
  type Browser,
  type BrowserContext,
  type Page,
  type Route,
} from "@playwright/test";
import { createE2eUser } from "./helpers";
import { admin, newSignedInPage, setupInstance, workspaceId } from "./workspace-wiki-vue-editor";

const taskSchema = z
  .object({
    id: z.string().uuid(),
    number: z.number(),
    title: z.string(),
    type: z.string(),
    parentId: z.string().uuid().nullable(),
    statusId: z.string().uuid(),
    priority: z.string(),
    startDate: z.string().nullable(),
    dueDate: z.string().nullable(),
    dueAt: z.string().nullable(),
  })
  .passthrough();
const detailSchema = taskSchema.extend({ canEdit: z.boolean() });
const projectSchema = z.object({ id: z.string().uuid() });
const workflowSchema = z.object({ statuses: z.array(z.object({ id: z.string().uuid() })) });
type Task = z.infer<typeof taskSchema>;
let auth: Awaited<ReturnType<BrowserContext["storageState"]>>;
test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
  const signed = await newSignedInPage(browser, baseURL, admin);
  try {
    auth = await signed.context.storageState();
  } finally {
    await signed.context.close();
  }
});

async function signedPage(browser: Browser, baseURL: string | undefined) {
  const context = await browser.newContext({ baseURL, storageState: auth });
  return { context, page: await context.newPage() };
}
async function fixture(page: Page, key: string) {
  const ws = await workspaceId(page.request);
  const base = `/api/v1/workspaces/${ws}`;
  const created = await page.request.post(`${base}/projects`, {
    data: { key, name: key, visibility: "workspace" },
  });
  expect(created.status()).toBe(201);
  const project = projectSchema.parse(await created.json());
  const statuses = workflowSchema.parse(
    await (await page.request.get(`${base}/projects/${project.id}/workflow`)).json(),
  ).statuses;
  const [initial, other] = statuses;
  if (!initial || !other) throw new Error("Fixture needs two workflow statuses");
  async function make(title: string, type: string, parentId: string | null = null) {
    const response = await page.request.post(`${base}/projects/${project.id}/tasks`, {
      data: {
        title,
        type,
        ...(parentId ? { parentId } : {}),
        statusId: initial.id,
        dueDate: "2027-03-13",
      },
    });
    expect(response.status(), await response.text()).toBe(201);
    return taskSchema.parse(await response.json());
  }
  const parent = await make(`${key} original epic`, "epic");
  const alternative = await make(`${key} alternative epic`, "epic");
  const task = await make(`${key} child`, "task", parent.id);
  const endpoint = `${base}/tasks/${task.id}`;
  const path = (row: Task) => `/w/${admin.workspaceSlug}/${key}-${String(row.number)}`;
  async function stored() {
    const response = await page.request.get(endpoint);
    expect(response.status()).toBe(200);
    return taskSchema.parse(await response.json());
  }
  return { base, project, parent, alternative, task, other, endpoint, path, stored };
}
type Fixture = Awaited<ReturnType<typeof fixture>>;
function hierarchy(task: Task) {
  return { type: task.type, parentId: task.parentId };
}
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
}
// The request reaches real Rust/DB before its actual response is held.
// No fabricated response, random sleep, or Query private state is used.
async function responseBarrier(page: Page, endpoint: string, method: string) {
  const held = deferred();
  const answered = deferred();
  const handler = async (route: Route) => {
    if (route.request().method() !== method) return route.continue();
    const response = await route.fetch();
    answered.resolve();
    await held.promise;
    await route.fulfill({ response });
  };
  await page.route(`**${endpoint}`, handler);
  return { answered: answered.promise, release: held.resolve };
}
async function draft(page: Page, f: Fixture, kind: "epic" | "parent") {
  if (kind === "epic") await page.getByTestId("task-edit-type").selectOption("epic");
  else {
    await page.getByTestId("task-edit-parent").click();
    await page.getByTestId(`task-edit-parent-option-${f.alternative.id}`).click();
  }
  await expect(page.getByTestId("task-edit-hierarchy-save")).toBeEnabled();
}
async function assertDraft(page: Page, f: Fixture, kind: "epic" | "parent") {
  await expect(page.getByTestId("task-edit-type")).toHaveValue(kind === "epic" ? "epic" : "task");
  if (kind === "parent") {
    await page.getByTestId("task-edit-parent").click();
    await expect(page.getByTestId(`task-edit-parent-option-${f.alternative.id}`)).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await page.getByTestId("task-edit-parent").click();
  }
  await expect(page.getByTestId("task-edit-hierarchy-save")).toBeEnabled();
}
async function saveAndReadback(
  page: Page,
  f: Fixture,
  expected: { type: string; parentId: string | null },
  browser: Browser,
  baseURL: string | undefined,
) {
  const saved = page.waitForResponse(
    (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
  );
  await page.getByTestId("task-edit-hierarchy-save").click();
  const response = await saved;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual(expected);
  expect(hierarchy(taskSchema.parse(await response.json()))).toEqual(expected);
  expect(hierarchy(await f.stored())).toEqual(expected);
  await expect(page.getByTestId("task-edit-hierarchy-save")).toBeDisabled();
  const fresh = await signedPage(browser, baseURL);
  try {
    const get = await fresh.page.request.get(f.endpoint);
    expect(get.status()).toBe(200);
    expect(hierarchy(taskSchema.parse(await get.json()))).toEqual(expected);
    await fresh.page.goto(f.path(f.task));
    await expect(fresh.page.getByTestId("task-edit-type")).toHaveValue(expected.type);
    await expect(fresh.page.getByTestId("task-edit-hierarchy-save")).toBeDisabled();
    if (expected.parentId) {
      await expect(fresh.page.getByTestId("task-edit-parent")).toContainText(f.alternative.title);
    }
  } finally {
    await fresh.context.close();
  }
}

for (const action of ["status", "priority"] as const) {
  for (const kind of ["epic", "parent"] as const) {
    test(`${action} preserves unsaved ${kind} hierarchy until explicit Save and fresh reentry`, async ({
      browser,
      baseURL,
    }) => {
      const signed = await signedPage(browser, baseURL);
      let release = () => {};
      try {
        const page = signed.page;
        const f = await fixture(
          page,
          `${action === "status" ? "TCS" : "TCP"}${kind === "epic" ? "E" : "P"}`,
        );
        await page.goto(f.path(f.task));
        await draft(page, f, kind);
        const endpoint = action === "status" ? `${f.endpoint}/move` : f.endpoint;
        const method = action === "status" ? "POST" : "PATCH";
        const barrier = await responseBarrier(page, endpoint, method);
        release = barrier.release;
        const committed = page.waitForResponse(
          (r) => r.url().endsWith(endpoint) && r.request().method() === method,
        );
        await page
          .getByTestId(`task-edit-${action}`)
          .selectOption(action === "status" ? f.other.id : "high");
        await barrier.answered;
        const beforeSave = await f.stored();
        expect(hierarchy(beforeSave)).toEqual(hierarchy(f.task));
        expect(action === "status" ? beforeSave.statusId : beforeSave.priority).toBe(
          action === "status" ? f.other.id : "high",
        );
        release();
        const response = await committed;
        expect(response.status()).toBe(200);
        expect(response.request().postDataJSON()).toEqual(
          action === "status"
            ? { statusId: f.other.id, expectedStatusId: f.task.statusId }
            : { priority: "high" },
        );
        expect(hierarchy(taskSchema.parse(await response.json()))).toEqual(hierarchy(f.task));
        await expect(page.getByTestId(`task-edit-${action}`)).toHaveValue(
          action === "status" ? f.other.id : "high",
        );
        await assertDraft(page, f, kind);
        await saveAndReadback(
          page,
          f,
          kind === "epic"
            ? { type: "epic", parentId: null }
            : { type: "task", parentId: f.alternative.id },
          browser,
          baseURL,
        );
      } finally {
        release();
        await signed.context.close();
      }
    });
  }
}

for (const dirty of ["type", "parent"] as const) {
  test(`same-target live refetch protects dirty ${dirty} and syncs its untouched peer`, async ({
    browser,
    baseURL,
  }) => {
    const signed = await signedPage(browser, baseURL);
    try {
      const page = signed.page;
      const f = await fixture(page, dirty === "type" ? "TCLT" : "TCLP");
      await page.goto(f.path(f.task));
      if (dirty === "type") await page.getByTestId("task-edit-type").selectOption("bug");
      else await draft(page, f, "parent");
      const committed =
        dirty === "type"
          ? { type: "story", parentId: f.alternative.id }
          : { type: "story", parentId: f.parent.id };
      const refreshed = page.waitForResponse(
        async (r) =>
          r.url().endsWith(f.endpoint) &&
          r.request().method() === "GET" &&
          r.status() === 200 &&
          taskSchema.parse(await r.json()).type === "story",
      );
      expect((await page.request.patch(f.endpoint, { data: committed })).status()).toBe(200);
      await refreshed;
      await expect(page.getByTestId("task-edit-type")).toHaveValue(
        dirty === "type" ? "bug" : "story",
      );
      await page.getByTestId("task-edit-parent").click();
      await expect(page.getByTestId(`task-edit-parent-option-${f.alternative.id}`)).toHaveAttribute(
        "aria-selected",
        "true",
      );
      await page.getByTestId("task-edit-parent").click();
      expect(hierarchy(await f.stored())).toEqual(committed);
      await page.getByTestId("task-edit-hierarchy-cancel").click();
      await expect(page.getByTestId("task-edit-type")).toHaveValue("story");
      await expect(page.getByTestId("task-edit-hierarchy-save")).toBeDisabled();
      await expect(page.getByTestId("task-edit-parent")).toContainText(
        dirty === "type" ? f.alternative.title : f.parent.title,
      );
    } finally {
      await signed.context.close();
    }
  });
}

for (const aba of [false, true]) {
  test(`late status success retires across ${aba ? "A-B-A" : "A-B"} without overwriting the selected hierarchy`, async ({
    browser,
    baseURL,
  }) => {
    const signed = await signedPage(browser, baseURL);
    let release = () => {};
    try {
      const page = signed.page;
      const f = await fixture(page, aba ? "TCABA" : "TCAB");
      await page.goto(f.path(f.task));
      await draft(page, f, "parent");
      const barrier = await responseBarrier(page, `${f.endpoint}/move`, "POST");
      release = barrier.release;
      const response = page.waitForResponse(
        (r) => r.url().endsWith(`${f.endpoint}/move`) && r.request().method() === "POST",
      );
      await page.getByTestId("task-edit-status").selectOption(f.other.id);
      await barrier.answered;
      await page.locator(`a[href="${f.path(f.parent)}"]`).click();
      await expect(page.getByRole("heading", { name: f.parent.title, exact: true })).toBeVisible();
      if (aba) {
        await page
          .locator(".task-home__crumb")
          .getByRole("link", { name: "TCABA", exact: true })
          .click();
        await page.getByTestId(`task-row-${f.task.id}`).click();
        await expect(page.getByTestId("task-edit-type")).toHaveValue("task");
      }
      await expect(page.getByTestId("task-edit-hierarchy-save")).toBeDisabled();
      // A retained page disables metadata while its previous move is pending.
      // Returning via the list mounts a new page whose draft has its own lifetime.
      if (aba) await page.getByTestId("task-edit-type").selectOption("bug");
      else await expect(page.getByTestId("task-edit-type")).toBeDisabled();
      release();
      expect((await response).status()).toBe(200);
      await expect(page.getByTestId("task-edit-type")).toBeEnabled();
      await expect(page.getByTestId("task-edit-type")).toHaveValue(aba ? "bug" : "epic");
      if (aba) await expect(page.getByTestId("task-edit-hierarchy-save")).toBeEnabled();
      else await expect(page.getByTestId("task-edit-hierarchy-save")).toBeDisabled();
      await expect(page.getByRole("alert")).toHaveCount(0);
      expect(hierarchy(await f.stored())).toEqual(hierarchy(f.task));
    } finally {
      release();
      await signed.context.close();
    }
  });
}

test("real hierarchy rejection retains the draft and user error until Cancel", async ({
  browser,
  baseURL,
}) => {
  const signed = await signedPage(browser, baseURL);
  let release = () => {};
  try {
    const page = signed.page;
    const f = await fixture(page, "TCFAIL");
    await page.goto(f.path(f.task));
    await draft(page, f, "parent");
    // Retire the selected parent in real Rust before dispatching the captured Save.
    const started = deferred();
    const held = deferred();
    release = held.resolve;
    await page.route(`**${f.endpoint}`, async (route) => {
      if (route.request().method() !== "PATCH") return route.continue();
      started.resolve();
      await held.promise;
      await route.continue();
    });
    const saved = page.waitForResponse(
      (r) => r.url().endsWith(f.endpoint) && r.request().method() === "PATCH",
    );
    await page.getByTestId("task-edit-hierarchy-save").click();
    await started.promise;
    const deleted = await page.request.delete(`${f.base}/tasks/${f.alternative.id}`);
    expect(deleted.status()).toBe(200);
    release();
    const rejection = await saved;
    expect(rejection.status()).toBe(404);
    await expect(page.getByRole("alert")).toBeVisible();
    await expect(page.getByTestId("task-edit-hierarchy-save")).toBeEnabled();
    expect(hierarchy(await f.stored())).toEqual(hierarchy(f.task));
    await page.getByTestId("task-edit-hierarchy-cancel").click();
    await expect(page.getByTestId("task-edit-hierarchy-save")).toBeDisabled();
    await expect(page.getByTestId("task-edit-parent")).toContainText(f.parent.title);
  } finally {
    release();
    await signed.context.close();
  }
});

test("real permission loss retires hierarchy; regrant never restores the old draft", async ({
  browser,
  baseURL,
}) => {
  const signed = await signedPage(browser, baseURL);
  let editing: Awaited<ReturnType<typeof newSignedInPage>> | undefined;
  try {
    const f = await fixture(signed.page, "TCPERM");
    const actor = { email: "tb-c-editor@example.com", password: "editorpass1" };
    createE2eUser(actor.email, actor.password, "Hierarchy Editor", {
      workspaceSlug: admin.workspaceSlug,
      membershipRole: "guest",
    });
    const members = z
      .object({ items: z.array(z.object({ email: z.string(), userId: z.string().uuid() })) })
      .parse(await (await signed.page.request.get(`${f.base}/members`)).json());
    const actorId = members.items.find((item) => item.email === actor.email)?.userId;
    if (!actorId) throw new Error("Fixture needs the editor membership");
    const memberEndpoint = `${f.base}/projects/${f.project.id}/members`;
    expect(
      (
        await signed.page.request.post(memberEndpoint, {
          data: { userId: actorId, role: "member" },
        })
      ).status(),
    ).toBe(201);
    editing = await newSignedInPage(browser, baseURL, actor);
    const page = editing.page;
    await page.goto(f.path(f.task));
    await draft(page, f, "epic");
    expect(
      (
        await signed.page.request.patch(`${memberEndpoint}/${actorId}`, {
          data: { role: "viewer" },
        })
      ).status(),
    ).toBe(200);
    const denied = await page.request.patch(f.endpoint, { data: { type: "epic", parentId: null } });
    expect(denied.status()).toBe(404);
    const readOnly = page.waitForResponse(
      async (r) =>
        r.url().endsWith(f.endpoint) &&
        r.request().method() === "GET" &&
        r.status() === 200 &&
        !detailSchema.parse(await r.json()).canEdit,
    );
    // A real task stream event prompts a same-target permission-aware GET.
    expect(
      (await signed.page.request.patch(f.endpoint, { data: { priority: "high" } })).status(),
    ).toBe(200);
    await readOnly;
    await expect(page.getByTestId("task-edit-type")).toBeDisabled();
    await expect(page.getByTestId("task-edit-type")).toHaveValue("task");
    expect(hierarchy(await f.stored())).toEqual(hierarchy(f.task));
    expect(
      (
        await signed.page.request.patch(`${memberEndpoint}/${actorId}`, {
          data: { role: "member" },
        })
      ).status(),
    ).toBe(200);
    const editable = page.waitForResponse(
      async (r) =>
        r.url().endsWith(f.endpoint) &&
        r.request().method() === "GET" &&
        r.status() === 200 &&
        detailSchema.parse(await r.json()).canEdit,
    );
    expect(
      (await signed.page.request.patch(f.endpoint, { data: { priority: "urgent" } })).status(),
    ).toBe(200);
    await editable;
    await expect(page.getByTestId("task-edit-type")).toBeEnabled();
    await expect(page.getByTestId("task-edit-type")).toHaveValue("task");
    await expect(page.getByTestId("task-edit-hierarchy-save")).toBeDisabled();
  } finally {
    await editing?.context.close();
    await signed.context.close();
  }
});
