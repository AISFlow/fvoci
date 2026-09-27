import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "pgantt",
  workspaceName: "Project Gantt E2E",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
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

test("project gantt shows two tasks and a dependency link", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page, admin.workspaceSlug);
  const base = `/api/v1/workspaces/${wsId}`;
  const now = new Date();
  const y = now.getUTCFullYear();
  const mo = String(now.getUTCMonth() + 1).padStart(2, "0");
  const dueA = `${y}-${mo}-12`;
  const dueB = `${y}-${mo}-20`;

  const projectResponse = await page.request.post(`${base}/projects`, {
    data: {
      key: "GNT",
      name: `Gantt chart ${Date.now()}`,
      visibility: "workspace",
    },
  });
  expect(projectResponse.status()).toBe(201);
  const project: { id: string; key: string } = await projectResponse.json();

  const blockerResponse = await page.request.post(`${base}/projects/${project.id}/tasks`, {
    data: { title: "Gantt blocker", startDate: dueA, dueDate: dueA },
  });
  expect(blockerResponse.status()).toBe(201);
  const blocker: { id: string } = await blockerResponse.json();

  const blockedResponse = await page.request.post(`${base}/projects/${project.id}/tasks`, {
    data: { title: "Gantt blocked", startDate: dueB, dueDate: dueB },
  });
  expect(blockedResponse.status()).toBe(201);
  const blocked: { id: string } = await blockedResponse.json();

  const depResponse = await page.request.post(`${base}/tasks/${blocker.id}/dependencies`, {
    data: { blockedId: blocked.id, type: "FS" },
  });
  expect(depResponse.status()).toBe(200);

  await page.goto(`/w/${admin.workspaceSlug}/${project.key}/gantt`);
  const chart = page.locator('[data-slot="gantt"]');
  await expect(chart).toBeVisible();
  await expect(chart.locator(".fvoci-gantt__row-label", { hasText: "Gantt blocker" })).toBeVisible();
  await expect(chart.locator(".fvoci-gantt__row-label", { hasText: "Gantt blocked" })).toBeVisible();
  await expect(page.locator("polyline.fvoci-gantt__link")).toHaveCount(1);
});

test("gantt bar drag persists after reload", async ({ page }) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1280, height: 900 });
  await ensureSetup(page);
  const wsId = await workspaceId(page, admin.workspaceSlug);
  const base = `/api/v1/workspaces/${wsId}`;
  const taskTitle = "Gantt drag persist";
  const now = new Date();
  const y = now.getUTCFullYear();
  const mo = String(now.getUTCMonth() + 1).padStart(2, "0");
  const startDay = `${y}-${mo}-10`;
  const dueDay = `${y}-${mo}-14`;
  const shiftDays = 5;
  const shiftedStart = new Date(`${startDay}T00:00:00Z`);
  shiftedStart.setUTCDate(shiftedStart.getUTCDate() + shiftDays);
  const shiftedDue = new Date(`${dueDay}T00:00:00Z`);
  shiftedDue.setUTCDate(shiftedDue.getUTCDate() + shiftDays);
  const nextStart = shiftedStart.toISOString().slice(0, 10);
  const nextDue = shiftedDue.toISOString().slice(0, 10);

  const projectResponse = await page.request.post(`${base}/projects`, {
    data: {
      key: "GDR",
      name: `Gantt drag ${Date.now()}`,
      visibility: "workspace",
    },
  });
  expect(projectResponse.ok()).toBeTruthy();
  const project: { id: string; key: string } = await projectResponse.json();
  const taskResponse = await page.request.post(`${base}/projects/${project.id}/tasks`, {
    data: { title: taskTitle, startDate: startDay, dueDate: dueDay },
  });
  expect(taskResponse.ok()).toBeTruthy();
  const task: { id: string; number: number } = await taskResponse.json();
  const displayId = `${project.key}-${task.number}`;

  await page.goto(`/w/${admin.workspaceSlug}/${project.key}/gantt`);
  const chart = page.locator('[data-slot="gantt"]');
  await expect(chart).toBeVisible();
  await expect(chart).toHaveAttribute("data-bar-edit", "1");
  const bar = chart.locator(`g[aria-label="${taskTitle}"] .fvoci-gantt__bar-rect`);
  await expect(bar).toBeVisible();

  const patchPromise = page.waitForResponse(
    (r) => r.url().includes(`/tasks/${task.id}`) && r.request().method() === "PATCH",
  );
  const box = await bar.boundingBox();
  expect(box).not.toBeNull();
  if (!box) throw new Error("Gantt bar bounds unavailable");
  const fromX = box.x + box.width / 2;
  const fromY = box.y + box.height / 2;
  const dragPx = shiftDays * 32;
  await page.mouse.move(fromX, fromY);
  await page.mouse.down();
  await page.mouse.move(fromX + dragPx, fromY, { steps: 10 });
  await page.mouse.up();
  const patch = await patchPromise;
  expect(patch.status()).toBe(200);
  const body = patch.request().postDataJSON() as {
    startDate?: string;
    dueDate?: string;
  };
  expect(body.startDate).toBe(nextStart);
  expect(body.dueDate).toBe(nextDue);

  await page.reload();
  await expect(chart).toBeVisible();
  await expect(chart.locator(`g[aria-label="${taskTitle}"] .fvoci-gantt__bar-rect`)).toBeVisible();
  await page.goto(`/w/${admin.workspaceSlug}/${displayId}`);
  await expect(page.getByLabel("마감(종일)")).toHaveValue(nextDue);
});

test("archived project gantt is read-only", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page, admin.workspaceSlug);
  const base = `/api/v1/workspaces/${wsId}`;
  const projectResponse = await page.request.post(`${base}/projects`, {
    data: {
      key: "GRO",
      name: `Gantt archived ${Date.now()}`,
      visibility: "workspace",
    },
  });
  expect(projectResponse.ok()).toBeTruthy();
  const project: { id: string; key: string } = await projectResponse.json();
  await page.request.post(`${base}/projects/${project.id}/tasks`, {
    data: {
      title: "Archived gantt task",
      startDate: "2026-09-10",
      dueDate: "2026-09-14",
    },
  });
  const archive = await page.request.post(`${base}/projects/${project.id}/archive`);
  expect(archive.ok()).toBeTruthy();
  await page.goto(`/w/${admin.workspaceSlug}/${project.key}/gantt?y=2026&m=9`);
  const chart = page.locator('[data-slot="gantt"]');
  await expect(chart).toBeVisible();
  await expect(chart).not.toHaveAttribute("data-bar-edit", "1");
});
