import { expect, test } from "@playwright/test";

// Real production boot/router boundary: no intercepted API or synthetic page.
test("project and task URLs mount Vue across direct loads, links, reload and encoded handoffs", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("경로");
  await page.getByLabel("이메일").fill("routes@example.com");
  await page.getByLabel("비밀번호").fill("routepass123");
  await page.getByLabel("워크스페이스 이름").fill("Route workspace");
  await page.getByLabel("주소(영문)").fill("routes");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/routes/projects");
  await expect(page.locator("#root[data-v-app]")).toHaveCount(0);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("OPS-DEV");
  await page.getByLabel("이름", { exact: true }).fill("Route project");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(/\/OPS-DEV\/tasks$/);
  await expect(page.getByRole("heading", { name: "Route project" })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("Route task");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(/\/OPS-DEV-2$/);
  await expect(page.getByRole("heading", { name: "Route task" })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);

  // Task breadcrumb and list links remain in one Vue runtime.
  await page.evaluate(() => { (window as unknown as { routeMarker: string }).routeMarker = "same-runtime"; });
  await page.locator(".task-home__crumb").getByRole("link", { name: "Route project" }).click();
  await expect(page).toHaveURL(/\/OPS-DEV\/tasks$/);
  await page.getByRole("link", { name: /Route task/ }).click();
  await expect(page.getByRole("heading", { name: "Route task" })).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { routeMarker?: string }).routeMarker)).toBe("same-runtime");
  await page.reload();
  await expect(page.getByRole("heading", { name: "Route task" })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);

  // Raw encoded resources start in React, then canonicalize to Vue once,
  // retaining query and fragment; mixed case and trailing slash load directly.
  await page.goto("/w/routes/%4FPS-DEV-2?from=encoded#task-comments");
  await expect(page).toHaveURL(/\/OPS-DEV-2\?from=encoded#task-comments$/);
  await expect(page.getByRole("heading", { name: "Route task" })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await page.goto("/w/routes/ops-dev-2/");
  await expect(page.getByRole("heading", { name: "Route task" })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);

  await page.goto("/w/routes/%4FPS-DEV?from=encoded#overview");
  await expect(page).toHaveURL(/\/OPS-DEV\?from=encoded#overview$/);
  await expect(page.getByRole("heading", { name: "Route project" })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  for (const view of ["tasks", "table", "board", "calendar", "gantt"]) {
    await page.goto(`/w/routes/OPS-DEV/${view}`);
    if (view === "gantt") await expect(page.getByRole("navigation", { name: "프로젝트 관리 메뉴" })).toBeVisible();
    else await expect(page.getByRole("heading", { name: "Route project" })).toBeVisible();
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
    await expect(page.getByRole("alert")).toHaveCount(0);
  }
  // Project document root shares the item URL grammar but resolves as a doc.
  await page.goto("/w/routes/OPS-DEV-1");
  await expect(page.locator(".tiptap")).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await page.goto("/w/routes/projects");
  await expect(page.getByRole("heading", { name: "프로젝트", exact: true })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(0);
});
