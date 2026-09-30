// The project Gantt (/w/:slug/:ref/gantt) is a page of the Vue app; every
// other page here is the React app's. These flows run against the production
// build served by the Rust server, like every web e2e group.
import assert from "node:assert/strict";
import { z } from "zod";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { createE2eUser, login, watchCspViolations } from "./helpers";

// Validate the response fields used by this flow; retain the complete payload.
const workspaceListSchema = z
  .object({ items: z.array(z.object({ id: z.string(), slug: z.string() }).passthrough()) })
  .passthrough();
const projectSchema = z
  .object({
    id: z.string(),
    key: z.string(),
    name: z.string(),
    rootDocumentId: z.string().nullable(),
  })
  .passthrough();
const taskSchema = z
  .object({
    id: z.string(),
    number: z.number(),
    startDate: z.union([z.string(), z.null()]),
    dueDate: z.union([z.string(), z.null()]),
    dueAt: z.union([z.string(), z.null()]),
  })
  .passthrough();
const meSchema = z
  .object({ userId: z.string(), timezone: z.string(), weekStartsOn: z.number() })
  .passthrough();
const layoutSchema = z
  .object({
    scale: z.object({ pxPerDay: z.number() }).passthrough(),
    columns: z.array(z.object({ date: z.string(), offDuty: z.boolean() }).passthrough()),
    bars: z.array(
      z
        .object({ id: z.string(), x: z.number(), width: z.number(), lane: z.number() })
        .passthrough(),
    ),
    paths: z.array(z.array(z.number())),
  })
  .passthrough();
const timezoneSchema = z.object({ userId: z.string(), timezone: z.string() }).passthrough();
const datePatchSchema = z
  .object({
    startDate: z.string(),
    dueDate: z.string().nullable().optional(),
    dueAt: z.string(),
    expectedDates: z
      .object({
        startDate: z.string().nullable(),
        dueDate: z.string().nullable(),
        dueAt: z.string().nullable(),
      })
      .passthrough(),
  })
  .passthrough();
const errorSchema = z.object({ code: z.string() }).passthrough();
const memberListSchema = z
  .object({
    items: z.array(
      z.object({ email: z.string(), userId: z.string(), role: z.string() }).passthrough(),
    ),
  })
  .passthrough();

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "pgantt",
  workspaceName: "Project Gantt E2E",
};

const viewer = {
  email: "gantt-viewer@example.com",
  password: "viewerpass1",
  givenName: "보기",
  familyName: "전용",
};

// A fixed month, so bars and dates do not depend on the day the suite runs.
const Y = 2031;
const M = 3;
const day = (d: number) => `${String(Y)}-0${String(M)}-${String(d).padStart(2, "0")}`;

// Icon sets must be bundled; these are the Iconify API hosts a runtime fetch would hit.
const ICON_API_HOSTS = ["api.iconify.design", "api.simplesvg.com", "api.unisvg.com"];

type Task = {
  id: string;
  number: number;
  startDate: string | null;
  dueDate: string | null;
  dueAt: string | null;
};

function watchIconRequests(page: Page): string[] {
  const hits: string[] = [];
  page.on("request", (request) => {
    const host = new URL(request.url()).hostname;
    if (ICON_API_HOSTS.includes(host)) hits.push(request.url());
  });
  return hits;
}

async function workspaceId(request: APIRequestContext, slug: string): Promise<string> {
  const res = await request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const workspace = workspaceListSchema
    .parse(await res.json())
    .items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  assert(workspace);
  return workspace.id;
}

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
  if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
}

async function createProject(
  request: APIRequestContext,
  wsId: string,
  key: string,
  visibility: "workspace" | "private" = "workspace",
): Promise<{ id: string; key: string; name: string }> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key, name: `Gantt ${key} ${String(Date.now())}`, visibility },
  });
  expect(res.status()).toBe(201);
  return projectSchema.parse(await res.json());
}

async function createTask(
  request: APIRequestContext,
  wsId: string,
  projectId: string,
  data: Record<string, string>,
): Promise<Task> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`, {
    data,
  });
  expect(res.status()).toBe(201);
  return taskSchema.parse(await res.json());
}

async function getTask(request: APIRequestContext, wsId: string, taskId: string): Promise<Task> {
  const res = await request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
  expect(res.ok()).toBe(true);
  return taskSchema.parse(await res.json());
}

function ganttUrl(key: string): string {
  return `/w/${admin.workspaceSlug}/${key}/gantt?y=${String(Y)}&m=${String(M)}`;
}

function bar(page: Page, taskId: string) {
  return page.locator(`[data-slot="gantt"] g[data-task-id="${taskId}"]`);
}

/** Drags a bar (or one of its handles) by whole days, at the chart's own scale. */
async function dragBy(
  page: Page,
  taskId: string,
  days: number,
  part: "bar" | "start" | "end" = "bar",
) {
  const pxPerDay = Number(
    await page.locator('[data-slot="gantt"]').getAttribute("data-px-per-day"),
  );
  expect(pxPerDay).toBeGreaterThan(0);
  const target =
    part === "bar"
      ? bar(page, taskId).locator(".fvoci-gantt__bar-rect")
      : bar(page, taskId).locator(`[data-handle="${part}"]`);
  await target.scrollIntoViewIfNeeded();
  const box = await target.boundingBox();
  if (!box) throw new Error("Gantt bar bounds unavailable");
  const fromX = box.x + box.width / 2;
  const fromY = box.y + box.height / 2;
  await page.mouse.move(fromX, fromY);
  await page.mouse.down();
  await page.mouse.move(fromX + days * pxPerDay, fromY, { steps: 12 });
  await page.mouse.up();
}

function patchOf(page: Page, taskId: string) {
  return page.waitForResponse(
    (r) => r.url().endsWith(`/tasks/${taskId}`) && r.request().method() === "PATCH",
  );
}

test("signed out, the Gantt URL goes through login and back; the chart matches the server layout", async ({
  page,
  browser,
}) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GNT");
  const blocker = await createTask(page.request, wsId, project.id, {
    title: "Gantt blocker",
    startDate: day(10),
    dueDate: day(12),
  });
  const blocked = await createTask(page.request, wsId, project.id, {
    title: "Gantt blocked",
    startDate: day(20),
    dueDate: day(21),
  });
  const dep = await page.request.post(
    `/api/v1/workspaces/${wsId}/tasks/${blocker.id}/dependencies`,
    {
      data: { blockedId: blocked.id, type: "FS" },
    },
  );
  expect(dep.status()).toBe(200);

  // A fresh browser: direct URL, then the login page (UI), then back.
  const context = await browser.newContext({ viewport: { width: 1600, height: 900 } });
  const fresh = await context.newPage();
  const csp = watchCspViolations(fresh);
  const iconFetches = watchIconRequests(fresh);
  await fresh.goto(ganttUrl(project.key));
  await expect(fresh).toHaveURL(/\/login\?returnTo=/);
  expect(new URL(fresh.url()).searchParams.get("returnTo")).toBe(ganttUrl(project.key));
  await fresh.getByLabel("이메일").fill(admin.email);
  await fresh.getByLabel("비밀번호").fill(admin.password);
  await fresh.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(fresh).toHaveURL(
    new RegExp(`/w/${admin.workspaceSlug}/${project.key}/gantt\\?y=${String(Y)}&m=${String(M)}$`),
  );

  const chart = fresh.locator('[data-slot="gantt"]');
  await expect(chart).toBeVisible();
  await expect(chart).toHaveAttribute("data-bar-edit", "1");
  await expect(
    chart.locator(".fvoci-gantt__row-label", { hasText: "Gantt blocker" }),
  ).toBeVisible();
  await expect(
    chart.locator(".fvoci-gantt__row-label", { hasText: "Gantt blocked" }),
  ).toBeVisible();
  await expect(chart.locator("polyline.fvoci-gantt__link")).toHaveCount(1);
  await expect(fresh.locator('[data-slot="gantt-period"]')).toHaveText(
    `${String(Y)}년 ${String(M)}월`,
  );
  await expect(bar(fresh, blocker.id)).toHaveAttribute("data-start", day(10));
  await expect(bar(fresh, blocker.id)).toHaveAttribute("data-end", day(12));

  // The browser lays the chart out itself; it must agree with the server's
  // own pixel layout (pxPerDay 32, lane 48, rows) for the same month.
  const me = meSchema.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
  const server = layoutSchema.parse(
    await (
      await fresh.request.get(`/api/v1/workspaces/${wsId}/projects/${project.id}/task-layout`, {
        params: {
          year: Y,
          month: M,
          weekStartsOn: me.weekStartsOn === 1 ? 1 : 0,
          pxPerDay: 32,
          laneHeight: 48,
        },
      })
    ).json(),
  );
  expect(Number(await chart.getAttribute("data-px-per-day"))).toBe(server.scale.pxPerDay);
  for (const serverBar of server.bars) {
    const rect = bar(fresh, serverBar.id).locator(".fvoci-gantt__bar-rect");
    expect(Number(await rect.getAttribute("x"))).toBe(serverBar.x);
    expect(Number(await rect.getAttribute("width"))).toBe(serverBar.width);
    const y = Number(await rect.getAttribute("y"));
    const h = Number(await rect.getAttribute("height"));
    expect(y + h / 2).toBeCloseTo(serverBar.lane * 48 + 24, 6);
  }
  const serverPoints = required(server.paths[0]).slice(2).join(" ");
  const clientPoints = required(
    await chart.locator("polyline.fvoci-gantt__link").getAttribute("points"),
  )
    .split(" ")
    .map((n) => Math.round(Number(n)))
    .join(" ");
  expect(clientPoints).toBe(serverPoints);
  const serverOffDuty = server.columns.filter((c) => c.offDuty).map((c) => c.date);
  const clientOffDuty = await chart
    .locator(".fvoci-gantt__tick[data-off-duty]")
    .evaluateAll((els) => els.map((el) => el.getAttribute("data-date")));
  expect(clientOffDuty).toEqual(serverOffDuty);

  // A refresh reloads the Vue page through the server's SPA fallback.
  await fresh.reload();
  await expect(chart).toBeVisible();
  await expect(bar(fresh, blocked.id)).toHaveAttribute("data-start", day(20));

  expect(csp).toEqual([]);
  expect(iconFetches).toEqual([]);
  await context.close();
});

test("dragging a bar saves it; the dates survive a reload and a dueAt keeps its time of day", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  const csp = watchCspViolations(page);
  const iconFetches = watchIconRequests(page);
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GDR");
  const ranged = await createTask(page.request, wsId, project.id, {
    title: "Gantt drag persist",
    startDate: day(10),
    dueDate: day(14),
  });
  const timed = await createTask(page.request, wsId, project.id, {
    title: "Gantt timed due",
    startDate: day(4),
  });
  const withDueAt = await page.request.patch(`/api/v1/workspaces/${wsId}/tasks/${timed.id}`, {
    data: { dueAt: `${day(6)}T09:30:00.000Z` },
  });
  expect(withDueAt.ok()).toBe(true);
  const me = timezoneSchema.parse(await (await page.request.get("/api/v1/auth/me")).json());
  const localTime = (iso: string) =>
    new Intl.DateTimeFormat("en-GB", {
      timeZone: me.timezone,
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hourCycle: "h23",
    }).format(new Date(iso));

  await page.goto(ganttUrl(project.key));
  const chart = page.locator('[data-slot="gantt"]');
  await expect(chart).toHaveAttribute("data-bar-edit", "1");
  await expect(bar(page, ranged.id)).toHaveAttribute("data-start", day(10));

  // Move by five days: both dates move, expectedDates are the loaded ones.
  let patch = patchOf(page, ranged.id);
  await dragBy(page, ranged.id, 5);
  let response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    startDate: day(15),
    dueDate: day(19),
    expectedDates: { startDate: day(10), dueDate: day(14), dueAt: null },
  });
  await expect(bar(page, ranged.id)).toHaveAttribute("data-start", day(15));
  await expect(bar(page, ranged.id)).toHaveAttribute("data-end", day(19));
  await expect(chart).not.toHaveAttribute("aria-busy", "true");

  // The end handle changes only the due date.
  patch = patchOf(page, ranged.id);
  await dragBy(page, ranged.id, 2, "end");
  response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    dueDate: day(21),
    expectedDates: { startDate: day(15), dueDate: day(19), dueAt: null },
  });
  await expect(bar(page, ranged.id)).toHaveAttribute("data-end", day(21));
  await expect(chart).not.toHaveAttribute("aria-busy", "true");

  // Keyboard: a focused bar moves a day with ArrowRight.
  patch = patchOf(page, ranged.id);
  await bar(page, ranged.id).focus();
  await page.keyboard.press("ArrowRight");
  expect((await patch).status()).toBe(200);
  await expect(bar(page, ranged.id)).toHaveAttribute("data-start", day(16));
  await expect(bar(page, ranged.id)).toHaveAttribute("data-end", day(22));
  await expect(chart).not.toHaveAttribute("aria-busy", "true");

  // A dueAt moves by whole days in the user's time zone and keeps its time there.
  patch = patchOf(page, timed.id);
  await dragBy(page, timed.id, 3);
  response = await patch;
  expect(response.status()).toBe(200);
  const body = datePatchSchema.parse(response.request().postDataJSON());
  expect(body.startDate).toBe(day(7));
  expect(body.dueDate).toBeUndefined();
  expect(localTime(body.dueAt)).toBe(localTime(`${day(6)}T09:30:00.000Z`));
  expect(Date.parse(body.dueAt) - Date.parse(`${day(6)}T09:30:00.000Z`)).toBe(3 * 86_400_000);
  expect(body.expectedDates).toEqual({
    startDate: day(4),
    dueDate: null,
    dueAt: `${day(6)}T09:30:00.000Z`,
  });
  await expect(bar(page, timed.id)).toHaveAttribute("data-start", day(7));
  await expect(chart).not.toHaveAttribute("aria-busy", "true");

  await page.reload();
  await expect(bar(page, ranged.id)).toHaveAttribute("data-start", day(16));
  await expect(bar(page, ranged.id)).toHaveAttribute("data-end", day(22));
  await expect(bar(page, timed.id)).toHaveAttribute("data-start", day(7));
  const savedRanged = await getTask(page.request, wsId, ranged.id);
  expect([savedRanged.startDate, savedRanged.dueDate, savedRanged.dueAt]).toEqual([
    day(16),
    day(22),
    null,
  ]);
  const savedTimed = await getTask(page.request, wsId, timed.id);
  expect(savedTimed.startDate).toBe(day(7));
  expect(savedTimed.dueDate).toBeNull();
  expect(Date.parse(required(savedTimed.dueAt))).toBe(Date.parse(`${day(9)}T09:30:00.000Z`));

  expect(csp).toEqual([]);
  expect(iconFetches).toEqual([]);
});

test("handles write only the date of their own edge; a one-date bar has no handle on its date", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  const csp = watchCspViolations(page);
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GHD");
  const ranged = await createTask(page.request, wsId, project.id, {
    title: "Gantt start handle",
    startDate: day(10),
    dueDate: day(14),
  });
  const dueOnly = await createTask(page.request, wsId, project.id, {
    title: "Gantt due only",
    dueDate: day(20),
  });
  const startOnly = await createTask(page.request, wsId, project.id, {
    title: "Gantt start only",
    startDate: day(8),
  });

  await page.goto(ganttUrl(project.key));
  await expect(page.locator('[data-slot="gantt"]')).toHaveAttribute("data-bar-edit", "1");
  // A one-date bar offers only the handle of the edge it has no date for.
  await expect(bar(page, dueOnly.id).locator('[data-handle="start"]')).toHaveCount(1);
  await expect(bar(page, dueOnly.id).locator('[data-handle="end"]')).toHaveCount(0);
  await expect(bar(page, startOnly.id).locator('[data-handle="start"]')).toHaveCount(0);
  await expect(bar(page, startOnly.id).locator('[data-handle="end"]')).toHaveCount(1);

  // The start handle of a two-date task writes only the start.
  let patch = patchOf(page, ranged.id);
  await dragBy(page, ranged.id, -2, "start");
  let response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    startDate: day(8),
    expectedDates: { startDate: day(10), dueDate: day(14), dueAt: null },
  });
  await expect(bar(page, ranged.id)).toHaveAttribute("data-start", day(8));
  await expect(bar(page, ranged.id)).toHaveAttribute("data-end", day(14));
  await expect(page.locator('[data-slot="gantt"]')).not.toHaveAttribute("aria-busy", "true");

  // The start handle of a due-only task adds the start and nothing else.
  patch = patchOf(page, dueOnly.id);
  await dragBy(page, dueOnly.id, -3, "start");
  response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    startDate: day(17),
    expectedDates: { startDate: null, dueDate: day(20), dueAt: null },
  });
  await expect(bar(page, dueOnly.id)).toHaveAttribute("data-start", day(17));
  await expect(bar(page, dueOnly.id)).toHaveAttribute("data-end", day(20));
  await expect(page.locator('[data-slot="gantt"]')).not.toHaveAttribute("aria-busy", "true");

  // The end handle of a start-only task adds the due date and nothing else.
  patch = patchOf(page, startOnly.id);
  await dragBy(page, startOnly.id, 2, "end");
  response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    dueDate: day(10),
    expectedDates: { startDate: day(8), dueDate: null, dueAt: null },
  });
  await expect(bar(page, startOnly.id)).toHaveAttribute("data-start", day(8));
  await expect(bar(page, startOnly.id)).toHaveAttribute("data-end", day(10));

  await page.reload();
  await expect(bar(page, ranged.id)).toHaveAttribute("data-start", day(8));
  const saved = await Promise.all(
    [ranged, dueOnly, startOnly].map((task) => getTask(page.request, wsId, task.id)),
  );
  expect(saved.map((task) => [task.startDate, task.dueDate, task.dueAt])).toEqual([
    [day(8), day(14), null],
    [day(17), day(20), null],
    [day(8), day(10), null],
  ]);
  expect(csp).toEqual([]);
});

test("a task spanning the month's edges moves by the day asked", async ({ page }) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GSP");
  // Starts in February, before the first day the March chart shows.
  const early = await createTask(page.request, wsId, project.id, {
    title: "Gantt spans the start",
    startDate: `${String(Y)}-02-10`,
    dueDate: day(5),
  });
  // Ends in April, after the last day the March chart shows.
  const late = await createTask(page.request, wsId, project.id, {
    title: "Gantt spans the end",
    startDate: day(28),
    dueDate: `${String(Y)}-04-10`,
  });

  await page.goto(ganttUrl(project.key));
  const chart = page.locator('[data-slot="gantt"]');
  await expect(chart).toHaveAttribute("data-bar-edit", "1");
  await expect(bar(page, early.id)).toHaveAttribute("data-start", `${String(Y)}-02-10`);
  expect(
    Number(await bar(page, early.id).locator(".fvoci-gantt__bar-rect").getAttribute("x")),
  ).toBeLessThan(0);
  const layoutEnd = await chart.locator(".fvoci-gantt__tick").last().getAttribute("data-date");
  expect(required(layoutEnd) < `${String(Y)}-04-10`).toBe(true);

  // ArrowLeft moves it one day earlier, not to the first visible day.
  let patch = patchOf(page, early.id);
  await bar(page, early.id).focus();
  await page.keyboard.press("ArrowLeft");
  let response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    startDate: `${String(Y)}-02-09`,
    dueDate: day(4),
    expectedDates: { startDate: `${String(Y)}-02-10`, dueDate: day(5), dueAt: null },
  });
  await expect(bar(page, early.id)).toHaveAttribute("data-start", `${String(Y)}-02-09`);
  await expect(bar(page, early.id)).toHaveAttribute("data-end", day(4));
  await expect(chart).not.toHaveAttribute("aria-busy", "true");

  // ArrowRight on a bar past the last visible day moves it one day later.
  patch = patchOf(page, late.id);
  await bar(page, late.id).focus();
  await page.keyboard.press("ArrowRight");
  response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    startDate: day(29),
    dueDate: `${String(Y)}-04-11`,
    expectedDates: { startDate: day(28), dueDate: `${String(Y)}-04-10`, dueAt: null },
  });
  await expect(chart).not.toHaveAttribute("aria-busy", "true");

  // Shift+ArrowRight extends its end by a day, beyond the visible range.
  patch = patchOf(page, late.id);
  await bar(page, late.id).focus();
  await page.keyboard.press("Shift+ArrowRight");
  response = await patch;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({
    dueDate: `${String(Y)}-04-12`,
    expectedDates: { startDate: day(29), dueDate: `${String(Y)}-04-11`, dueAt: null },
  });
  await expect(bar(page, late.id)).toHaveAttribute("data-end", `${String(Y)}-04-12`);

  const saved = await Promise.all(
    [early, late].map((task) => getTask(page.request, wsId, task.id)),
  );
  expect(saved.map((task) => [task.startDate, task.dueDate])).toEqual([
    [`${String(Y)}-02-09`, day(4)],
    [day(29), `${String(Y)}-04-12`],
  ]);
});

test("a task changed elsewhere is not overwritten, and a broken dependency snaps back", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  const csp = watchCspViolations(page);
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GCF");
  const first = await createTask(page.request, wsId, project.id, {
    title: "Gantt first",
    startDate: day(10),
    dueDate: day(12),
  });
  const second = await createTask(page.request, wsId, project.id, {
    title: "Gantt second",
    startDate: day(17),
    dueDate: day(18),
  });
  const dep = await page.request.post(`/api/v1/workspaces/${wsId}/tasks/${first.id}/dependencies`, {
    data: { blockedId: second.id, type: "FS" },
  });
  expect(dep.status()).toBe(200);

  // No stream: the page must not learn of the other edit before the drag.
  await page.route(/\/api\/v1\/workspaces\/[^/]+\/projects\/[^/]+\/stream$/, (route) =>
    route.abort(),
  );
  await page.goto(ganttUrl(project.key));
  await expect(bar(page, first.id)).toHaveAttribute("data-end", day(12));

  const elsewhere = await page.request.patch(`/api/v1/workspaces/${wsId}/tasks/${first.id}`, {
    data: { dueDate: day(13) },
  });
  expect(elsewhere.ok()).toBe(true);

  const patch = patchOf(page, first.id);
  await dragBy(page, first.id, 2);
  const conflict = await patch;
  expect(conflict.status()).toBe(409);
  expect(errorSchema.parse(await conflict.json()).code).toBe("document_version_mismatch");
  await expect(page.getByRole("alert")).toContainText("다른 곳에서 먼저 수정되었습니다");
  // The layout is reloaded: the bar shows the other edit, not the drag.
  await expect(bar(page, first.id)).toHaveAttribute("data-start", day(10));
  await expect(bar(page, first.id)).toHaveAttribute("data-end", day(13));
  const unchanged = await getTask(page.request, wsId, first.id);
  expect([unchanged.startDate, unchanged.dueDate]).toEqual([day(10), day(13)]);
  await expect(page.locator('[data-slot="gantt"]')).not.toHaveAttribute("aria-busy", "true");

  // Moving the blocked task before its blocker ends is refused and nothing moves.
  const refused = patchOf(page, second.id);
  await dragBy(page, second.id, -6);
  const contradiction = await refused;
  expect(contradiction.status()).toBe(400);
  expect(errorSchema.parse(await contradiction.json()).code).toBe("dependency_contradiction");
  await expect(page.getByRole("alert")).toContainText("의존 관계가 서로 모순됩니다");
  await expect(bar(page, second.id)).toHaveAttribute("data-start", day(17));
  await expect(bar(page, second.id)).toHaveAttribute("data-end", day(18));
  const kept = await getTask(page.request, wsId, second.id);
  expect([kept.startDate, kept.dueDate]).toEqual([day(17), day(18)]);

  expect(csp).toEqual([]);
});

test("a view-only member gets no drag handles and the server refuses a forced change", async ({
  page,
  browser,
}) => {
  test.setTimeout(120_000);
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GVW", "private");
  const task = await createTask(page.request, wsId, project.id, {
    title: "Gantt view only",
    startDate: day(10),
    dueDate: day(12),
  });
  createE2eUser(viewer.email, viewer.password, viewer.givenName, {
    familyName: viewer.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "guest",
  });
  const viewerUser = memberListSchema
    .parse(await (await page.request.get(`/api/v1/workspaces/${wsId}/members`)).json())
    .items.find((m) => m.email === viewer.email);
  expect(viewerUser).toBeTruthy();
  assert(viewerUser);
  const added = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/members`,
    {
      data: { userId: required(viewerUser).userId, role: "viewer" },
    },
  );
  expect(added.status()).toBe(201);

  const context = await browser.newContext();
  const viewerPage = await context.newPage();
  const csp = watchCspViolations(viewerPage);
  await login(viewerPage, viewer.email, viewer.password);
  await viewerPage.goto(ganttUrl(project.key));
  const chart = viewerPage.locator('[data-slot="gantt"]');
  await expect(bar(viewerPage, task.id)).toHaveAttribute("data-start", day(10));
  await expect(chart).not.toHaveAttribute("data-bar-edit", "1");
  await expect(chart.locator(".fvoci-gantt__handle")).toHaveCount(0);

  const forced = await viewerPage.request.patch(`/api/v1/workspaces/${wsId}/tasks/${task.id}`, {
    data: {
      startDate: day(15),
      dueDate: day(17),
      expectedDates: { startDate: day(10), dueDate: day(12), dueAt: null },
    },
  });
  expect(forced.status()).toBe(404);
  const unchanged = await getTask(page.request, wsId, task.id);
  expect([unchanged.startDate, unchanged.dueDate]).toEqual([day(10), day(12)]);
  expect(csp).toEqual([]);
  await context.close();
});

test("an archived project's Gantt is read-only", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GRO");
  const task = await createTask(page.request, wsId, project.id, {
    title: "Archived gantt task",
    startDate: day(10),
    dueDate: day(14),
  });
  const archive = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/archive`,
  );
  expect(archive.ok()).toBe(true);
  await page.goto(ganttUrl(project.key));
  await expect(bar(page, task.id)).toBeVisible();
  const chart = page.locator('[data-slot="gantt"]');
  await expect(chart).not.toHaveAttribute("data-bar-edit", "1");
  await expect(chart.locator(".fvoci-gantt__handle")).toHaveCount(0);
});

test("the Gantt page loads none of the wiki editor's code or styles", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GLD");
  const task = await createTask(page.request, wsId, project.id, {
    title: "Gantt load",
    startDate: day(3),
    dueDate: day(4),
  });
  await page.goto(ganttUrl(project.key));
  await expect(bar(page, task.id)).toBeVisible();
  // Every script and stylesheet this document loaded (the boot module, the
  // Vue app, the Gantt page chunk and what they import). The wiki editor
  // (Tiptap/ProseMirror, Yjs, the collab provider) and its .fvoci-editor
  // styles are the wiki page's chunk only.
  const assets = await page.evaluate(() =>
    performance
      .getEntriesByType("resource")
      .map((entry) => new URL(entry.name).pathname)
      .filter((path) => /^\/assets\/[^/]+\.(js|css)$/.test(path)),
  );
  expect(assets.some((path) => path.endsWith(".js"))).toBe(true);
  expect(assets.some((path) => path.endsWith(".css"))).toBe(true);
  const withEditor: string[] = [];
  for (const path of assets) {
    const res = await page.request.get(path);
    expect(res.ok()).toBe(true);
    const body = await res.text();
    if (body.includes("ProseMirror") || body.includes("fvoci-editor")) withEditor.push(path);
  }
  expect(withEditor).toEqual([]);
});

test("a project list that fails to load offers a retry", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GRT");
  const task = await createTask(page.request, wsId, project.id, {
    title: "Gantt retry",
    startDate: day(3),
    dueDate: day(4),
  });
  let failList = true;
  await page.route(/\/api\/v1\/workspaces\/[^/]+\/projects(\?[^/]*)?$/, (route) => {
    if (!failList || route.request().method() !== "GET") return route.continue();
    return route.fulfill({
      status: 503,
      contentType: "application/problem+json",
      body: JSON.stringify({ type: "about:blank", title: "Service Unavailable", status: 503 }),
    });
  });
  await page.goto(ganttUrl(project.key));
  await expect(page.getByRole("alert")).toContainText("불러오지 못했습니다");
  failList = false;
  await page.getByRole("button", { name: "다시 시도" }).click();
  await expect(bar(page, task.id)).toHaveAttribute("data-start", day(3));
});

test("Vue project tabs and the workspace project list stay in one runtime", async ({ page }) => {
  test.setTimeout(120_000);
  const csp = watchCspViolations(page);
  await ensureSetup(page);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "GNV");
  await createTask(page.request, wsId, project.id, {
    title: "Gantt nav",
    startDate: day(3),
    dueDate: day(4),
  });
  const marker = () => page.evaluate(() => window.__sameDocument);
  const markDocument = () =>
    page.evaluate(() => {
      window.__sameDocument = true;
    });

  await page.goto(ganttUrl(project.key));
  await expect(page.locator('[data-slot="gantt"]')).toBeVisible();
  await markDocument();
  await page.locator('[data-slot="project-link"]').click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/${project.key}$`));
  await expect(page.getByRole("heading", { level: 1, name: project.name })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  expect(await marker()).toBeUndefined();

  await page.goto(`/w/${admin.workspaceSlug}/${project.key}/tasks`);
  await expect(page.getByRole("heading", { level: 1, name: project.name })).toBeVisible();
  await markDocument();
  await page.getByRole("link", { name: "간트", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/${project.key}/gantt$`));
  await expect(
    page.locator('[data-slot="gantt"]').or(page.locator('[data-slot="gantt-empty"]')),
  ).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  expect(await marker(), "tasks → Gantt stays in the Vue app").toBe(true);

  // The tasks tab also stays in the Vue app.
  await markDocument();
  await page.getByRole("link", { name: "태스크", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/${project.key}/tasks$`));
  await expect(page.getByRole("heading", { level: 1, name: project.name })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  expect(await marker(), "Gantt → tasks stays in the Vue app").toBe(true);

  // The workspace project list is a connected Vue page.
  await markDocument();
  await page.locator(`a[href="/w/${admin.workspaceSlug}/projects"]`).first().click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/projects$`));
  await expect(page.getByRole("button", { name: "새 프로젝트", exact: true })).toBeVisible();
  await expect(page.getByText(project.name, { exact: true })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  expect(await marker(), "project list navigation stays in Vue").toBe(true);
  expect(csp).toEqual([]);
});

function required<T>(value: T | null | undefined): T {
  assert(value !== null && value !== undefined, "Expected fixture value to exist");
  return value;
}

declare global {
  interface Window {
    __sameDocument?: boolean;
  }
}
