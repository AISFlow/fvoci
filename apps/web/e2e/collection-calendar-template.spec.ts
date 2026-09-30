import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });
async function fixture(page: Page, key: string) {
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
  async function open(month = "2027-05") { await page.goto(`/w/caltemplate/${key}/calendar`); await expect(page).toHaveURL(`/w/caltemplate/${key}/calendar`); await expect(page.locator("[data-v-app]")).toHaveCount(1); await page.locator('input[type="month"]').fill(month); await expect(page.locator('table[data-testid="collection-calendar"]')).toBeVisible(); }
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
  await context.setOffline(false); await expect(input).toHaveValue("2026-11-02T09:30");
  const saved = page.waitForResponse(r => r.request().method() === "PATCH" && r.url().endsWith(`/tasks/${point.id}`));
  await editor.getByRole("button", { name: "저장 뷰 저장" }).click(); expect((await saved).ok()).toBe(true);
  await expect.poll(async () => (await f.stored(point.id)).dueAt).toBe("2026-11-02T14:30:00Z");
  await page.reload(); await page.locator('input[type="month"]').fill("2026-11"); await expect(page.locator('td[data-date="2026-11-02"]').getByTestId(`collection-preview-${point.displayId}`)).toBeVisible();
});
