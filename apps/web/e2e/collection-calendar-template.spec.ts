import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";

test.describe.configure({ mode: "serial" });
async function fixture(page: Page, key: string) {
  const served: Promise<{ path: string; sha256: string }>[] = [];
  page.on("response", response => {
    const path = new URL(response.url()).pathname;
    if (path.startsWith("/assets/") && /\.(js|css)$/.test(path)) {
      served.push(response.body().then(body => ({ path: path.slice(1), sha256: createHash("sha256").update(body).digest("hex") })));
    }
  });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "시작하기" }).or(page.getByRole("button", { name: "로그아웃" })).or(page.getByRole("button", { name: "로그인", exact: true }))).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김"); await page.getByLabel("이름", { exact: true }).fill("달력");
    await page.getByLabel("이메일").fill("calendar@example.com"); await page.getByLabel("비밀번호").fill("supersecret1");
    await page.getByLabel("워크스페이스 이름").fill("Calendar template"); await page.getByLabel("주소(영문)").fill("caltemplate");
    await page.getByRole("button", { name: "시작하기" }).click(); await expect(page).toHaveURL(/\/$/);
  } else if (await page.getByRole("button", { name: "로그인", exact: true }).count()) await login(page, "calendar@example.com", "supersecret1");
  const ws = (await (await page.request.get("/api/v1/me/workspaces")).json()).items.find((w: { slug: string }) => w.slug === "caltemplate");
  const base = `/api/v1/workspaces/${ws.id}`;
  expect((await page.request.patch("/api/v1/auth/me", { data: { givenName: "달력", timezone: "America/New_York" } })).ok()).toBe(true);
  const res = await page.request.post(`${base}/projects`, { data: { key, name: key, visibility: "workspace" } }); expect(res.status()).toBe(201);
  const project = await res.json();
  const wf = await (await page.request.get(`${base}/projects/${project.id}/workflow`)).json();
  async function task(title: string, dates: object) {
    const created = await page.request.post(`${base}/projects/${project.id}/tasks`, { data: { title, type: "task", statusId: wf.statuses[0].id, ...dates } }); expect(created.status()).toBe(201);
    const row = await created.json(); return { ...row, displayId: `${key}-${row.number}` };
  }
  async function stored(id: string) { const res = await page.request.get(`${base}/tasks/${id}`); expect(res.ok()).toBe(true); return res.json(); }
  async function open(month = "2027-05") { await page.goto(`/w/caltemplate/${key}/calendar`); await expect(page).toHaveURL(`/w/caltemplate/${key}/calendar`); await expect(page.locator("[data-v-app]")).toHaveCount(1); await page.locator('input[type="month"]').fill(month); await expect(page.locator('table[data-testid="collection-calendar"]')).toBeVisible(); writeFileSync(`/tmp/fvoci-front272-calendar-served-${key}.json`, JSON.stringify({ head: process.env.FVOCI_CALENDAR_VERIFY_HEAD ?? "unbound", url: new URL(page.url()).pathname, assets: await Promise.all(served) }, null, 2)); }
  return { base, project, task, stored, open };
}

test("template day/week/month, mini calendar, keyboard and responsive sidebar connect to real timed task API", async ({ page }) => {
  const f = await fixture(page, "VIEW"); const timed = await f.task("Point deadline", {});
  expect((await page.request.patch(`${f.base}/tasks/${timed.id}`, { data: { dueAt: "2027-05-05T13:30:00Z" } })).ok()).toBe(true);
  await f.open(); await page.screenshot({ path: "/tmp/fvoci-front272-calendar-month.png" }); await page.getByTestId("calendar-mini").getByLabel("2027-05-05", { exact: true }).click();
  await page.getByRole("tab", { name: "Week", exact: true }).click();
  const grid = page.getByTestId("calendar-time-grid");
  await expect(grid.getByTestId(`collection-preview-${timed.displayId}`)).toContainText("09:30");
  await page.screenshot({ path: "/tmp/fvoci-front272-calendar-week.png" });
  await grid.getByTestId(`collection-preview-${timed.displayId}`).dragTo(grid.locator('[data-calendar-target="2027-05-06"][data-hour="14"]'));
  await expect.poll(async () => (await f.stored(timed.id)).dueAt).toBe("2027-05-06T18:30:00Z");
  expect((await f.stored(timed.id)).dueDate).toBeNull();
  await page.getByTestId("calendar-mini").getByLabel("2027-05-06", { exact: true }).click();
  await page.getByRole("tab", { name: "Day", exact: true }).click();
  await expect(grid.getByTestId(`collection-preview-${timed.displayId}`)).toBeVisible();
  await page.locator('.template-calendar').focus(); await page.keyboard.press("m"); await expect(page.locator('table[data-testid="collection-calendar"]')).toBeVisible();
  await page.keyboard.press("w"); await expect(grid).toBeVisible(); await page.keyboard.press("d"); await expect(page.getByRole("tab", { name: "Day", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowRight"); await expect(grid.getByRole("button", { name: "2027-05-07", exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByTestId("calendar-mini")).toBeHidden(); await page.getByRole("button", { name: "Calendar sidebar" }).click(); await expect(page.getByTestId("calendar-mini")).toBeVisible();
  await page.getByTestId("calendar-mini").getByLabel("2027-05-06", { exact: true }).click(); await expect(page.getByTestId("calendar-mini")).toBeHidden();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test("real date interval resize and optimistic pending settle; failed concurrent write rolls back and refetches", async ({ page }) => {
  const f = await fixture(page, "RANGE"); const range = await f.task("Stored range", { startDate: "2027-05-05", dueDate: "2027-05-07" });
  const point = await f.task("Single date", { dueDate: "2027-05-08" });
  await f.open();
  await expect(page.getByRole("button", { name: "Resize end · Single date" })).toHaveCount(0);
  let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route(`**${f.base}/tasks/${range.id}`, async route => { if (route.request().method() === "PATCH") await gate; await route.continue(); });
  await page.getByRole("button", { name: "Resize end · Stored range" }).dragTo(page.locator('td[data-date="2027-05-09"]'));
  await expect(page.locator('td[data-date="2027-05-09"]').getByTestId(`collection-preview-${range.displayId}`)).toBeVisible();
  await expect(page.getByTestId(`collection-preview-${range.displayId}`)).toHaveAttribute("aria-busy", "true");
  expect((await f.stored(range.id)).dueDate).toBe("2027-05-07"); release();
  await expect.poll(async () => (await f.stored(range.id)).dueDate).toBe("2027-05-09");
  await expect(page.getByTestId(`collection-preview-${range.displayId}`)).not.toHaveAttribute("aria-busy", "true");
  await page.unroute(`**${f.base}/tasks/${range.id}`);
  await page.getByRole("button", { name: "Resize start · Stored range" }).dragTo(page.locator('td[data-date="2027-05-06"]'));
  await expect.poll(async () => (await f.stored(range.id)).startDate).toBe("2027-05-06");
  // Snapshot in editor holds its expectedDates while another real client commits.
  await page.getByTestId(`collection-preview-${point.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  await editor.locator('input[type="date"]').fill("2027-05-12");
  expect((await page.request.patch(`${f.base}/tasks/${point.id}`, { data: { dueDate: "2027-05-10" } })).ok()).toBe(true);
  const conflict = page.waitForResponse(r => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${point.id}`));
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click(); expect((await conflict).status()).toBe(409);
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-12");
  await expect(page.locator('td[data-date="2027-05-10"]').getByTestId(`collection-preview-${point.displayId}`)).toBeVisible();
  await expect(page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${point.displayId}`)).toHaveCount(0);
  await editor.getByRole("button", { name: "최신 저장 뷰 불러오기" }).click();
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await f.stored(point.id)).dueDate).toBe("2027-05-12");
  await expect(editor).toHaveCount(0);
  await page.getByTestId(`collection-preview-${point.displayId}`).click();
  await editor.getByLabel("마감 시각", { exact: true }).check();
  // Converting a plain date requires the user's time; no invented 09:00 default.
  await expect(editor.locator('input[type="datetime-local"]')).toHaveValue("");
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect(editor.getByRole("alert")).toBeVisible(); expect((await f.stored(point.id)).dueDate).toBe("2027-05-12");
  await editor.locator('input[type="datetime-local"]').fill("2027-05-12T10:45");
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await f.stored(point.id)).dueAt).toBe("2027-05-12T14:45:00Z");
  expect((await f.stored(point.id)).dueDate).toBeNull();
});

test("offline reconnect preserves editor intent, refuses unsaved writes, and DST gap/fold stays in existing date contract", async ({ page, context }) => {
  const f = await fixture(page, "DST"); const point = await f.task("DST point", {});
  expect((await page.request.patch(`${f.base}/tasks/${point.id}`, { data: { dueAt: "2026-11-01T06:30:00Z" } })).ok()).toBe(true);
  await f.open("2026-11"); await page.getByTestId(`collection-preview-${point.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" }); const input = editor.locator('input[type="datetime-local"]');
  await expect(input).toHaveValue("2026-11-01T01:30");
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click(); await expect(editor).toHaveCount(0);
  expect((await f.stored(point.id)).dueAt).toBe("2026-11-01T06:30:00Z");
  await page.getByTestId(`collection-preview-${point.displayId}`).click(); await input.fill("2026-03-08T02:30");
  let patches = 0; page.on("request", req => { if (req.method() === "PATCH" && req.url().endsWith(`/tasks/${point.id}`)) patches++; });
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click(); await expect(editor.getByRole("alert")).toBeVisible(); expect(patches).toBe(0);
  await input.fill("2026-11-02T09:30");
  await context.setOffline(true); await expect(page.getByRole("status").filter({ hasText: "Offline · unsaved drafts" })).toBeVisible();
  await expect(editor.getByRole("button", { name: "저장 뷰 저장" })).toBeDisabled();
  // Independent HTTP client commits while the browser is offline. Reconnect may
  // refresh the grid but must retain both draft intent and the old conflict guard.
  expect((await page.request.patch(`${f.base}/tasks/${point.id}`, { data: { dueAt: "2026-11-03T14:30:00Z" } })).ok()).toBe(true);
  await context.setOffline(false); await expect(input).toHaveValue("2026-11-02T09:30");
  const saved = page.waitForResponse(r => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${point.id}`));
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click(); expect((await saved).status()).toBe(409);
  await expect(input).toHaveValue("2026-11-02T09:30");
  await expect(page.locator('td[data-date="2026-11-03"]').getByTestId(`collection-preview-${point.displayId}`)).toBeVisible();
  await editor.getByRole("button", { name: "최신 저장 뷰 불러오기" }).click();
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await f.stored(point.id)).dueAt).toBe("2026-11-02T14:30:00Z");
  await page.reload(); await page.locator('input[type="month"]').fill("2026-11"); await expect(page.locator('td[data-date="2026-11-02"]').getByTestId(`collection-preview-${point.displayId}`)).toBeVisible();
});

test("custom date editor retains stale item guard, rolls back conflict and preserves draft for explicit retry", async ({ page }) => {
  const f = await fixture(page, "VALUE"); const item = await f.task("Custom calendar date", {});
  const collection = await (await page.request.get(`${f.base}/projects/${f.project.id}/collection`)).json();
  const created = await page.request.post(`${f.base}/collections/${collection.id}/fields`, { data: { name: "Custom date", key: "custom_date", type: "date" } }); expect(created.status()).toBe(201);
  const field = await created.json();
  const query = async () => {
    const res = await page.request.post(`${f.base}/collections/${collection.id}/query`, { data: { config: { query: { filters: {}, sort: [] }, dateBy: null, groupBy: null }, limit: 100 } }); expect(res.ok()).toBe(true);
    return (await res.json()).items.find((r: { taskId: string }) => r.taskId === item.id);
  };
  const valuePath = `${f.base}/collections/${collection.id}/items/${(await query()).id}/values`;
  async function put(date: string) { const row = await query(); const res = await page.request.put(valuePath, { data: { fieldId: field.id, expectedFieldVersion: field.version, expectedVersion: row.version, value: { date } } }); expect(res.ok()).toBe(true); }
  await put("2027-05-08"); await f.open();
  await page.getByLabel("날짜 기준", { exact: true }).selectOption({ label: "Custom date" });
  await page.getByTestId(`collection-preview-${item.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" }); await editor.locator('input[type="date"]').fill("2027-05-12");
  const snapshotVersion = (await query()).version;
  await put("2027-05-10");
  const conflict = page.waitForResponse(r => r.request().method() === "PUT" && r.url().endsWith(valuePath));
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click(); const response = await conflict;
  expect(response.status()).toBe(409); expect(response.request().postDataJSON()).toMatchObject({ expectedVersion: snapshotVersion, expectedFieldVersion: field.version, value: { date: "2027-05-12" } });
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-12");
  await expect(page.locator('td[data-date="2027-05-10"]').getByTestId(`collection-preview-${item.displayId}`)).toBeVisible();
  await expect(page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${item.displayId}`)).toHaveCount(0);
  expect((await query()).values[field.id]).toEqual({ date: "2027-05-10" });
  await editor.getByRole("button", { name: "최신 저장 뷰 불러오기" }).click(); await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect.poll(async () => (await query()).values[field.id]).toEqual({ date: "2027-05-12" });
});

test("archive during pending calendar write is refused by real Rust permission guard and rolls back", async ({ page }) => {
  const f = await fixture(page, "DENY"); const item = await f.task("Permission changes", { dueDate: "2027-05-08" }); await f.open();
  await page.getByTestId(`collection-preview-${item.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" }); await editor.locator('input[type="date"]').fill("2027-05-12");
  let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route(`**${f.base}/tasks/${item.id}`, async route => { if (route.request().method() === "PATCH") await gate; await route.continue(); });
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  await expect(page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${item.displayId}`)).toHaveAttribute("aria-busy", "true");
  expect((await page.request.post(`${f.base}/projects/${f.project.id}/archive`)).ok()).toBe(true);
  const refused = page.waitForResponse(r => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${item.id}`)); release();
  expect((await refused).status()).toBe(409);
  await expect(page.locator('td[data-date="2027-05-08"]').getByTestId(`collection-preview-${item.displayId}`)).toBeVisible();
  await expect(page.locator('td[data-date="2027-05-12"]').getByTestId(`collection-preview-${item.displayId}`)).toHaveCount(0);
  expect((await f.stored(item.id)).dueDate).toBe("2027-05-08");
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-12"); await expect(editor.getByRole("button", { name: "저장 뷰 저장" })).toBeDisabled();
});

test("dual due fields keep Rust date precedence on unchanged edit; resize click and keyboard keep endpoint constraints", async ({ page }) => {
  const f = await fixture(page, "EDGE"); const dual = await f.task("Both due fields", { dueDate: "2027-05-08" });
  expect((await page.request.patch(`${f.base}/tasks/${dual.id}`, { data: { dueAt: "2027-05-20T13:30:00Z" } })).ok()).toBe(true);
  const range = await f.task("Keyboard resize", { startDate: "2027-05-05", dueDate: "2027-05-07" });
  await f.open();
  await expect(page.locator('td[data-date="2027-05-08"]').getByTestId(`collection-preview-${dual.displayId}`)).toBeVisible();
  await page.getByTestId(`collection-preview-${dual.displayId}`).click();
  const editor = page.getByRole("form", { name: "Calendar event editor" });
  await expect(editor.locator('input[type="date"]')).toHaveValue("2027-05-08"); await expect(editor.getByLabel("마감 시각", { exact: true })).not.toBeChecked();
  const unchanged = page.waitForResponse(r => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${dual.id}`));
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click(); const response = await unchanged; expect(response.ok()).toBe(true);
  expect(response.request().postDataJSON()).not.toHaveProperty("dueAt");
  expect((await f.stored(dual.id)).dueDate).toBe("2027-05-08"); expect((await f.stored(dual.id)).dueAt).toBe("2027-05-20T13:30:00Z");
  await expect(page.locator('td[data-date="2027-05-08"]').getByTestId(`collection-preview-${dual.displayId}`)).not.toHaveAttribute("aria-busy", "true");
  await page.getByRole("button", { name: "Resize end · Keyboard resize" }).click();
  const date = editor.locator('input[type="date"]');
  await expect(editor.getByLabel("마감 시각", { exact: true })).toHaveCount(0);
  await date.fill("2027-05-04"); await editor.getByRole("button", { name: "저장 뷰 저장" }).click();
  expect(await date.evaluate((node: HTMLInputElement) => node.validity.rangeUnderflow)).toBe(true);
  expect((await f.stored(range.id)).dueDate).toBe("2027-05-07");
  await date.fill("2027-05-09"); await date.press("Enter");
  await expect.poll(async () => (await f.stored(range.id)).dueDate).toBe("2027-05-09");
  await expect(editor).toHaveCount(0);
  const start = page.getByRole("button", { name: "Resize start · Keyboard resize" }); await expect(start).toBeEnabled(); await start.focus(); await start.press("Enter");
  await expect(date).toHaveValue("2027-05-05"); await date.fill("2027-05-10"); await date.press("Enter");
  expect(await date.evaluate((node: HTMLInputElement) => node.validity.rangeOverflow)).toBe(true);
  expect((await f.stored(range.id)).startDate).toBe("2027-05-05");
  await date.fill("2027-05-06"); await date.press("Enter"); await expect.poll(async () => (await f.stored(range.id)).startDate).toBe("2027-05-06");
});
