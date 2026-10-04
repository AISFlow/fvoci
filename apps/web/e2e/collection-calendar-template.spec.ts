import assert from "node:assert/strict";
import { z } from "zod";
import { expect, test, type BrowserContext, type Page, type Response } from "@playwright/test";
import { login } from "./helpers";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

// Validate the response fields used by this flow; retain the complete payload.
const workspaceListSchema = z
  .object({ items: z.array(z.object({ id: z.string(), slug: z.string() }).passthrough()) })
  .passthrough();
const projectSchema = z
  .object({ id: z.string(), key: z.string(), rootDocumentId: z.string().nullable() })
  .passthrough();
const workflowSchema = z
  .object({
    id: z.string(),
    statuses: z.array(
      z.object({ id: z.string(), name: z.string(), category: z.string() }).passthrough(),
    ),
  })
  .passthrough();
const taskSchema = z.object({ id: z.string(), number: z.number() }).passthrough();
const taskDatesSchema = z
  .object({
    id: z.string(),
    startDate: z.string().nullable(),
    dueDate: z.string().nullable(),
    dueAt: z.string().nullable(),
  })
  .passthrough();
const idSchema = z.object({ id: z.string() }).passthrough();
const versionedSchema = z.object({ id: z.string(), version: z.number() }).passthrough();
const collectionQuerySchema = z
  .object({
    items: z.array(
      z
        .object({
          id: z.string(),
          taskId: z.string().nullable(),
          version: z.number(),
          values: z.record(z.unknown()),
        })
        .passthrough(),
    ),
  })
  .passthrough();

test.describe.configure({ mode: "serial" });
async function fixture(page: Page, key: string) {
  const observedAssets: { url: string; path: string; status: number }[] = [];
  const bodylessAssets: { path: string; status: number; redirectedFrom: string | null }[] = [];
  const captureAsset = (response: Response) => {
    const path = new URL(response.url()).pathname;
    if (path.startsWith("/assets/") && /\.(js|css)$/.test(path)) {
      const status = response.status();
      if (status >= 300 && status < 400) {
        const previous = response.request().redirectedFrom();
        bodylessAssets.push({
          path,
          status,
          redirectedFrom: previous ? new URL(previous.url()).pathname : null,
        });
        return;
      }
      // Only retain transport metadata here. Setup/auth can replace the
      // document before Chromium's deferred getResponseBody completes.
      observedAssets.push({ url: response.url(), path, status });
    }
  };
  page.on("response", captureAsset);
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("달력");
    await page.getByLabel("이메일").fill("calendar@example.com");
    await page.getByLabel("비밀번호").fill("supersecret1");
    await page.getByLabel("워크스페이스 이름").fill("Calendar template");
    await page.getByLabel("주소(영문)").fill("caltemplate");
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else if (await page.getByRole("button", { name: "로그인", exact: true }).count())
    await login(page, "calendar@example.com", "supersecret1");
  const ws = workspaceListSchema
    .parse(await (await page.request.get("/api/v1/me/workspaces")).json())
    .items.find((w: { slug: string }) => w.slug === "caltemplate");
  assert(ws, "Calendar workspace must exist");
  const base = `/api/v1/workspaces/${ws.id}`;
  expect(
    (
      await page.request.patch("/api/v1/auth/me", {
        data: { givenName: "달력", timezone: "America/New_York" },
      })
    ).ok(),
  ).toBe(true);
  const res = await page.request.post(`${base}/projects`, {
    data: { key, name: key, visibility: "workspace" },
  });
  expect(res.status()).toBe(201);
  const project = projectSchema.parse(await res.json());
  const wf = workflowSchema.parse(
    await (await page.request.get(`${base}/projects/${project.id}/workflow`)).json(),
  );
  async function task(title: string, dates: object) {
    const created = await page.request.post(`${base}/projects/${project.id}/tasks`, {
      data: { title, type: "task", statusId: required(wf.statuses[0]).id, ...dates },
    });
    expect(created.status()).toBe(201);
    const row = taskSchema.parse(await created.json());
    return { ...row, displayId: `${key}-${String(row.number)}` };
  }
  async function stored(id: string) {
    const res = await page.request.get(`${base}/tasks/${id}`);
    expect(res.ok()).toBe(true);
    return taskDatesSchema.parse(await res.json());
  }
  async function open(month = "2027-05") {
    await page.goto(`/w/caltemplate/${key}/calendar`);
    await expect(page).toHaveURL(`/w/caltemplate/${key}/calendar`);
    await expect(page.locator("[data-v-app]")).toHaveCount(1);
    await page.locator('input[type="month"]').fill(month);
    await expect(page.locator('table[data-testid="collection-calendar"]')).toBeVisible();
    page.off("response", captureAsset);
    const assets: { path: string; sha256: string; bytes: number; loadedStatus: number }[] = [];
    const measured = new Set<string>();
    for (const asset of observedAssets) {
      expect(new URL(asset.url).origin, `loaded asset ${asset.path} origin`).toBe(
        new URL(page.url()).origin,
      );
      expect(asset.status, `loaded asset ${asset.path} status`).toBeGreaterThanOrEqual(200);
      expect(asset.status, `loaded asset ${asset.path} status`).toBeLessThan(300);
      if (measured.has(asset.url)) continue;
      // This measures a fresh HTTP response for an observed loaded URL, not
      // the original browser body. APIRequestContext owns these bytes across
      // page navigation and bypasses Chromium's cache/CDP retention.
      const response = await page.request.get(asset.url, { maxRedirects: 0 });
      expect(response.ok(), `served asset ${asset.path} status ${String(response.status())}`).toBe(
        true,
      );
      const body = await response.body();
      expect(body.length, `served asset ${asset.path} has a body`).toBeGreaterThan(0);
      const sha256 = createHash("sha256").update(body).digest("hex");
      const candidate = readFileSync(new URL(`../dist${asset.path}`, import.meta.url));
      expect(sha256, `served asset ${asset.path} matches candidate dist`).toBe(
        createHash("sha256").update(candidate).digest("hex"),
      );
      assets.push({
        path: asset.path.slice(1),
        sha256,
        bytes: body.length,
        loadedStatus: asset.status,
      });
      measured.add(asset.url);
      await response.dispose();
    }
    expect(assets.length, "successful terminal asset responses were captured").toBeGreaterThan(0);
    expect(
      assets.some((asset) => asset.path.endsWith(".js")),
      "candidate JS was served",
    ).toBe(true);
    expect(
      assets.some((asset) => asset.path.endsWith(".css")),
      "candidate CSS was served",
    ).toBe(true);
    writeFileSync(
      `/tmp/fvoci-front272-calendar-served-${key}.json`,
      JSON.stringify(
        {
          head: process.env.FVOCI_CALENDAR_VERIFY_HEAD ?? "unbound",
          url: new URL(page.url()).pathname,
          assets,
          bodylessAssets,
          measurement: "http-refetch-of-observed-loaded-url",
          originalBrowserBody: "NOTMEASURED",
        },
        null,
        2,
      ),
    );
  }
  return { base, project, task, stored, open };
}

test("template day/week/month, mini calendar, keyboard and responsive sidebar connect to real timed task API", async ({
  page,
}) => {
  const f = await fixture(page, "VIEW");
  const timed = await f.task("Point deadline", {});
  expect(
    (
      await page.request.patch(`${f.base}/tasks/${timed.id}`, {
        data: { dueAt: "2027-05-05T13:30:00Z" },
      })
    ).ok(),
  ).toBe(true);
  await f.open();
  await page.screenshot({ path: "/tmp/fvoci-front272-calendar-month.png" });
  await page.getByTestId("calendar-mini").getByLabel("2027-05-05", { exact: true }).click();
  await page.getByRole("tab", { name: "Week", exact: true }).click();
  const grid = page.getByTestId("calendar-time-grid");
  await expect(grid.getByTestId(`collection-preview-${timed.displayId}`)).toContainText("09:30");
  await page.screenshot({ path: "/tmp/fvoci-front272-calendar-week.png" });
  await grid
    .getByTestId(`collection-preview-${timed.displayId}`)
    .dragTo(grid.locator('[data-calendar-target="2027-05-06"][data-hour="14"]'));
  await expect.poll(async () => (await f.stored(timed.id)).dueAt).toBe("2027-05-06T18:30:00Z");
  expect((await f.stored(timed.id)).dueDate).toBeNull();
  await page.getByTestId("calendar-mini").getByLabel("2027-05-06", { exact: true }).click();
  await page.getByRole("tab", { name: "Day", exact: true }).click();
  await expect(grid.getByTestId(`collection-preview-${timed.displayId}`)).toBeVisible();
  await page.locator(".template-calendar").focus();
  await page.keyboard.press("m");
  await expect(page.locator('table[data-testid="collection-calendar"]')).toBeVisible();
  await page.keyboard.press("w");
  await expect(grid).toBeVisible();
  await page.keyboard.press("d");
  await expect(page.getByRole("tab", { name: "Day", exact: true })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await page.keyboard.press("ArrowRight");
  await expect(grid.getByRole("button", { name: "2027-05-07", exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByTestId("calendar-mini")).toBeHidden();
  await page.getByRole("button", { name: "Calendar sidebar" }).click();
  await expect(page.getByTestId("calendar-mini")).toBeVisible();
  await page.getByTestId("calendar-mini").getByLabel("2027-05-06", { exact: true }).click();
  await expect(page.getByTestId("calendar-mini")).toBeHidden();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test("real date interval resize and optimistic pending settle; failed concurrent write rolls back and refetches", async ({
  page,
}) => {
  const f = await fixture(page, "RANGE");
  const range = await f.task("Stored range", { startDate: "2027-05-05", dueDate: "2027-05-07" });
  const point = await f.task("Single date", { dueDate: "2027-05-08" });
  await f.open();
  await expect(page.getByRole("button", { name: "Resize end · Single date" })).toHaveCount(0);
  const { promise: gate, resolve: release } = deferred();
  await page.route(`**${f.base}/tasks/${range.id}`, async (route) => {
    if (route.request().method() === "PATCH") await gate;
    await route.continue();
  });
  await page
    .getByRole("button", { name: "Resize end · Stored range" })
    .dragTo(page.locator('td[data-date="2027-05-09"]'));
  await expect(
    page.locator('td[data-date="2027-05-09"]').getByTestId(`collection-preview-${range.displayId}`),
  ).toBeVisible();
  await expect(page.getByTestId(`collection-preview-${range.displayId}`)).toHaveAttribute(
    "aria-busy",
    "true",
  );
  expect((await f.stored(range.id)).dueDate).toBe("2027-05-07");
  release();
  await expect.poll(async () => (await f.stored(range.id)).dueDate).toBe("2027-05-09");
  await expect(page.getByTestId(`collection-preview-${range.displayId}`)).not.toHaveAttribute(
    "aria-busy",
    "true",
  );
  await page.unroute(`**${f.base}/tasks/${range.id}`);
  await page
    .getByRole("button", { name: "Resize start · Stored range" })
    .dragTo(page.locator('td[data-date="2027-05-06"]'));
  await expect.poll(async () => (await f.stored(range.id)).startDate).toBe("2027-05-06");
  // Snapshot in editor holds its expectedDates while another real client commits.
  await page.getByTestId(`collection-preview-${point.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  await editor.locator('input[type="date"]').fill("2027-05-12");
  expect(
    (
      await page.request.patch(`${f.base}/tasks/${point.id}`, { data: { dueDate: "2027-05-10" } })
    ).ok(),
  ).toBe(true);
  const conflict = page.waitForResponse(
    (r) => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${point.id}`),
  );
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  expect((await conflict).status()).toBe(409);
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-12");
  await expect(
    page.locator('td[data-date="2027-05-10"]').getByTestId(`collection-preview-${point.displayId}`),
  ).toBeVisible();
  await expect(
    page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${point.displayId}`),
  ).toHaveCount(0);
  await editor.getByRole("button", { name: "최신 저장 뷰 불러오기" }).click();
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await f.stored(point.id)).dueDate).toBe("2027-05-12");
  await expect(editor).toHaveCount(0);
  await page.getByTestId(`collection-preview-${point.displayId}`).click();
  await editor.getByLabel("마감 시각", { exact: true }).check();
  // Converting a plain date requires the user's time; no invented 09:00 default.
  await expect(editor.locator('input[type="datetime-local"]')).toHaveValue("");
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect(editor.getByRole("alert")).toBeVisible();
  expect((await f.stored(point.id)).dueDate).toBe("2027-05-12");
  await editor.locator('input[type="datetime-local"]').fill("2027-05-12T10:45");
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await f.stored(point.id)).dueAt).toBe("2027-05-12T14:45:00Z");
  expect((await f.stored(point.id)).dueDate).toBeNull();
});

test("offline reconnect preserves editor intent, refuses unsaved writes, and DST gap/fold stays in existing date contract", async ({
  page,
  context,
}) => {
  await offlineCalendarScenario(page, context, "DST");
});

test("cached fields SSE refetch failure preserves the same Calendar draft through reconnect", async ({
  page,
  context,
}) => {
  await offlineCalendarScenario(page, context, "REFETCH", true);
});

test("offline fields retry interaction keeps the Calendar draft through reconnect and explicit save", async ({
  page,
  context,
}) => {
  await offlineCalendarScenario(page, context, "OFFLINERETRY", true, true);
});

for (const metadata of ["collection", "views"] as const)
  test(`cached ${metadata} transport failure keeps the Calendar draft through owned retry and reconnect`, async ({
    page,
    context,
  }) => {
    await offlineCalendarScenario(
      page,
      context,
      metadata === "collection" ? "COLREFRESH" : "VIEWREFRESH",
      true,
      true,
      metadata,
    );
  });

test("online fields transport failure keeps the Calendar draft while visible retry initiates a real GET", async ({
  page,
}) => {
  const f = await fixture(page, "RETRY");
  const point = await f.task("Retry point", { dueDate: "2027-05-08" });
  await f.open();
  await page.getByTestId(`collection-preview-${point.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  const input = editor.locator('input[type="date"]');
  const popover = page.locator('[data-slot="content"]').filter({ has: editor });
  await input.fill("2027-05-12");
  const node = required(await input.elementHandle());
  const collection = idSchema.parse(
    await (await page.request.get(`${f.base}/projects/${f.project.id}/collection`)).json(),
  );
  const path = `${f.base}/collections/${collection.id}/fields`;
  const failure = page.waitForEvent("requestfailed", {
    predicate: (request) => request.url().endsWith(path),
  });
  await page.route(`**${path}`, (route) => route.abort("failed"));
  expect(
    (
      await page.request.patch(`${f.base}/tasks/${point.id}`, {
        data: { title: "Retry point updated by peer" },
      })
    ).ok(),
  ).toBe(true);
  expect((await failure).failure()?.errorText).toContain("ERR_FAILED");
  const error = fieldsErrorLocator(page);
  await expect(error).toBeVisible();
  await expect(input).toHaveValue("2027-05-12");
  expect(await page.evaluate(() => navigator.onLine)).toBe(true);
  await page.unroute(`**${path}`);
  const { promise: retryStarted, resolve: markRetryStarted } = deferred();
  const { promise: retryGate, resolve: releaseRetry } = deferred();
  await page.route(`**${path}`, async (route) => {
    markRetryStarted();
    await retryGate;
    await route.continue();
  });
  const recovered = page.waitForResponse((response) => response.url().endsWith(path));
  await error.getByRole("button", { name: "다시 시도" }).click();
  await retryStarted;
  try {
    // Keep the error region mounted until outside pointer/focus handling has
    // settled. A fast200 must not conceal an unintended popover dismissal.
    await expect(error).toBeVisible();
    await expect(popover).toHaveAttribute("data-state", "open");
    await expect(editor).toBeVisible();
    await expect(input).toHaveValue("2027-05-12");
    expect(await input.evaluate((current, prior) => current === prior, node)).toBe(true);
  } finally {
    releaseRetry();
  }
  expect((await recovered).status()).toBe(200);
  await expect(error).toHaveCount(0);
  await expect(popover).toHaveAttribute("data-state", "open");
  await expect(editor).toBeVisible();
  await expect(input).toHaveValue("2027-05-12");
  expect(await input.evaluate((current, prior) => current === prior, node)).toBe(true);
  await expect(editor.getByRole("button", { name: "저장 뷰 저장" })).toBeEnabled();
  expect((await f.stored(point.id)).dueDate).toBe("2027-05-08");
});

function fieldsErrorLocator(page: Page) {
  // CollectionContents renders the cached-fields error immediately before its
  // toolbar. Rows can independently fail offline and render another direct alert;
  // target the fields retry owner without hiding or conflating those two errors.
  return page.locator(
    'section[data-testid="collection-calendar"] > [role="alert"]:has(+ .collection-toolbar)',
  );
}

async function offlineCalendarScenario(
  page: Page,
  context: BrowserContext,
  key: string,
  fieldsBarrier = false,
  offlineRetry = false,
  metadata: "fields" | "collection" | "views" = "fields",
): Promise<void> {
  const f = await fixture(page, key);
  const point = await f.task("DST point", {});
  expect(
    (
      await page.request.patch(`${f.base}/tasks/${point.id}`, {
        data: { dueAt: "2026-11-01T06:30:00Z" },
      })
    ).ok(),
  ).toBe(true);
  await f.open("2026-11");
  await page.getByTestId(`collection-preview-${point.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  const input = editor.locator('input[type="datetime-local"]');
  const popover = page.locator('[data-slot="content"]').filter({ has: editor });
  await expect(input).toHaveValue("2026-11-01T01:30");
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect(editor).toHaveCount(0);
  expect((await f.stored(point.id)).dueAt).toBe("2026-11-01T06:30:00Z");
  await page.getByTestId(`collection-preview-${point.displayId}`).click();
  await input.fill("2026-03-08T02:30");
  let patches = 0;
  page.on("request", (req) => {
    if (req.method() === "PATCH" && req.url().endsWith(`/tasks/${point.id}`)) patches++;
  });
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect(editor.getByRole("alert")).toBeVisible();
  expect(patches).toBe(0);
  await input.fill("2026-11-02T09:30");
  let afterOffline: (() => Promise<void>) | undefined;
  let afterReconnect: (() => Promise<void>) | undefined;
  if (fieldsBarrier) {
    const draftNode = required(await input.elementHandle());
    const collection = idSchema.parse(
      await (await page.request.get(`${f.base}/projects/${f.project.id}/collection`)).json(),
    );
    const fieldsPath =
      metadata === "collection"
        ? `${f.base}/projects/${f.project.id}/collection`
        : `${f.base}/collections/${collection.id}/${metadata}`;
    const { promise: fieldsStarted, resolve: markFieldsStarted } = deferred();
    const { promise: fieldsGate, resolve: releaseFields } = deferred();
    await page.route(`**${fieldsPath}`, async (route) => {
      markFieldsStarted();
      await fieldsGate;
      await route.continue();
    });
    // Correlate requests intercepted after these gates are installed, not any
    // historical200. Siblings always fetch genuine Rust responses, including a
    // later invalidation round: only the selected request uses browser offline.
    const siblingPaths = [
      `${f.base}/collections/${collection.id}/fields`,
      `${f.base}/projects/${f.project.id}/collection`,
      `${f.base}/collections/${collection.id}/views`,
      `${f.base}/collections/${collection.id}/query`,
    ].filter((path) => path !== fieldsPath);
    const siblings = siblingPaths.map((path) => {
      const { promise, resolve } = deferred();
      return { path, promise, resolve };
    });
    for (const sibling of siblings)
      await page.route(`**${sibling.path}`, async (route) => {
        const response = await route.fetch();
        expect(response.status()).toBe(200);
        await route.fulfill({ response });
        sibling.resolve();
      });
    expect(
      (
        await page.request.patch(`${f.base}/tasks/${point.id}`, {
          data: { title: "DST point updated by peer" },
        })
      ).ok(),
    ).toBe(true);
    await fieldsStarted;
    await Promise.all(siblings.map((sibling) => sibling.promise));
    const fieldsFailed = page.waitForEvent("requestfailed", {
      predicate: (request) => request.url().endsWith(fieldsPath),
    });
    afterOffline = async () => {
      releaseFields();
      expect((await fieldsFailed).failure()?.errorText).toContain("ERR_INTERNET_DISCONNECTED");
      await expect(input).toHaveValue("2026-11-02T09:30");
      await expect(popover).toHaveAttribute("data-state", "open");
      await expect(editor).toBeVisible();
      expect(await input.evaluate((node, previous) => node === previous, draftNode)).toBe(true);
      const fieldsError =
        metadata === "collection"
          ? page.locator('[role="alert"]:has(+ section[data-testid="collection-calendar"])')
          : fieldsErrorLocator(page);
      await expect(fieldsError).toBeVisible();
      await expect(fieldsError.getByRole("button", { name: "다시 시도" })).toBeVisible();
      expect(patches).toBe(0);
      await page.unroute(`**${fieldsPath}`);
      for (const sibling of siblings) await page.unroute(`**${sibling.path}`);
      const recovered = page.waitForResponse(
        (response) => response.url().endsWith(fieldsPath) && response.ok(),
      );
      afterReconnect = async () => {
        expect((await recovered).status()).toBe(200);
        await expect(fieldsError).toHaveCount(0);
        await expect(popover).toHaveAttribute("data-state", "open");
        await expect(editor).toBeVisible();
        expect(await input.evaluate((node, previous) => node === previous, draftNode)).toBe(true);
      };
      // This checks outside-interaction ownership; the separate online case
      // proves retry GET causality without automatic reconnect fetching.
      if (offlineRetry) await fieldsError.getByRole("button", { name: "다시 시도" }).click();
      await expect(popover).toHaveAttribute("data-state", "open");
      await expect(editor).toBeVisible();
    };
  }
  await context.setOffline(true);
  await afterOffline?.();
  await expect(
    page.getByRole("status").filter({ hasText: "Offline · unsaved drafts" }),
  ).toBeVisible();
  await expect(editor.getByRole("button", { name: "저장 뷰 저장" })).toBeDisabled();
  // Independent HTTP client commits while the browser is offline. Reconnect may
  // refresh the grid but must retain both draft intent and the old conflict guard.
  expect(
    (
      await page.request.patch(`${f.base}/tasks/${point.id}`, {
        data: { dueAt: "2026-11-03T14:30:00Z" },
      })
    ).ok(),
  ).toBe(true);
  await context.setOffline(false);
  await expect(input).toHaveValue("2026-11-02T09:30");
  await afterReconnect?.();
  const saved = page.waitForResponse(
    (r) => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${point.id}`),
  );
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  expect((await saved).status()).toBe(409);
  await expect(editor.getByRole("alert")).toBeVisible();
  await expect(input).toHaveValue("2026-11-02T09:30");
  await expect(
    page.locator('td[data-date="2026-11-03"]').getByTestId(`collection-preview-${point.displayId}`),
  ).toBeVisible();
  await editor.getByRole("button", { name: "최신 저장 뷰 불러오기" }).click();
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await f.stored(point.id)).dueAt).toBe("2026-11-02T14:30:00Z");
  await page.reload();
  await page.locator('input[type="month"]').fill("2026-11");
  await expect(
    page.locator('td[data-date="2026-11-02"]').getByTestId(`collection-preview-${point.displayId}`),
  ).toBeVisible();
  const browser = required(context.browser());
  const fresh = await browser.newContext({ storageState: await context.storageState() });
  try {
    const freshPage = await fresh.newPage();
    await freshPage.goto(page.url());
    await freshPage.locator('input[type="month"]').fill("2026-11");
    await expect(freshPage.getByTestId(`collection-preview-${point.displayId}`)).toBeVisible();
    const response = await fresh.request.get(
      `${new URL(page.url()).origin}${f.base}/tasks/${point.id}`,
    );
    expect(response.status()).toBe(200);
    expect(taskDatesSchema.parse(await response.json())).toMatchObject({
      id: point.id,
      dueAt: "2026-11-02T14:30:00Z",
    });
  } finally {
    await fresh.close();
  }
}

test("custom date editor retains stale item guard, rolls back conflict and preserves draft for explicit retry", async ({
  page,
}) => {
  const f = await fixture(page, "VALUE");
  const item = await f.task("Custom calendar date", {});
  const collection = idSchema.parse(
    await (await page.request.get(`${f.base}/projects/${f.project.id}/collection`)).json(),
  );
  const created = await page.request.post(`${f.base}/collections/${collection.id}/fields`, {
    data: { name: "Custom date", key: "custom_date", type: "date" },
  });
  expect(created.status()).toBe(201);
  const field = versionedSchema.parse(await created.json());
  const query = async () => {
    const res = await page.request.post(`${f.base}/collections/${collection.id}/query`, {
      data: {
        config: { query: { filters: {}, sort: [] }, dateBy: null, groupBy: null },
        limit: 100,
      },
    });
    expect(res.ok()).toBe(true);
    const row = collectionQuerySchema
      .parse(await res.json())
      .items.find((r) => r.taskId === item.id);
    return required(row);
  };
  const valuePath = `${f.base}/collections/${collection.id}/items/${(await query()).id}/values`;
  async function put(date: string) {
    const row = await query();
    const res = await page.request.put(valuePath, {
      data: {
        fieldId: field.id,
        expectedFieldVersion: field.version,
        expectedVersion: row.version,
        value: { date },
      },
    });
    expect(res.ok()).toBe(true);
  }
  await put("2027-05-08");
  await f.open();
  await page.getByLabel("날짜 기준", { exact: true }).selectOption({ label: "Custom date" });
  await page.getByTestId(`collection-preview-${item.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  await editor.locator('input[type="date"]').fill("2027-05-12");
  const snapshotVersion = (await query()).version;
  await put("2027-05-10");
  const conflict = page.waitForResponse(
    (r) => r.request().method() === "PUT" && r.url().endsWith(valuePath),
  );
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  const response = await conflict;
  expect(response.status()).toBe(409);
  expect(response.request().postDataJSON()).toMatchObject({
    expectedVersion: snapshotVersion,
    expectedFieldVersion: field.version,
    value: { date: "2027-05-12" },
  });
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-12");
  await expect(
    page.locator('td[data-date="2027-05-10"]').getByTestId(`collection-preview-${item.displayId}`),
  ).toBeVisible();
  await expect(
    page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${item.displayId}`),
  ).toHaveCount(0);
  expect((await query()).values[field.id]).toEqual({ date: "2027-05-10" });
  await editor.getByRole("button", { name: "최신 저장 뷰 불러오기" }).click();
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await query()).values[field.id]).toEqual({ date: "2027-05-12" });
});

test("archive during pending calendar write is refused by real Rust permission guard and rolls back", async ({
  page,
}) => {
  const f = await fixture(page, "DENY");
  const item = await f.task("Permission changes", { dueDate: "2027-05-08" });
  await f.open();
  await page.getByTestId(`collection-preview-${item.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  await editor.locator('input[type="date"]').fill("2027-05-12");
  const { promise: gate, resolve: release } = deferred();
  await page.route(`**${f.base}/tasks/${item.id}`, async (route) => {
    if (route.request().method() === "PATCH") await gate;
    await route.continue();
  });
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect(
    page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${item.displayId}`),
  ).toHaveAttribute("aria-busy", "true");
  expect((await page.request.post(`${f.base}/projects/${f.project.id}/archive`)).ok()).toBe(true);
  const refused = page.waitForResponse(
    (r) => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${item.id}`),
  );
  release();
  expect((await refused).status()).toBe(409);
  await expect(
    page.locator('td[data-date="2027-05-08"]').getByTestId(`collection-preview-${item.displayId}`),
  ).toBeVisible();
  await expect(
    page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${item.displayId}`),
  ).toHaveCount(0);
  expect((await f.stored(item.id)).dueDate).toBe("2027-05-08");
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-12");
  await expect(editor.getByRole("button", { name: "저장 뷰 저장" })).toBeDisabled();
});

test("dual due fields keep Rust date precedence on unchanged edit; resize click and keyboard keep endpoint constraints", async ({
  page,
}) => {
  const f = await fixture(page, "EDGE");
  const dual = await f.task("Both due fields", { dueDate: "2027-05-08" });
  expect(
    (
      await page.request.patch(`${f.base}/tasks/${dual.id}`, {
        data: { dueAt: "2027-05-20T13:30:00Z" },
      })
    ).ok(),
  ).toBe(true);
  const range = await f.task("Keyboard resize", { startDate: "2027-05-05", dueDate: "2027-05-07" });
  await f.open();
  await expect(
    page.locator('td[data-date="2027-05-08"]').getByTestId(`collection-preview-${dual.displayId}`),
  ).toBeVisible();
  await page.getByTestId(`collection-preview-${dual.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-08");
  await expect(editor.getByLabel("마감 시각", { exact: true })).not.toBeChecked();
  const unchanged = page.waitForResponse(
    (r) => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${dual.id}`),
  );
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  const response = await unchanged;
  expect(response.ok()).toBe(true);
  expect(response.request().postDataJSON()).not.toHaveProperty("dueAt");
  expect((await f.stored(dual.id)).dueDate).toBe("2027-05-08");
  expect((await f.stored(dual.id)).dueAt).toBe("2027-05-20T13:30:00Z");
  await expect(
    page.locator('td[data-date="2027-05-08"]').getByTestId(`collection-preview-${dual.displayId}`),
  ).not.toHaveAttribute("aria-busy", "true");
  await page.getByRole("button", { name: "Resize end · Keyboard resize" }).click();
  const date = editor.locator('input[type="date"]');
  await expect(editor.getByLabel("마감 시각", { exact: true })).toHaveCount(0);
  await date.fill("2027-05-04");
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  expect(await date.evaluate((node: HTMLInputElement) => node.validity.rangeUnderflow)).toBe(true);
  expect((await f.stored(range.id)).dueDate).toBe("2027-05-07");
  await date.fill("2027-05-09");
  await date.press("Enter");
  await expect.poll(async () => (await f.stored(range.id)).dueDate).toBe("2027-05-09");
  await expect(editor).toHaveCount(0);
  const start = page.getByRole("button", { name: "Resize start · Keyboard resize" });
  await expect(start).toBeEnabled();
  await start.focus();
  await start.press("Enter");
  await expect(date).toHaveValue("2027-05-05");
  await date.fill("2027-05-10");
  await date.press("Enter");
  expect(await date.evaluate((node: HTMLInputElement) => node.validity.rangeOverflow)).toBe(true);
  expect((await f.stored(range.id)).startDate).toBe("2027-05-05");
  await date.fill("2027-05-06");
  await date.press("Enter");
  await expect.poll(async () => (await f.stored(range.id)).startDate).toBe("2027-05-06");
});

function required<T>(value: T | null | undefined): T {
  assert(value !== null && value !== undefined, "Expected fixture value to exist");
  return value;
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve: (() => void) | undefined;
  const promise = new Promise<void>((fulfill) => {
    resolve = fulfill;
  });
  assert(resolve, "Promise executor must initialize its resolver");
  return { promise, resolve };
}
