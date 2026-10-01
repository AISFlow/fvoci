import { execFileSync } from "node:child_process";
import { z } from "zod";
import { expect, test, type Page } from "@playwright/test";
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
    const response = await page.request.patch(f.endpoint, { data: { dueDate: "2027-03-15" } });
    expect(response.status()).toBe(200);
    const committed = taskSchema.parse(await response.json());
    const opened = page.waitForResponse((r) => r.url().endsWith("/stream") && r.status() === 200);
    refused = false;
    await opened;
    await expect(page.getByTestId("task-edit-due-date")).toHaveValue(committed.dueDate ?? "");
    expect(datesOf(await f.stored())).toEqual(datesOf(committed));
  } finally {
    await signed.context.close();
  }
});
