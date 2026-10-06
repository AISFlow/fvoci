import type { Editor } from "@tiptap/core";
import type { HocuspocusProvider } from "@hocuspocus/provider";
import * as Y from "yjs";
import { decodeHocuspocusFrame, frameBytes, persistParts } from "../e2e-pending/collab-wire";
import { execFileSync } from "node:child_process";
import { z } from "zod";
import { expect, test, type BrowserContext, type Page, type TestInfo } from "@playwright/test";
import { createE2eUser } from "./helpers";
import { admin, newSignedInPage, setupInstance, workspaceId } from "./workspace-wiki-vue-editor";

type NativeAdmissionUpdate = {
  local: boolean;
  providerOrigin: boolean;
  clientId: number;
  bytes: number[];
  before: string;
  after: string;
};
type AdmissionOwner = {
  doc: Y.Doc;
  provider: HocuspocusProvider;
  editor: Editor;
  element: HTMLElement;
  clientId: number;
  updates: number;
  localUpdates: number;
  unauthorizedLocalWrites: number;
  records: NativeAdmissionUpdate[];
};

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
let adminAuth: Awaited<ReturnType<BrowserContext["storageState"]>> | undefined;
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

function readTaskBodyDb(workspace: string, task: string) {
  const connection = process.env.DATABASE_APP_URL;
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!connection || !container?.startsWith("fvoci-rust-test-pg-"))
    throw new Error("Requires owned restricted app-role DB");
  const app = new URL(connection);
  if (
    !/^fvoci_app_fvoci_e2e_[a-f0-9]{16}$/.test(app.username) ||
    !/^\/fvoci_e2e_[a-f0-9]{16}$/.test(app.pathname)
  )
    throw new Error("Invalid fixture app role");
  for (const id of [workspace, task])
    if (!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(id))
      throw new Error("Invalid fixture resource");
  const result = JSON.parse(
    execFileSync(
      "docker",
      [
        "exec",
        "-i",
        container,
        "psql",
        "-X",
        "-qAt",
        "-U",
        app.username,
        "-d",
        app.pathname.slice(1),
        "-v",
        "ON_ERROR_STOP=1",
      ],
      {
        input: `BEGIN READ ONLY; SET LOCAL app.tenant_id = '${workspace}'; SELECT jsonb_build_object('contentJson',t.content_json,'version',t.version,'role',current_user,'superuser',r.rolsuper,'bypassRls',r.rolbypassrls,'rlsActive',row_security_active(c.oid),'notOwner',pg_get_userbyid(c.relowner) <> current_user) FROM fvoci.tasks t JOIN pg_roles r ON r.rolname=current_user JOIN pg_class c ON c.oid='fvoci.tasks'::regclass WHERE t.id='${task}' AND t.workspace_id='${workspace}'; ROLLBACK;`,
        encoding: "utf8",
      },
    ),
  ) as unknown;
  const witness = z
    .object({
      contentJson: z.unknown(),
      version: z.number(),
      role: z.string(),
      superuser: z.boolean(),
      bypassRls: z.boolean(),
      rlsActive: z.boolean(),
      notOwner: z.boolean(),
    })
    .parse(result);
  expect(witness).toMatchObject({
    role: app.username,
    superuser: false,
    bypassRls: false,
    rlsActive: true,
    notOwner: true,
  });
  return witness;
}

function datesOf(task: z.infer<typeof taskSchema>) {
  return { startDate: task.startDate, dueDate: task.dueDate, dueAt: task.dueAt };
}

test("real PATCH omission/null/date/version and 409 preserve the committed row", async ({
  browser,
  baseURL,
}) => {
  const signed = await newSignedInPage(browser, baseURL, admin);
  // Reuse this real authenticated session for the extra permission fixture;
  // the ten original cases already exhaust the server's per-email login limit.
  adminAuth = await signed.context.storageState();
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
  }, testInfo) => {
    const signed = await newSignedInPage(browser, baseURL, admin);
    const page = signed.page;
    let release = () => {};
    let releaseAdmission = () => {};
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
      const admissions: { generation: number; scope: string; delivered: boolean }[] = [];
      const requests: { generation: number; id: string }[] = [];
      const acks: { generation: number; id: string }[] = [];
      let socketGeneration = 0;
      let deliverGrant = () => {};
      let grantHeld = false;
      const workspace = f.base.split("/").at(-1);
      if (!workspace) throw new Error("Missing workspace fixture");
      const routingKey = `${workspace}:task:${parent.id}`;
      if (archivedTarget)
        await page.routeWebSocket(/\/collab(?:\?|$)/, (socket) => {
          const generation = ++socketGeneration;
          const server = socket.connectToServer();
          socket.onMessage((message) => {
            const frame = decodeHocuspocusFrame(frameBytes(message));
            const parts = frame?.kind === "stateless" ? persistParts(frame.payload) : null;
            if (
              frame &&
              "routingKey" in frame &&
              frame.routingKey === routingKey &&
              parts?.kind === "request"
            )
              requests.push({ generation, id: parts.id });
            server.send(message);
          });
          server.onMessage((message) => {
            const frame = decodeHocuspocusFrame(frameBytes(message));
            if (frame?.kind === "auth-scope" && frame.routingKey === routingKey) {
              const admission = { generation, scope: frame.scope, delivered: false };
              admissions.push(admission);
              if (
                frame.scope === "read-write" &&
                admissions.some((item) => item.scope === "readonly")
              ) {
                grantHeld = true;
                deliverGrant = () => {
                  admission.delivered = true;
                  socket.send(message);
                };
                return;
              }
              admission.delivered = true;
            }
            const parts = frame?.kind === "stateless" ? persistParts(frame.payload) : null;
            if (
              frame &&
              "routingKey" in frame &&
              frame.routingKey === routingKey &&
              parts?.kind === "done"
            )
              acks.push({ generation, id: parts.id });
            socket.send(message);
          });
        });
      releaseAdmission = () => {
        if (grantHeld) {
          grantHeld = false;
          deliverGrant();
        }
      };
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
        await expect(page.getByTestId("task-clone")).toHaveCount(0);
        await expect(page.locator("[data-comment-compose]")).toHaveCount(0);
        await expect(
          page
            .getByTestId("task-time-entries")
            .getByRole("button", { name: "기록 추가", exact: true }),
        ).toHaveCount(0);
        const retainedUrl = page.url();
        const timeOrigin = await page.evaluate(() => performance.timeOrigin);
        const body = page.getByTestId("task-body");
        await expect(body.locator('[data-collab-status="connected"]')).toBeVisible();
        await expect(body.locator(".ProseMirror")).toHaveAttribute("contenteditable", "false");
        await expect(body.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
        await body.locator(".ProseMirror").evaluate((root) => {
          const element = root as HTMLElement & {
            editor: Editor;
            retainedAdmission?: AdmissionOwner;
          };
          const options = (name: string) =>
            element.editor.extensionManager.extensions.find((extension) => extension.name === name)
              ?.options as Record<string, unknown>;
          const doc = options("collaboration").document as Y.Doc;
          const provider = options("collaborationCaret").provider as HocuspocusProvider;
          const witness: AdmissionOwner = {
            doc,
            provider,
            editor: element.editor,
            element,
            clientId: doc.clientID,
            updates: 0,
            localUpdates: 0,
            unauthorizedLocalWrites: 0,
            records: [],
          };
          element.retainedAdmission = witness;
          let previous = doc.getXmlFragment("prosemirror").toJSON();
          doc.on(
            "update",
            (bytes: Uint8Array, origin: unknown, _doc: Y.Doc, transaction: Y.Transaction) => {
              witness.updates++;
              if (transaction.local) witness.localUpdates++;
              if (transaction.local && !element.editor.isEditable)
                witness.unauthorizedLocalWrites++;
              const after = doc.getXmlFragment("prosemirror").toJSON();
              witness.records.push({
                local: transaction.local,
                providerOrigin: origin === provider,
                clientId: doc.clientID,
                bytes: Array.from(bytes),
                before: previous,
                after,
              });
              previous = after;
            },
          );
        });
        const observeAdmission = () =>
          body.evaluate((root) => {
            const element = root.querySelector(".ProseMirror") as HTMLElement & {
              editor: Editor;
              retainedAdmission?: AdmissionOwner;
            };
            const witness = element.retainedAdmission;
            if (!witness) throw new Error("Task body or its actual editor was remounted");
            const options = (name: string) =>
              element.editor.extensionManager.extensions.find(
                (extension) => extension.name === name,
              )?.options as Record<string, unknown>;
            const save = Array.from(root.querySelectorAll("button")).find(
              (button) => button.textContent.trim() === "저장",
            );
            if (!save) throw new Error("Missing actual Save affordance");
            return {
              sameEditor: element.editor === witness.editor,
              sameElement: element === witness.element,
              sameDoc: options("collaboration").document === witness.doc,
              sameProvider: options("collaborationCaret").provider === witness.provider,
              sameClientId: witness.doc.clientID === witness.clientId,
              authenticated: witness.provider.isAuthenticated,
              scope: witness.provider.authorizedScope,
              status: root
                .querySelector("[data-collab-status]")
                ?.getAttribute("data-collab-status"),
              editorEditable: element.editor.isEditable,
              domEditable: element.getAttribute("contenteditable"),
              canPersistAffordance: !save.disabled,
              updates: witness.updates,
              localUpdates: witness.localUpdates,
              unauthorizedLocalWrites: witness.unauthorizedLocalWrites,
              records: witness.records,
            };
          });
        expect(await observeAdmission()).toMatchObject({
          sameDoc: true,
          sameProvider: true,
          sameClientId: true,
          authenticated: true,
          scope: "readonly",
          editorEditable: false,
          domEditable: "false",
          canPersistAffordance: false,
          updates: 0,
        });
        const originalCanonicalResponse = await page.request.get(parentEndpoint);
        expect(originalCanonicalResponse.status()).toBe(200);
        const originalCanonical = z
          .object({ contentJson: z.unknown(), version: z.number() })
          .parse(await originalCanonicalResponse.json());
        const originalDb = readTaskBodyDb(workspace, parent.id);
        expect(originalDb).toMatchObject(originalCanonical);
        const oldAdmission = admissions.find((item) => item.scope === "readonly" && item.delivered);
        if (!oldAdmission) throw new Error("Requires actual server readonly authentication");
        const collectionEndpoint = `${parentEndpoint}/collection-item`;
        const archivedCollection = await page.request.get(collectionEndpoint);
        expect(archivedCollection.status()).toBe(200);
        expect(
          z.object({ canEdit: z.literal(false) }).parse(await archivedCollection.json()).canEdit,
        ).toBe(false);
        const collectionRefetched = page.waitForResponse(
          async (r) =>
            r.url().endsWith(collectionEndpoint) &&
            r.request().method() === "GET" &&
            r.status() === 200 &&
            z.object({ canEdit: z.boolean() }).parse(await r.json()).canEdit,
        );
        expect(
          (await page.request.patch(parentEndpoint, { data: { archived: false } })).status(),
        ).toBe(200);
        expect(
          z.object({ canEdit: z.literal(true) }).parse(await (await collectionRefetched).json())
            .canEdit,
        ).toBe(true);
        const restored = await page.request.get(parentEndpoint);
        expect(restored.status()).toBe(200);
        expect(
          z.object({ archivedAt: z.null(), canEdit: z.literal(true) }).parse(await restored.json())
            .archivedAt,
        ).toBeNull();
        await expect(page.getByTestId("task-edit-title")).toBeEnabled();
        await expect(page).toHaveURL(retainedUrl);
        expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin);
        // HTTP alone cannot promote the old readonly admission; the real new server grant is deliberately held.
        await expect(body.locator(".ProseMirror")).toHaveAttribute("contenteditable", "false");
        await expect(body.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
        const timeEndpoint = `${parentEndpoint}/time-entries`;
        const allowed = await page.request.get(timeEndpoint);
        expect(allowed.status()).toBe(200);
        expect(z.object({ canCreate: z.literal(true) }).parse(await allowed.json()).canCreate).toBe(
          true,
        );
        await expect(
          page
            .getByTestId("task-time-entries")
            .getByRole("button", { name: "기록 추가", exact: true }),
        ).toBeVisible();
        await expect(page.getByTestId("task-clone")).toBeEnabled();
        await expect(page.locator("[data-comment-compose] textarea")).toBeEnabled();
        const timePanel = page.getByTestId("task-time-entries");
        await timePanel.getByRole("button", { name: "기록 추가", exact: true }).click();
        // Fixed unambiguous past local time; planned future time is not an
        // elapsed record. Keep the exact independent 1800-second DB oracle.
        await timePanel.getByLabel("시작", { exact: true }).fill("2020-03-14T10:00");
        await timePanel.getByLabel("종료", { exact: true }).fill("2020-03-14T10:30");
        await timePanel.getByLabel("메모", { exact: true }).fill("UI restored REST entry");
        await timePanel
          .getByLabel("기록·수정 사유", { exact: true })
          .fill("부모 복원 후 허용된 30분 기록");
        expect(
          await timePanel.locator("form").evaluate((form) => {
            const fields = Array.from(
              form.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("input, textarea"),
            );
            const ids = fields.map((field) => field.id);
            return {
              fields: fields.length,
              unique: ids.every(
                (id) =>
                  Boolean(id) && document.querySelectorAll(`[id="${CSS.escape(id)}"]`).length === 1,
              ),
              associated: fields.every((field) =>
                Array.from(field.labels ?? []).some(
                  (label) => label.control === field && label.htmlFor === field.id,
                ),
              ),
            };
          }),
        ).toEqual({ fields: 4, unique: true, associated: true });
        const saved = page.waitForResponse(
          (r) =>
            new URL(r.url()).pathname === `${parentEndpoint}/timer/history` &&
            r.request().method() === "POST",
        );
        await timePanel.getByRole("button", { name: "기록 추가", exact: true }).click();
        const savedResponse = await saved;
        expect(savedResponse.status(), await savedResponse.text()).toBe(200);
        const record = z
          .object({
            record: z.object({
              id: z.string().uuid(),
              startedAt: z.string(),
              endedAt: z.string(),
              note: z.literal("UI restored REST entry"),
            }),
          })
          .parse(await savedResponse.json()).record;
        expect((Date.parse(record.endedAt) - Date.parse(record.startedAt)) / 1000).toBe(1800);
        const entry = { id: record.id, durationSeconds: 1800, note: record.note };
        await expect(timePanel).toContainText(entry.note);
        const database = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
        if (!database) throw new Error("Actual DB tracer requires wrapper-owned database");
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
              `SELECT json_build_object('id',id,'taskId',task_id,'durationSeconds',duration_seconds,'note',note) FROM fvoci.time_entries WHERE id='${entry.id}'::uuid`,
            ],
            { encoding: "utf8" },
          ),
        ) as { id: string; taskId: string; durationSeconds: number; note: string };
        expect(row).toEqual({ ...entry, taskId: parent.id });
        if (!adminAuth) throw new Error("Fresh client requires the earlier real admin session");
        const fresh = await browser.newContext({ baseURL, storageState: adminAuth });
        try {
          const reloaded = await fresh.request.get(timeEndpoint);
          expect(reloaded.status()).toBe(200);
          expect(
            z
              .object({
                items: z.array(
                  z.object({
                    id: z.string(),
                    note: z.string().nullable(),
                    durationSeconds: z.number().nullable(),
                  }),
                ),
              })
              .parse(await reloaded.json()).items,
          ).toContainEqual(entry);
        } finally {
          await fresh.close();
        }
        // Actual REST authorization succeeds while the new socket's genuine writable auth frame remains held.
        expect(
          (
            await page.request.post(timeEndpoint, {
              data: {
                startedAt: "2027-03-14T09:00:00Z",
                endedAt: "2027-03-14T09:30:00Z",
                note: "REST grant probe",
              },
            })
          ).status(),
        ).toBe(201);
        await expect(page).toHaveURL(retainedUrl);
        expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin);
        await expect(body.locator(".ProseMirror")).toHaveAttribute("contenteditable", "false");
        await expect(body.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
        await expect.poll(() => grantHeld).toBe(true);
        const newAdmission = admissions.at(-1);
        expect(newAdmission).toMatchObject({ scope: "read-write", delivered: false });
        if (!newAdmission) throw new Error("Missing actual fresh server writable admission");
        expect(newAdmission.generation).toBeGreaterThan(oldAdmission.generation);
        const causal = await observeAdmission();
        const canonicalResponse = await page.request.get(parentEndpoint);
        expect(canonicalResponse.status()).toBe(200);
        const canonical = z
          .object({ contentJson: z.unknown(), version: z.number() })
          .parse(await canonicalResponse.json());
        await testInfo.attach("w3-task-held-auth-causal-native-updates.json", {
          body: JSON.stringify({
            state: causal,
            canonical,
            originalCanonical,
            originalDb,
            currentDb: readTaskBodyDb(workspace, parent.id),
            decoded: causal.records.map((record) => {
              const update = Y.decodeUpdate(new Uint8Array(record.bytes));
              return {
                local: record.local,
                providerOrigin: record.providerOrigin,
                currentClientId: record.clientId,
                structs: update.structs.map((item) => ({
                  client: item.id.client,
                  clock: item.id.clock,
                  length: item.length,
                  kind: item.constructor.name,
                  contentKind: item instanceof Y.Item ? item.content.constructor.name : null,
                  text:
                    item instanceof Y.Item && item.content instanceof Y.ContentString
                      ? item.content.str
                      : null,
                })),
                deletes: Array.from(update.ds.clients, ([client, ranges]) => ({
                  client,
                  ranges: ranges.map((range) => ({ clock: range.clock, length: range.len })),
                })),
              };
            }),
          }),
          contentType: "application/json",
        });
        expect(canonical).toEqual(originalCanonical);
        expect(readTaskBodyDb(workspace, parent.id)).toEqual(originalDb);
        expect(causal).toMatchObject({
          sameEditor: true,
          sameElement: true,
          localUpdates: 0,
          unauthorizedLocalWrites: 0,
        });
        expect(await observeAdmission()).toMatchObject({
          sameDoc: true,
          sameProvider: true,
          sameClientId: true,
          editorEditable: false,
          domEditable: "false",
          canPersistAffordance: false,
          updates: 0,
        });
        expect(requests).toEqual([]);
        await testInfo.attach("w3-task-before-fresh-auth-delivery.json", {
          body: JSON.stringify({ admissions, state: await observeAdmission() }),
          contentType: "application/json",
        });
        await body.evaluate((root) => {
          const owner = window as Window & {
            taskAdmissionFrames?: {
              running: boolean;
              states: {
                scope: string | undefined;
                authenticated: boolean;
                status: string | null;
                editable: boolean;
                dom: string | null;
                saveEnabled: boolean;
              }[];
            };
          };
          const frames = {
            running: true,
            states: [] as {
              scope: string | undefined;
              authenticated: boolean;
              status: string | null;
              editable: boolean;
              dom: string | null;
              saveEnabled: boolean;
            }[],
          };
          owner.taskAdmissionFrames = frames;
          const sample = () => {
            if (!frames.running || frames.states.length >= 128) return;
            const element = root.querySelector(".ProseMirror") as HTMLElement & {
              editor: Editor;
              retainedAdmission: { provider: HocuspocusProvider };
            };
            const save = Array.from(root.querySelectorAll("button")).find(
              (button) => button.textContent.trim() === "저장",
            );
            frames.states.push({
              scope: element.retainedAdmission.provider.authorizedScope,
              authenticated: element.retainedAdmission.provider.isAuthenticated,
              status:
                root.querySelector("[data-collab-status]")?.getAttribute("data-collab-status") ??
                null,
              editable: element.editor.isEditable,
              dom: element.getAttribute("contenteditable"),
              saveEnabled: save !== undefined && !save.disabled,
            });
            requestAnimationFrame(sample);
          };
          requestAnimationFrame(sample);
        });
        releaseAdmission();
        // One joint public observation checks Save, editor and genuine provider
        // admission; neither HTTP canEdit nor a later Save-only sample is proof.
        await expect.poll(observeAdmission).toMatchObject({
          sameDoc: true,
          sameProvider: true,
          sameClientId: true,
          authenticated: true,
          scope: "read-write",
          status: "connected",
          editorEditable: true,
          domEditable: "true",
          canPersistAffordance: true,
          updates: 0,
        });
        await testInfo.attach("w3-task-fresh-auth-coherent.json", {
          body: JSON.stringify({ admissions, state: await observeAdmission() }),
          contentType: "application/json",
        });
        const frames = await page.evaluate(async () => {
          await new Promise<void>((resolve) =>
            requestAnimationFrame(() =>
              requestAnimationFrame(() => {
                resolve();
              }),
            ),
          );
          const owner = window as Window & {
            taskAdmissionFrames?: {
              running: boolean;
              states: {
                scope: string | undefined;
                authenticated: boolean;
                status: string | null;
                editable: boolean;
                dom: string | null;
                saveEnabled: boolean;
              }[];
            };
          };
          if (!owner.taskAdmissionFrames) throw new Error("Missing bounded frame observation");
          owner.taskAdmissionFrames.running = false;
          return owner.taskAdmissionFrames.states;
        });
        expect(
          frames.some((state) => state.scope === "read-write" && state.status === "connected"),
        ).toBe(true);
        for (const state of frames.filter(
          (state) => state.scope === "read-write" && state.status === "connected",
        )) {
          expect(state.dom).toBe(String(state.editable));
          expect(state.saveEnabled).toBe(state.editable);
        }
        await testInfo.attach("w3-task-admission-frame-coherence.json", {
          body: JSON.stringify(frames),
          contentType: "application/json",
        });
        const restoredBody = body.locator(".ProseMirror");
        await restoredBody.click();
        await page.keyboard.type("실제 재인증 태스크 본문");
        await body.getByRole("button", { name: "저장", exact: true }).click();
        await expect(body.locator('[data-collab-persisted="true"]')).toBeVisible();
        const matched = requests.filter(
          (request) =>
            request.generation === newAdmission.generation &&
            acks.some((ack) => ack.generation === request.generation && ack.id === request.id),
        );
        expect(matched.length).toBeGreaterThan(0);
        const savedTask = await page.request.get(parentEndpoint);
        expect(savedTask.status()).toBe(200);
        const committed = z
          .object({ contentJson: z.unknown() })
          .parse(await savedTask.json()).contentJson;
        expect(JSON.stringify(committed)).toContain("실제 재인증 태스크 본문");
        const appConnection = process.env.DATABASE_APP_URL;
        const container = process.env.FVOCI_TEST_PG_CONTAINER;
        if (!appConnection || !container?.startsWith("fvoci-rust-test-pg-"))
          throw new Error("Requires owned app-role task DB");
        const app = new URL(appConnection);
        if (
          !/^fvoci_app_fvoci_e2e_[a-f0-9]{16}$/.test(app.username) ||
          !/^\/fvoci_e2e_[a-f0-9]{16}$/.test(app.pathname)
        )
          throw new Error("Requires restricted fixture role");
        const storedBody = JSON.parse(
          execFileSync(
            "docker",
            [
              "exec",
              "-i",
              container,
              "psql",
              "-X",
              "-qAt",
              "-U",
              app.username,
              "-d",
              app.pathname.slice(1),
              "-v",
              "ON_ERROR_STOP=1",
            ],
            {
              input: `BEGIN READ ONLY; SET LOCAL app.tenant_id = '${workspace}'; SELECT content_json FROM fvoci.tasks WHERE id='${parent.id}'; ROLLBACK;`,
              encoding: "utf8",
            },
          ),
        ) as unknown;
        expect(storedBody).toEqual(committed);
        const newest = await browser.newContext({ baseURL, storageState: adminAuth });
        try {
          const client = await newest.newPage();
          await client.goto(retainedUrl);
          await expect(client.getByTestId("task-body").locator(".ProseMirror")).toContainText(
            "실제 재인증 태스크 본문",
          );
          const response = await client.request.get(parentEndpoint);
          expect(response.status()).toBe(200);
          expect(
            z.object({ contentJson: z.unknown() }).parse(await response.json()).contentJson,
          ).toEqual(committed);
        } finally {
          await newest.close();
        }
        expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin);
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
      releaseAdmission();
      await page
        .evaluate(() => {
          const owner = window as Window & { taskAdmissionFrames?: { running: boolean } };
          if (owner.taskAdmissionFrames) owner.taskAdmissionFrames.running = false;
        })
        .catch(() => undefined);
      await signed.context.close();
    }
  });
}

test("HTTP viewer rights keep task metadata disabled and reject direct writes", async ({
  browser,
  baseURL,
}) => {
  if (!adminAuth) throw new Error("Permission fixture requires the earlier real admin session");
  const context = await browser.newContext({ baseURL, storageState: adminAuth });
  const signed = { context, page: await context.newPage() };
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
    await expect(page.getByTestId("task-clone")).toHaveCount(0);
    await expect(page.locator("[data-comment-compose]")).toHaveCount(0);
    await expect(
      page.getByTestId("task-time-entries").getByRole("button", { name: "기록 추가", exact: true }),
    ).toHaveCount(0);
    await expect(page.getByTestId("task-body").locator(".ProseMirror")).toHaveAttribute(
      "contenteditable",
      "false",
    );
    const times = await page.request.get(`${f.endpoint}/time-entries`);
    expect(times.status()).toBe(200);
    expect(z.object({ canCreate: z.literal(false) }).parse(await times.json()).canCreate).toBe(
      false,
    );
    expect(
      (
        await page.request.post(`${f.endpoint}/time-entries`, {
          data: { startedAt: "2027-03-14T10:00:00Z", endedAt: "2027-03-14T10:30:00Z" },
        })
      ).status(),
    ).toBe(404);
    expect((await page.request.post(`${f.endpoint}/clone`)).status()).toBe(404);
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
    const streamPath = `${f.base}/task-stream`;
    await page.route(`**${streamPath}`, (route) =>
      route.fulfill({ status: 503, body: "transport unavailable" }),
    );
    const refusedStream = page.waitForResponse(
      (response) => new URL(response.url()).pathname === streamPath && response.status() === 503,
    );
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    await refusedStream;
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
    const streamPath = `${f.base}/task-stream`;
    let refused = true;
    await page.route(`**${streamPath}`, (route) =>
      refused ? route.fulfill({ status: 503, body: "transport unavailable" }) : route.continue(),
    );
    const refusedStream = page.waitForResponse(
      (response) => new URL(response.url()).pathname === streamPath && response.status() === 503,
    );
    await page.goto(`/w/${admin.workspaceSlug}/${f.displayId}`);
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue("2027-03-13");
    await refusedStream;
    const response = await page.request.patch(f.endpoint, {
      data: { title: "Peer rename", dueDate: "2027-03-15" },
    });
    expect(response.status()).toBe(200);
    const committed = taskSchema.parse(await response.json());
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue("2027-03-13");
    await expect(page.getByTestId("task-edit-title")).toHaveValue(f.task.title);
    const opened = page.waitForResponse(
      (response) => new URL(response.url()).pathname === streamPath && response.status() === 200,
    );
    refused = false;
    const reopened = await opened;
    expect(reopened.headers()["content-type"]).toContain("text/event-stream");
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

const taskAdmissionEventKinds = [
  "transport-open",
  "auth-received",
  "frame-hold",
  "frame-release",
  "frame-delivery",
  "persist-request",
  "persist-ack",
  "browser-open",
  "browser-close",
  "browser-authenticated",
  "browser-state",
  "http-archive",
  "http-unarchive",
  "http-denied",
  "cleanup-start",
] as const;
type TaskAdmissionEventKind = (typeof taskAdmissionEventKinds)[number];

// Artifact projection only: never serialize raw frames, routing keys, IDs or error text.
function taskAdmissionFields(value: unknown): Record<string, string | number | boolean> {
  const output: Record<string, string | number | boolean> = {};
  if (typeof value !== "object" || value === null) return output;
  const input = value as Record<string, unknown>;
  for (const key of [
    "order",
    "socket",
    "activeSocket",
    "frame",
    "authenticationEpoch",
    "initialAuthenticationEpoch",
    "updates",
    "localUpdates",
    "unauthorizedLocalWrites",
    "violations",
    "dropped",
    "requestCount",
    "ackCount",
  ]) {
    const number = input[key];
    output[key] =
      typeof number === "number" &&
      Number.isSafeInteger(number) &&
      number >= 0 &&
      number <= 1_000_000
        ? number
        : "unknown";
  }
  for (const key of ["elapsedMs", "code"]) {
    const number = input[key];
    output[key] =
      typeof number === "number" &&
      Number.isSafeInteger(number) &&
      number >= 0 &&
      number <= (key === "code" ? 4999 : 3_600_000)
        ? number
        : "unknown";
  }
  for (const key of [
    "authenticated",
    "editable",
    "canPersistAffordance",
    "saveEnabled",
    "ownerBroken",
    "sameRoot",
    "sameEditor",
    "sameElement",
    "sameDoc",
    "sameProvider",
    "sameClientId",
    "sameSocket",
    "archived",
    "canEdit",
    "completed",
  ]) {
    output[key] = typeof input[key] === "boolean" ? input[key] : "unknown";
  }
  for (const key of ["scope", "authenticatedScope"]) {
    output[key] = input[key] === "readonly" || input[key] === "read-write" ? input[key] : "unknown";
  }
  output.status = ["connected", "connecting", "disconnected", "unauthorized"].includes(
    typeof input.status === "string" ? input.status : "",
  )
    ? (input.status as string)
    : "unknown";
  output.dom = input.dom === "true" || input.dom === "false" ? input.dom : "unknown";
  return output;
}

function createTaskAdmissionDiagnostic(clock: () => number = () => performance.now()) {
  let start = 0;
  try {
    start = clock();
  } catch {
    /* diagnostic clock unavailable */
  }
  const events: Record<string, string | number | boolean>[] = [];
  let order = 0;
  let dropped = 0;
  const record = (kind: TaskAdmissionEventKind, value: unknown = {}) => {
    try {
      if (!taskAdmissionEventKinds.includes(kind)) return;
      const elapsedMs = Math.floor(clock() - start);
      const event = { ...taskAdmissionFields(value), kind, order: ++order, elapsedMs };
      if (events.length === 256) {
        events.splice(16, 1); // Keep the initial admissions plus the most recent events.
        dropped = Math.min(1_000_000, dropped + 1);
      }
      events.push(event);
    } catch {
      // Diagnostics cannot interrupt forwarding, held-frame release or an assertion.
    }
  };
  return { record, snapshot: () => ({ events: events.map((event) => ({ ...event })), dropped }) };
}

function taskAdmissionPacket(value: unknown) {
  const input =
    typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
  const project = (lane: unknown) => {
    const object =
      typeof lane === "object" && lane !== null ? (lane as Record<string, unknown>) : {};
    const events = Array.isArray(object.events) ? (object.events as unknown[]) : [];
    return {
      dropped: taskAdmissionFields(object).dropped,
      events: events.slice(-256).flatMap((event) => {
        if (typeof event !== "object" || event === null) return [];
        const kind = (event as Record<string, unknown>).kind;
        if (!taskAdmissionEventKinds.some((allowed) => allowed === kind)) return [];
        return [{ ...taskAdmissionFields(event), kind }];
      }),
    };
  };
  return {
    schema: "w3-task-fresh-admission-v1",
    routeDeliveryMeaning: "route-send-returned",
    routeToProviderSocketBinding: "unknown",
    crossLaneClockOrder: "unknown",
    route: project(input.route),
    browser: project(input.browser),
    state: taskAdmissionFields(input.state),
    completed: input.completed === true,
  };
}

async function attachTaskAdmissionDiagnostic(
  testInfo: Pick<TestInfo, "attach">,
  schedule: "natural" | "server-readonly" | "retired-grant",
  read: () => Promise<unknown>,
) {
  if (!["natural", "server-readonly", "retired-grant"].includes(schedule)) return;
  const capture = { active: true };
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      (async () => {
        let value: unknown;
        try {
          value = await read();
        } catch {
          /* unavailable observation */
        }
        if (!capture.active) return;
        await testInfo.attach(`w3-task-${schedule}-wire-coherence.json`, {
          body: JSON.stringify(taskAdmissionPacket(value)),
          contentType: "application/json",
        });
      })().catch(() => undefined),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, 500);
      }),
    ]);
  } finally {
    capture.active = false;
    clearTimeout(timer);
  }
}

for (const schedule of ["natural", "server-readonly", "retired-grant"] as const) {
  test(`actual task fresh admission ${schedule} preserves coherent editor and Save authority on the same Y.Doc`, async ({
    browser,
    baseURL,
  }, testInfo) => {
    if (!adminAuth) throw new Error("Requires earlier actual admin login");
    const context = await browser.newContext({ baseURL, storageState: adminAuth });
    const page = await context.newPage();
    let releaseFrame = () => {};
    const diagnostic = createTaskAdmissionDiagnostic();
    let completed = false;
    let frameOrdinal = 0;
    try {
      const f = await fixture(
        page,
        schedule === "natural" ? "TGN" : schedule === "server-readonly" ? "TGR" : "TGL",
        {},
      );
      const initial = await page.request.patch(f.endpoint, { data: { archived: true } });
      expect(initial.status()).toBe(200);
      diagnostic.record("http-archive", { archived: true });
      const originalBody = z
        .object({ contentJson: z.unknown(), version: z.number() })
        .parse(await (await page.request.get(f.endpoint)).json());
      const workspace = f.base.split("/").at(-1);
      if (!workspace) throw new Error("Missing workspace");
      const routingKey = `${workspace}:task:${f.task.id}`;
      let generation = 0;
      let held = false;
      const admissions: { generation: number; scope: string; delivered: boolean }[] = [];
      const requests: { generation: number; id: string }[] = [];
      const acks: { generation: number; id: string }[] = [];
      await page.routeWebSocket(/\/collab(?:\?|$)/, (socket) => {
        const current = ++generation;
        const server = socket.connectToServer();
        diagnostic.record("transport-open", { socket: current });
        socket.onMessage((message) => {
          const frame = decodeHocuspocusFrame(frameBytes(message));
          if (
            schedule === "server-readonly" &&
            current > 1 &&
            frame?.kind === "auth-token" &&
            frame.routingKey === routingKey
          ) {
            const ordinal = ++frameOrdinal;
            diagnostic.record("frame-hold", { socket: current, frame: ordinal });
            held = true;
            releaseFrame = () => {
              held = false;
              diagnostic.record("frame-release", { socket: current, frame: ordinal });
              server.send(message);
              diagnostic.record("frame-delivery", { socket: current, frame: ordinal });
            };
            return;
          }
          const parts = frame?.kind === "stateless" ? persistParts(frame.payload) : null;
          if (
            frame &&
            "routingKey" in frame &&
            frame.routingKey === routingKey &&
            parts?.kind === "request"
          ) {
            requests.push({ generation: current, id: parts.id });
            diagnostic.record("persist-request", {
              socket: current,
              requestCount: requests.length,
            });
          }
          server.send(message);
        });
        server.onMessage((message) => {
          const frame = decodeHocuspocusFrame(frameBytes(message));
          let forwardedAdmission: { frame: number; scope: string } | undefined;
          if (frame?.kind === "auth-scope" && frame.routingKey === routingKey) {
            const ordinal = ++frameOrdinal;
            diagnostic.record("auth-received", {
              socket: current,
              frame: ordinal,
              scope: frame.scope,
            });
            const admission = { generation: current, scope: frame.scope, delivered: false };
            admissions.push(admission);
            if (schedule === "retired-grant" && frame.scope === "read-write") {
              diagnostic.record("frame-hold", {
                socket: current,
                frame: ordinal,
                scope: frame.scope,
              });
              held = true;
              releaseFrame = () => {
                if (!held) return;
                held = false;
                admission.delivered = true;
                diagnostic.record("frame-release", {
                  socket: current,
                  frame: ordinal,
                  scope: frame.scope,
                });
                socket.send(message);
                diagnostic.record("frame-delivery", {
                  socket: current,
                  frame: ordinal,
                  scope: frame.scope,
                });
              };
              return;
            }
            admission.delivered = true;
            forwardedAdmission = { frame: ordinal, scope: frame.scope };
          }
          const parts = frame?.kind === "stateless" ? persistParts(frame.payload) : null;
          if (
            frame &&
            "routingKey" in frame &&
            frame.routingKey === routingKey &&
            parts?.kind === "done"
          ) {
            acks.push({ generation: current, id: parts.id });
            diagnostic.record("persist-ack", { socket: current, ackCount: acks.length });
          }
          socket.send(message);
          if (forwardedAdmission)
            diagnostic.record("frame-delivery", { socket: current, ...forwardedAdmission });
        });
      });
      const path = `/w/${admin.workspaceSlug}/${f.displayId}`;
      await page.goto(path);
      const body = page.getByTestId("task-body");
      await expect(body.locator('[data-collab-status="connected"]')).toBeVisible();
      const editor = body.locator(".ProseMirror");
      await expect(editor).toHaveAttribute("contenteditable", "false");
      await expect(body.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
      await body.evaluate((root) => {
        const element = root.querySelector(".ProseMirror") as HTMLElement & { editor: Editor };
        const options = (name: string) =>
          element.editor.extensionManager.extensions.find((extension) => extension.name === name)
            ?.options as Record<string, unknown>;
        const doc = options("collaboration").document as Y.Doc;
        const provider = options("collaborationCaret").provider as HocuspocusProvider;
        const owner = window as Window & {
          taskFreshAdmission?: {
            doc: Y.Doc;
            provider: HocuspocusProvider;
            editor: Editor;
            element: HTMLElement;
            clientId: number;
            root: Element;
            updates: number;
            localUpdates: number;
            unauthorizedLocalWrites: number;
            records: NativeAdmissionUpdate[];
            running: boolean;
            ownerBroken: boolean;
            violations: number;
            authenticationEpoch: number;
            initialAuthenticationEpoch: number;
            authenticatedScope: string | undefined;
            states: {
              scope: string | undefined;
              authenticated: boolean;
              authenticationEpoch: number;
              authenticatedScope: string | undefined;
              status: string | null;
              editable: boolean;
              dom: string | null;
              saveEnabled: boolean;
            }[];
          };
        };
        const witness = {
          doc,
          provider,
          editor: element.editor,
          element,
          clientId: doc.clientID,
          root,
          updates: 0,
          localUpdates: 0,
          unauthorizedLocalWrites: 0,
          records: [] as NativeAdmissionUpdate[],
          running: true,
          ownerBroken: false,
          violations: 0,
          authenticationEpoch: 0,
          initialAuthenticationEpoch: 0,
          authenticatedScope: provider.authorizedScope,
          states: [] as {
            scope: string | undefined;
            authenticated: boolean;
            authenticationEpoch: number;
            authenticatedScope: string | undefined;
            status: string | null;
            editable: boolean;
            dom: string | null;
            saveEnabled: boolean;
          }[],
        };
        owner.taskFreshAdmission = witness;
        let append: (kind: string, value?: Record<string, unknown>) => void = () => {};
        try {
          const diagnosticOwner = window as Window & {
            taskFreshAdmissionDiagnostic?: {
              events: Record<string, unknown>[];
              dropped: number;
            };
          };
          const observed = { events: [] as Record<string, unknown>[], dropped: 0 };
          diagnosticOwner.taskFreshAdmissionDiagnostic = observed;
          const start = performance.now();
          const sockets = new WeakMap<object, number>();
          let socketOrdinal = 0;
          let order = 0;
          const socketNumber = (socket: object | null | undefined) => {
            if (!socket) return "unknown";
            let ordinal = sockets.get(socket);
            if (ordinal === undefined) {
              ordinal = ++socketOrdinal;
              sockets.set(socket, ordinal);
            }
            return ordinal;
          };
          append = (kind: string, value: Record<string, unknown> = {}) => {
            try {
              if (observed.events.length === 256) {
                observed.events.splice(16, 1);
                observed.dropped = Math.min(1_000_000, observed.dropped + 1);
              }
              observed.events.push({
                ...value,
                kind,
                order: ++order,
                elapsedMs: Math.floor(performance.now() - start),
                activeSocket: socketNumber(provider.configuration.websocketProvider.webSocket),
                authenticationEpoch: witness.authenticationEpoch,
                authenticatedScope: witness.authenticatedScope,
                scope: provider.authorizedScope,
                authenticated: provider.isAuthenticated,
              });
            } catch {
              /* no diagnostic failure may escape a provider callback */
            }
          };
          // Public events observe closure without overriding route close forwarding.
          provider.on("open", (payload: { event: Event }) => {
            try {
              const current = provider.configuration.websocketProvider.webSocket;
              append("browser-open", {
                socket: socketNumber(payload.event.target),
                sameSocket:
                  !payload.event.target || !current ? "unknown" : payload.event.target === current,
              });
            } catch {
              /* unavailable event target */
            }
          });
          provider.on("close", (payload: { event: CloseEvent }) => {
            try {
              const current = provider.configuration.websocketProvider.webSocket;
              append("browser-close", {
                socket: socketNumber(payload.event.target),
                code: payload.event.code,
                sameSocket:
                  !payload.event.target || !current ? "unknown" : payload.event.target === current,
              });
            } catch {
              /* unavailable close witness */
            }
          });
        } catch {
          /* diagnostics unavailable; original witness still operates */
        }
        provider.on("authenticated", ({ scope }: { scope: typeof provider.authorizedScope }) => {
          witness.authenticationEpoch++;
          witness.authenticatedScope = scope;
          append("browser-authenticated");
        });
        let previousDiagnosticState: Record<string, unknown> | undefined;
        let previous = doc.getXmlFragment("prosemirror").toJSON();
        doc.on(
          "update",
          (bytes: Uint8Array, origin: unknown, _doc: Y.Doc, transaction: Y.Transaction) => {
            witness.updates++;
            if (transaction.local) witness.localUpdates++;
            if (transaction.local && !element.editor.isEditable) witness.unauthorizedLocalWrites++;
            const after = doc.getXmlFragment("prosemirror").toJSON();
            witness.records.push({
              local: transaction.local,
              providerOrigin: origin === provider,
              clientId: doc.clientID,
              bytes: Array.from(bytes),
              before: previous,
              after,
            });
            previous = after;
          },
        );
        const sample = () => {
          if (!witness.running) return;
          const current = root.querySelector(".ProseMirror") as HTMLElement & { editor: Editor };
          if (current !== witness.element || current.editor !== witness.editor) {
            witness.ownerBroken = true;
            append("browser-state", { ownerBroken: true });
            return;
          }
          if (witness.states.length >= 256) witness.states.splice(1, 1);
          const save = Array.from(root.querySelectorAll("button")).find(
            (button) => button.textContent.trim() === "저장",
          );
          const state = {
            scope: provider.authorizedScope,
            authenticated: provider.isAuthenticated,
            authenticationEpoch: witness.authenticationEpoch,
            authenticatedScope: witness.authenticatedScope,
            status:
              root.querySelector("[data-collab-status]")?.getAttribute("data-collab-status") ??
              null,
            editable: current.editor.isEditable,
            dom: current.getAttribute("contenteditable"),
            saveEnabled: save !== undefined && !save.disabled,
          };
          if (
            state.status === "connected" &&
            (state.dom !== String(state.editable) ||
              state.saveEnabled !== state.editable ||
              (state.authenticated && state.scope === "readonly" && state.editable))
          )
            witness.violations++;
          witness.states.push(state);
          try {
            const diagnosticState = {
              ...state,
              status: state.status,
              editable: state.editable,
              dom: state.dom,
              saveEnabled: state.saveEnabled,
              updates: witness.updates,
              localUpdates: witness.localUpdates,
              unauthorizedLocalWrites: witness.unauthorizedLocalWrites,
              ownerBroken: witness.ownerBroken,
              violations: witness.violations,
            };
            if (
              !previousDiagnosticState ||
              Object.entries(diagnosticState).some(
                ([key, value]) => previousDiagnosticState?.[key] !== value,
              )
            )
              append("browser-state", diagnosticState);
            previousDiagnosticState = diagnosticState;
          } catch {
            /* diagnostics cannot stop the original sampling loop */
          }
          requestAnimationFrame(sample);
        };
        sample();
      });
      const observe = () =>
        body.evaluate((root) => {
          const owner = window as Window & {
            taskFreshAdmission?: {
              doc: Y.Doc;
              provider: HocuspocusProvider;
              clientId: number;
              root: Element;
              editor: Editor;
              element: HTMLElement;
              updates: number;
              localUpdates: number;
              unauthorizedLocalWrites: number;
              records: NativeAdmissionUpdate[];
              ownerBroken: boolean;
              violations: number;
              authenticationEpoch: number;
              initialAuthenticationEpoch: number;
              authenticatedScope: string | undefined;
            };
          };
          const witness = owner.taskFreshAdmission;
          if (!witness) throw new Error("Missing actual task admission");
          const element = root.querySelector(".ProseMirror") as HTMLElement & { editor: Editor };
          const options = (name: string) =>
            element.editor.extensionManager.extensions.find((extension) => extension.name === name)
              ?.options as Record<string, unknown>;
          const save = Array.from(root.querySelectorAll("button")).find(
            (button) => button.textContent.trim() === "저장",
          );
          return {
            authenticationEpoch: witness.authenticationEpoch,
            initialAuthenticationEpoch: witness.initialAuthenticationEpoch,
            authenticatedScope: witness.authenticatedScope,
            ownerBroken: witness.ownerBroken,
            violations: witness.violations,
            sameRoot: root === witness.root,
            sameEditor: element.editor === witness.editor,
            sameElement: element === witness.element,
            sameDoc: options("collaboration").document === witness.doc,
            sameProvider: options("collaborationCaret").provider === witness.provider,
            sameClientId: witness.doc.clientID === witness.clientId,
            authenticated: witness.provider.isAuthenticated,
            scope: witness.provider.authorizedScope,
            status: root.querySelector("[data-collab-status]")?.getAttribute("data-collab-status"),
            editable: element.editor.isEditable,
            dom: element.getAttribute("contenteditable"),
            canPersistAffordance: save !== undefined && !save.disabled,
            updates: witness.updates,
            localUpdates: witness.localUpdates,
            unauthorizedLocalWrites: witness.unauthorizedLocalWrites,
            records: witness.records,
          };
        });
      expect(await observe()).toMatchObject({
        authenticated: true,
        scope: "readonly",
        editable: false,
        dom: "false",
        canPersistAffordance: false,
        updates: 0,
      });
      const timeOrigin = await page.evaluate(() => performance.timeOrigin);
      expect((await page.request.patch(f.endpoint, { data: { archived: false } })).status()).toBe(
        200,
      );
      diagnostic.record("http-unarchive", { archived: false });
      if (schedule !== "natural") {
        await expect.poll(() => held).toBe(true);
        expect(await observe()).toMatchObject({
          editable: false,
          dom: "false",
          canPersistAffordance: false,
          updates: 0,
        });
        const collection = `${f.endpoint}/collection-item`;
        const retired = page.waitForResponse(async (response) => {
          const denied =
            response.url().endsWith(collection) &&
            response.status() === 200 &&
            response.request().method() === "GET" &&
            !z.object({ canEdit: z.boolean() }).parse(await response.json()).canEdit;
          if (denied) diagnostic.record("http-denied", { canEdit: false });
          return denied;
        });
        expect((await page.request.patch(f.endpoint, { data: { archived: true } })).status()).toBe(
          200,
        );
        diagnostic.record("http-archive", { archived: true });
        await retired;
        releaseFrame();
      }
      await expect.poll(observe).toMatchObject({
        ownerBroken: false,
        violations: 0,
        sameRoot: true,
        sameEditor: true,
        sameElement: true,
        sameDoc: true,
        sameProvider: true,
        sameClientId: true,
        authenticated: true,
        scope: schedule === "server-readonly" ? "readonly" : "read-write",
        status: "connected",
        editable: schedule === "natural",
        dom: String(schedule === "natural"),
        canPersistAffordance: schedule === "natural",
        updates: 0,
      });
      const frames = await page.evaluate(async () => {
        await new Promise<void>((resolve) =>
          requestAnimationFrame(() =>
            requestAnimationFrame(() => {
              resolve();
            }),
          ),
        );
        const owner = window as Window & {
          taskFreshAdmission?: {
            running: boolean;
            states: {
              scope: string | undefined;
              authenticated: boolean;
              authenticationEpoch: number;
              authenticatedScope: string | undefined;
              status: string | null;
              editable: boolean;
              dom: string | null;
              saveEnabled: boolean;
            }[];
          };
        };
        if (!owner.taskFreshAdmission) throw new Error("Missing bounded observation");
        owner.taskFreshAdmission.running = false;
        return owner.taskFreshAdmission.states;
      });
      const finalAdmission = await observe();
      expect(finalAdmission).toMatchObject({
        ownerBroken: false,
        violations: 0,
        sameRoot: true,
        sameEditor: true,
        sameElement: true,
        sameDoc: true,
        sameProvider: true,
        sameClientId: true,
        authenticated: true,
        scope: schedule === "server-readonly" ? "readonly" : "read-write",
        status: "connected",
        editable: schedule === "natural",
        dom: String(schedule === "natural"),
        canPersistAffordance: schedule === "natural",
        updates: 0,
      });
      expect(finalAdmission.authenticationEpoch).toBeGreaterThan(
        finalAdmission.initialAuthenticationEpoch,
      );
      expect(
        frames.some(
          (frame) =>
            frame.authenticated &&
            frame.authenticationEpoch > finalAdmission.initialAuthenticationEpoch &&
            frame.authenticatedScope ===
              (schedule === "server-readonly" ? "readonly" : "read-write") &&
            frame.scope === frame.authenticatedScope &&
            frame.status === "connected",
        ),
      ).toBe(true);
      expect(
        frames.some(
          (frame) =>
            frame.authenticated &&
            frame.scope === "readonly" &&
            frame.status === "connected" &&
            !frame.editable &&
            !frame.saveEnabled,
        ),
      ).toBe(true);
      if (schedule === "natural")
        expect(
          frames.some(
            (frame) =>
              frame.authenticated &&
              frame.scope === "read-write" &&
              frame.status === "connected" &&
              frame.editable &&
              frame.saveEnabled,
          ),
        ).toBe(true);
      for (const frame of frames.filter((frame) => frame.status === "connected")) {
        expect(frame.dom).toBe(String(frame.editable));
        expect(frame.saveEnabled).toBe(frame.editable);
        if (frame.scope === "readonly") expect(frame.editable).toBe(false);
      }
      expect(
        admissions.some(
          (admission) =>
            admission.generation > 1 &&
            admission.delivered &&
            admission.scope === (schedule === "server-readonly" ? "readonly" : "read-write"),
        ),
      ).toBe(true);
      expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin);
      if (schedule === "natural") {
        await editor.click();
        await page.keyboard.type("자연스러운 태스크 재인증 한글");
        await body.getByRole("button", { name: "저장", exact: true }).click();
        await expect(body.locator('[data-collab-persisted="true"]')).toBeVisible();
        expect(
          requests.some(
            (request) =>
              request.generation > 1 &&
              acks.some((ack) => ack.generation === request.generation && ack.id === request.id),
          ),
        ).toBe(true);
        const response = await page.request.get(f.endpoint);
        expect(response.status()).toBe(200);
        const committed = z
          .object({ contentJson: z.unknown() })
          .parse(await response.json()).contentJson;
        expect(JSON.stringify(committed)).toContain("자연스러운 태스크 재인증 한글");
        const connection = process.env.DATABASE_APP_URL;
        const container = process.env.FVOCI_TEST_PG_CONTAINER;
        if (!connection || !container?.startsWith("fvoci-rust-test-pg-"))
          throw new Error("Requires own restricted app DB");
        const app = new URL(connection);
        if (
          !/^fvoci_app_fvoci_e2e_[a-f0-9]{16}$/.test(app.username) ||
          !/^\/fvoci_e2e_[a-f0-9]{16}$/.test(app.pathname)
        )
          throw new Error("Invalid app-role fixture");
        const stored = JSON.parse(
          execFileSync(
            "docker",
            [
              "exec",
              "-i",
              container,
              "psql",
              "-X",
              "-qAt",
              "-U",
              app.username,
              "-d",
              app.pathname.slice(1),
              "-v",
              "ON_ERROR_STOP=1",
            ],
            {
              input: `BEGIN READ ONLY; SET LOCAL app.tenant_id = '${workspace}'; SELECT content_json FROM fvoci.tasks WHERE id='${f.task.id}'; ROLLBACK;`,
              encoding: "utf8",
            },
          ),
        ) as unknown;
        expect(stored).toEqual(committed);
        const fresh = await browser.newContext({ baseURL, storageState: adminAuth });
        try {
          const newest = await fresh.newPage();
          await newest.goto(path);
          await expect(newest.getByTestId("task-body").locator(".ProseMirror")).toContainText(
            "자연스러운 태스크 재인증 한글",
          );
          const saved = await newest.request.get(f.endpoint);
          expect(saved.status()).toBe(200);
          expect(
            z.object({ contentJson: z.unknown() }).parse(await saved.json()).contentJson,
          ).toEqual(committed);
        } finally {
          await fresh.close();
        }
      } else {
        expect(requests).toEqual([]);
        const response = await page.request.get(f.endpoint);
        expect(response.status()).toBe(200);
        expect(
          z.object({ contentJson: z.unknown(), version: z.number() }).parse(await response.json()),
        ).toEqual(originalBody);
      }
      completed = true;
    } finally {
      diagnostic.record("cleanup-start", { completed });
      // Capture on success AND failure, before the original release/stop/close sequence.
      await attachTaskAdmissionDiagnostic(testInfo, schedule, async () => {
        let browserState: unknown;
        try {
          browserState = await page.evaluate(() => {
            const owner = window as Window & {
              taskFreshAdmissionDiagnostic?: { events: Record<string, unknown>[]; dropped: number };
              taskFreshAdmission?: AdmissionOwner & {
                root: Element;
                ownerBroken: boolean;
                violations: number;
                authenticationEpoch: number;
                initialAuthenticationEpoch: number;
                authenticatedScope: string | undefined;
              };
            };
            const witness = owner.taskFreshAdmission;
            if (!witness) return { browser: owner.taskFreshAdmissionDiagnostic };
            const root = document.querySelector('[data-testid="task-body"]');
            const element = root?.querySelector(".ProseMirror") as
              (HTMLElement & { editor: Editor }) | null;
            const options = (name: string) =>
              element?.editor.extensionManager.extensions.find(
                (extension) => extension.name === name,
              )?.options as Record<string, unknown> | undefined;
            const save = Array.from(root?.querySelectorAll("button") ?? []).find(
              (button) => button.textContent.trim() === "저장",
            );
            return {
              browser: owner.taskFreshAdmissionDiagnostic,
              state: {
                authenticationEpoch: witness.authenticationEpoch,
                initialAuthenticationEpoch: witness.initialAuthenticationEpoch,
                authenticatedScope: witness.authenticatedScope,
                authenticated: witness.provider.isAuthenticated,
                scope: witness.provider.authorizedScope,
                status: root
                  ?.querySelector("[data-collab-status]")
                  ?.getAttribute("data-collab-status"),
                editable: element?.editor.isEditable,
                dom: element?.getAttribute("contenteditable"),
                canPersistAffordance: save !== undefined && !save.disabled,
                ownerBroken: witness.ownerBroken,
                violations: witness.violations,
                sameRoot: root === witness.root,
                sameElement: element === witness.element,
                sameEditor: element?.editor === witness.editor,
                sameDoc: options("collaboration")?.document === witness.doc,
                sameProvider: options("collaborationCaret")?.provider === witness.provider,
                sameClientId: witness.doc.clientID === witness.clientId,
                updates: witness.updates,
                localUpdates: witness.localUpdates,
                unauthorizedLocalWrites: witness.unauthorizedLocalWrites,
              },
            };
          });
        } catch {
          /* original assertion/cleanup must survive unavailable page evaluation */
        }
        const captured =
          typeof browserState === "object" && browserState !== null
            ? (browserState as Record<string, unknown>)
            : {};
        return {
          route: diagnostic.snapshot(),
          browser: captured.browser,
          state: captured.state,
          completed,
        };
      });
      releaseFrame();
      await page
        .evaluate(() => {
          const owner = window as Window & { taskFreshAdmission?: { running: boolean } };
          if (owner.taskFreshAdmission) owner.taskFreshAdmission.running = false;
        })
        .catch(() => undefined);
      await context.close();
    }
  });
}
