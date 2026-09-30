// /login is a page of the Vue app (logout landing, MFA step, OIDC error
// query). Setup, invite, home and public information are also Vue pages;
// The remaining auth links and consent prompt are Vue pages too.
// These flows run against the production build served by the Rust server.
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { login, logout, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "vlogin",
  workspaceName: "Vue Login E2E",
};

// Icon sets must be bundled; these are the Iconify API hosts a runtime fetch would hit.
const ICON_API_HOSTS = ["api.iconify.design", "api.simplesvg.com", "api.unisvg.com"];

function watchIconRequests(page: Page): string[] {
  const hits: string[] = [];
  page.on("request", (request) => {
    const host = new URL(request.url()).hostname;
    if (ICON_API_HOSTS.includes(host)) hits.push(request.url());
  });
  return hits;
}

function vueRoot(page: Page) {
  return page.locator("#root.isolate");
}

async function expectVueLogin(page: Page): Promise<void> {
  await expect(page).toHaveURL(/\/login\/?(?:\?|$)/);
  await expect(vueRoot(page)).toHaveCount(1);
  await expect(page.getByRole("heading", { name: "로그인" })).toBeVisible();
  await expect(page.getByRole("button", { name: "로그인", exact: true })).toBeVisible();
}

async function workspaceId(request: APIRequestContext, slug: string): Promise<string> {
  const res = await request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const workspace = (await res.json()).items.find((item: { slug: string }) => item.slug === slug);
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
  } else if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
  // Setup starts at '/', then may cross Vue /login before returning home.
  // Require the authenticated home to mount before the caller navigates again.
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
}

async function failSetupUntilRetry(page: Page): Promise<{ exhausted: Promise<unknown> }> {
  await page.route("**/api/v1/setup", (route) =>
    route.fulfill({ status: 503, contentType: "application/json", body: '{"code":"unavailable"}' }),
  );
  let failures = 0;
  // Wait for the production query's initial request and three retries, so the
  // visible error assertion starts after the query enters its terminal state.
  const exhausted = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/v1/setup" &&
      response.status() === 503 &&
      ++failures === 4,
  );
  return { exhausted };
}

async function createProject(
  request: APIRequestContext,
  wsId: string,
  key: string,
): Promise<{ id: string; key: string }> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key, name: `Login ${key} ${Date.now()}`, visibility: "workspace" },
  });
  expect(res.status()).toBe(201);
  return res.json();
}

test("logout lands on the Vue login page; a refresh stays there", async ({ page }) => {
  const csp = watchCspViolations(page);
  const icons = watchIconRequests(page);
  await ensureSetup(page);
  await expect(page).toHaveURL(/\/$/);
  await expect(vueRoot(page)).toHaveCount(1);

  await logout(page);
  await expectVueLogin(page);

  await page.reload();
  await expectVueLogin(page);

  expect(csp).toEqual([]);
  expect(icons).toEqual([]);
});

test("password login reaches the Vue home; a wrong password stays on Vue /login", async ({
  page,
}) => {
  const csp = watchCspViolations(page);
  const icons = watchIconRequests(page);
  await page.goto("/login");
  await expectVueLogin(page);

  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill("wrong-password-1");
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expectVueLogin(page);

  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(vueRoot(page)).toHaveCount(1);
  await expect(page.getByRole("button", { name: "로그아웃" })).toBeVisible();

  expect(csp).toEqual([]);
  expect(icons).toEqual([]);
});

test("returnTo a Vue gantt path does a full load there after login", async ({ page }) => {
  const csp = watchCspViolations(page);
  const icons = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request, admin.workspaceSlug);
  const project = await createProject(page.request, wsId, "LGN");
  const ganttPath = `/w/${admin.workspaceSlug}/${project.key}/gantt`;

  await logout(page);
  await expectVueLogin(page);

  await page.goto(`/login?returnTo=${encodeURIComponent(ganttPath)}`);
  await expectVueLogin(page);
  expect(new URL(page.url()).searchParams.get("returnTo")).toBe(ganttPath);

  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/${project.key}/gantt$`));
  await expect(vueRoot(page)).toHaveCount(1);
  // An empty project has no chart bars; the Vue Gantt page still mounts.
  await expect(page.getByRole("link", { name: "간트" })).toBeVisible();
  await expect(page.getByText("태스크가 없습니다")).toBeVisible();

  expect(csp).toEqual([]);
  expect(icons).toEqual([]);
});

test("authenticated Vue login exposes setup failure and retries to home with its real session", async ({
  page,
}) => {
  await ensureSetup(page);
  const before = await page.request.get("/api/v1/auth/me");
  expect(before.status()).toBe(200);
  const user = await before.json();
  const { exhausted } = await failSetupUntilRetry(page);
  const me = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/v1/auth/me" && response.status() === 200,
  );

  await page.goto("/login#mfa=setup-error-fragment");
  expect(await (await me).json()).toEqual(user);
  await exhausted;
  await expect(vueRoot(page)).toHaveCount(1);
  await expect(page.getByRole("alert")).toHaveText("불러오지 못했습니다.");
  const retry = page.getByRole("button", { name: "다시 시도", exact: true });
  await expect(retry).toBeEnabled();
  expect(new URL(page.url()).hash).toBe("#mfa=setup-error-fragment");
  await expect(page.getByLabel("이메일")).toHaveCount(0);
  await expect(page.getByLabel("인증 코드")).toHaveCount(0);

  await page.unroute("**/api/v1/setup");
  const recovered = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/api/v1/setup" && response.status() === 200,
  );
  await retry.click();
  expect((await recovered).status()).toBe(200);
  await expect(page).toHaveURL(/\/$/);
  await expect(vueRoot(page)).toHaveCount(1);
  await expect(page.getByRole("button", { name: "로그아웃" })).toBeVisible();
  expect((await (await page.request.get("/api/v1/setup")).json()).needed).toBe(false);
  const after = await page.request.get("/api/v1/auth/me");
  expect(after.status()).toBe(200);
  expect(await after.json()).toEqual(user);
});

test("signed-out setup failure preserves the MFA fragment until a real setup retry succeeds", async ({
  page,
}) => {
  await login(page, admin.email, admin.password);
  await logout(page);
  const { exhausted } = await failSetupUntilRetry(page);
  const me = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/v1/auth/me" && response.status() === 401,
  );

  // This marker tests fragment gating and consumption; MFA verification itself
  // is covered by mfa-flow.spec.ts with a real server-issued challenge.
  // Change the query too: from the logout landing, a hash-only goto would reuse
  // the healthy setup query in the existing document instead of loading it anew.
  await page.goto("/login?returnTo=%2F#mfa=setup-error-fragment");
  await me;
  await exhausted;
  await expect(page.getByRole("alert")).toHaveText("불러오지 못했습니다.");
  const retry = page.getByRole("button", { name: "다시 시도", exact: true });
  await expect(retry).toBeEnabled();
  expect(new URL(page.url()).hash).toBe("#mfa=setup-error-fragment");
  await expect(page.getByLabel("이메일")).toHaveCount(0);
  await expect(page.getByLabel("인증 코드")).toHaveCount(0);

  await page.unroute("**/api/v1/setup");
  const recovered = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/api/v1/setup" && response.status() === 200,
  );
  await retry.click();
  expect((await recovered).status()).toBe(200);
  await expect(page.getByLabel("인증 코드")).toBeVisible();
  expect(new URL(page.url()).hash).toBe("");
  expect((await (await page.request.get("/api/v1/setup")).json()).needed).toBe(false);
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
});
