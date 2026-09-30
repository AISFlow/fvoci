import { expect, test, type Page } from "@playwright/test";
import { authSql, expectVueAuth } from "./auth-link-evidence";
import { createE2eUser, login, watchCspViolations } from "./helpers";
import { currentStep, totp } from "./mfa-helpers";

const admin = { email: "vue-console-owner@example.com", password: "supersecret1" };
const member = { email: "vue-console-member@example.com", password: " membersecret1 " };

test.beforeAll(async ({ browser }) => {
  const page = await browser.newPage({ baseURL: process.env.PLAYWRIGHT_BASE_URL });
  try {
    const status = await page.request.get("/api/v1/setup");
    expect(status.status()).toBe(200);
    if ((await status.json()).needed) {
      await page.goto("/");
      await expect(page).toHaveURL(/\/setup$/);
      await page.getByLabel("이름", { exact: true }).fill("콘솔 관리자");
      await page.getByLabel("이메일").fill(admin.email);
      await page.getByLabel("비밀번호").fill(admin.password);
      await page.getByLabel("워크스페이스 이름").fill("Vue Console");
      await page.getByLabel("주소(영문)").fill("vue-console");
      await page.getByRole("button", { name: "시작하기" }).click();
      await expect(page).toHaveURL(/\/$/);
    }
    // A failed test replaces the Playwright worker; keep setup idempotent
    // so later independent tests still execute against this group's DB.
    if (authSql(`SELECT count(*) FROM fvoci.users WHERE email = '${member.email}'`) === "0") {
      createE2eUser(member.email, member.password, "콘솔 멤버");
    }
  } finally {
    await page.close();
  }
});

async function expectSecretsAbsent(page: Page, secrets: string[]): Promise<void> {
  const retained = await page.evaluate(() => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: { _context: { provides: Record<string, { getQueryCache: () => { getAll: () => unknown[] } }> } };
    };
    return JSON.stringify({
      href: location.href,
      local: Object.entries(localStorage),
      session: Object.entries(sessionStorage),
      queries: root.__vue_app__._context.provides.VUE_QUERY_CLIENT.getQueryCache().getAll().map(
        (query) => (query as { state: unknown }).state,
      ),
    });
  });
  for (const secret of secrets) expect(retained.includes(secret)).toBe(false);
}

test("Vue account saves profile and MFA preserves password whitespace without caching secrets", async ({ page }) => {
  const violations = watchCspViolations(page);
  await login(page, member.email, member.password);
  await page.goto("/settings/account");
  await expectVueAuth(page);
  await page.locator("#settings-given-name").fill("재로드 멤버");
  const nameSaved = page.waitForResponse((res) => res.url().endsWith("/api/v1/auth/me") && res.request().method() === "PATCH");
  await page.getByRole("region", { name: "계정 설정", exact: true }).getByRole("button", { name: "저장", exact: true }).click();
  expect((await nameSaved).status()).toBe(200);
  await page.reload();
  await expect(page.locator("#settings-given-name")).toHaveValue("재로드 멤버");
  // Inject a dropped transport connection, without supplying an API answer.
  // Browser offline mode pauses TanStack queries before they hit transport.
  const meRoute = "**/api/v1/auth/me";
  try {
    await page.route(meRoute, (route) => route.abort("connectionfailed"));
    await page.evaluate(async () => {
      const root = document.getElementById("root") as HTMLElement & {
        __vue_app__: { _context: { provides: Record<string, { invalidateQueries: (input: { queryKey: string[] }) => Promise<void> }> } };
      };
      await root.__vue_app__._context.provides.VUE_QUERY_CLIENT.invalidateQueries({ queryKey: ["auth", "me"] });
    });
    await expect(page.getByRole("alert")).toBeVisible();
    await expect(page).toHaveURL(/\/settings\/account$/);
  } finally {
    await page.unroute(meRoute);
  }
  await page.getByRole("button", { name: "다시 시도", exact: true }).click();
  await expect(page.locator("#settings-given-name")).toHaveValue("재로드 멤버");
  const preferences = page.getByRole("region", { name: "설정", exact: true });
  await preferences.getByLabel("시간대", { exact: true }).click();
  await page.getByRole("option", { name: "UTC", exact: true }).click();
  await preferences.getByLabel("주 시작", { exact: true }).click();
  await page.getByRole("option", { name: "일요일", exact: true }).click();
  await preferences.getByLabel("글자 크기", { exact: true }).click();
  await page.getByRole("option", { name: "크게", exact: true }).click();
  const preferenceSaved = page.waitForResponse((res) => res.url().endsWith("/api/v1/auth/me") && res.request().method() === "PATCH");
  await preferences.getByRole("button", { name: "저장", exact: true }).click();
  expect((await preferenceSaved).status()).toBe(200);
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize)).toBe("18px");
  await preferences.getByLabel("테마", { exact: true }).click();
  await page.getByRole("option", { name: "다크", exact: true }).click();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await page.reload();
  await expect(preferences.getByLabel("시간대", { exact: true })).toContainText("UTC");
  await expect(preferences.getByLabel("주 시작", { exact: true })).toContainText("일요일");
  await expect(preferences.getByLabel("글자 크기", { exact: true })).toContainText("크게");
  await expect(page.locator("html")).toHaveClass(/dark/);
  const profile = await (await page.request.get("/api/v1/auth/me")).json();
  expect(profile).toMatchObject({ givenName: "재로드 멤버", locale: "ko", timezone: "UTC", weekStartsOn: 0, textScale: 18 });
  await page.goto("/");
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize)).toBe("18px");
  await page.goto("/settings/account");

  const mfa = page.getByTestId("mfa-section");
  await mfa.locator("#settings-mfa-confirm").fill("wrong-password");
  await mfa.getByRole("button", { name: "설정", exact: true }).click();
  await expect(mfa.getByRole("alert")).toBeVisible();
  await expect(mfa.locator("#settings-mfa-confirm")).toHaveValue("");
  await mfa.locator("#settings-mfa-confirm").fill(member.password);
  const setupResponse = page.waitForResponse((response) => response.url().endsWith("/api/v1/auth/mfa/setup"));
  await mfa.getByRole("button", { name: "설정", exact: true }).click();
  const response = await setupResponse;
  expect(response.status()).toBe(200);
  expect(response.request().postDataJSON().currentPassword).toBe(member.password);
  const secret = (await mfa.getByTestId("mfa-secret").textContent())!.trim();
  expect(secret).toMatch(/^[A-Z2-7=]+$/i);
  await expectSecretsAbsent(page, [secret, member.password]);
  expect(authSql(`SELECT bool_and(totp_secret LIKE 'enc:v2:%') FROM fvoci.user_mfa m JOIN fvoci.users u ON u.id = m.user_id WHERE u.email = '${member.email}'`)).toBe("t");
  await mfa.locator("#settings-mfa-code").fill(totp(secret, currentStep()));
  await mfa.getByRole("button", { name: "켜기", exact: true }).click();
  await expect(mfa.getByTestId("mfa-recovery-codes")).toBeVisible();
  const recovery = await mfa.getByTestId("mfa-recovery-codes").locator("li").allTextContents();
  expect(recovery).toHaveLength(10);
  await expectSecretsAbsent(page, [secret, ...recovery]);
  await mfa.getByRole("button", { name: "보관했습니다", exact: true }).click();
  await expect(mfa.getByTestId("mfa-secret")).toHaveCount(0);
  await expect(mfa.getByTestId("mfa-recovery-codes")).toHaveCount(0);
  await page.reload();
  await expectVueAuth(page);
  await expect(mfa.getByTestId("mfa-status")).toContainText("사용 중");
  const status = await page.request.get("/api/v1/auth/mfa");
  expect(await status.json()).toEqual({ enabled: true, recoveryCodesLeft: 10 });
  await mfa.locator("#settings-mfa-confirm").fill(member.password);
  const disabled = page.waitForResponse((res) => res.url().endsWith("/api/v1/auth/mfa/disable"));
  await mfa.getByRole("button", { name: "해제", exact: true }).click();
  expect((await disabled).status()).toBe(200);
  await expect(mfa.locator("#settings-mfa-confirm")).toHaveValue("");
  await page.reload();
  await expect(mfa.getByTestId("mfa-status")).toContainText("사용 안 함");
  await expectSecretsAbsent(page, [secret, ...recovery, member.password]);
  expect(violations).toEqual([]);
});

test("Vue personal API tokens show a secret once, persist metadata, and revoke actual Rust access", async ({ page, browser }) => {
  await login(page, admin.email, admin.password);
  await page.goto("/settings/account");
  await expectVueAuth(page);
  const tokenSection = page.getByRole("region", { name: "토큰", exact: true });
  await tokenSection.getByLabel("워크스페이스 이름", { exact: true }).click();
  await page.getByRole("option", { name: "Vue Console", exact: true }).click();
  await tokenSection.getByLabel("이름", { exact: true }).fill("콘솔 개인 토큰");
  await tokenSection.getByLabel("프로젝트 조회", { exact: true }).check();
  const createdResponse = page.waitForResponse((res) => res.url().endsWith("/api/v1/me/api-tokens") && res.request().method() === "POST");
  await tokenSection.getByRole("button", { name: "발급", exact: true }).click();
  const created = await createdResponse;
  expect(created.status()).toBe(201);
  const output = await created.json();
  const secret = page.getByTestId("account-token-secret");
  await expect(secret.getByRole("textbox")).toHaveValue(output.token);
  await expectSecretsAbsent(page, [output.token]);
  const list = await (await page.request.get("/api/v1/me/api-tokens")).json();
  expect(list.items.find((item: { id: string }) => item.id === output.id)).toMatchObject({ name: "콘솔 개인 토큰", workspaceId: output.workspaceId });
  expect(JSON.stringify(list)).not.toContain(output.token);
  const external = await browser.newContext({ baseURL: process.env.PLAYWRIGHT_BASE_URL });
  try {
    const path = `/api/v1/workspaces/${output.workspaceId}/projects`;
    expect((await external.request.get(path, { headers: { Authorization: `Bearer ${output.token}` } })).status()).toBe(200);
    expect((await external.request.get("/api/v1/me/api-tokens", { headers: { Authorization: `Bearer ${output.token}` } })).status()).toBe(404);
    await page.reload();
    await expect(secret).toHaveCount(0);
    const row = page.getByTestId("account-token-row").filter({ hasText: "콘솔 개인 토큰" });
    await expect(row).toBeVisible();
    await row.getByRole("button", { name: "폐기", exact: true }).click();
    const dialog = page.getByRole("alertdialog");
    const revoked = page.waitForResponse((res) => res.url().endsWith(`/api/v1/me/api-tokens/${output.id}`) && res.request().method() === "DELETE");
    await dialog.getByRole("button", { name: "폐기", exact: true }).click();
    expect((await revoked).status()).toBe(200);
    await expect(row).toHaveCount(0);
    expect((await external.request.get(path, { headers: { Authorization: `Bearer ${output.token}` } })).status()).toBe(401);
    await page.reload();
    await expect(row).toHaveCount(0);
    await expectSecretsAbsent(page, [output.token]);
  } finally {
    await external.close();
  }
});

test("Vue instance settings persist and admin user actions enforce the last-admin invariant", async ({ page }) => {
  await login(page, admin.email, admin.password);
  await page.goto("/settings/admin");
  await expectVueAuth(page);
  const share = page.getByRole("region", { name: "공유 링크", exact: true });
  await share.getByLabel("share.defaultExpiresDays").fill("17");
  const saved = page.waitForResponse((res) => res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH");
  await share.getByRole("button", { name: "저장", exact: true }).click();
  expect((await saved).status()).toBe(200);
  await page.reload();
  await expect(share.getByLabel("share.defaultExpiresDays")).toHaveValue("17");
  const branding = page.getByRole("region", { name: "브랜딩", exact: true });
  await expect(branding.getByLabel("branding.name")).toBeDisabled();

  const users = page.getByRole("region", { name: "사용자", exact: true });
  const memberRow = users.getByRole("row").filter({ hasText: member.email });
  const toggle = memberRow.getByRole("button", { name: `인스턴스 관리자: ${member.email}`, exact: true });
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await page.reload();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  const lastAdmin = users.getByRole("button", { name: `인스턴스 관리자: ${admin.email}`, exact: true });
  const refused = page.waitForResponse((res) => res.url().endsWith("/api/v1/admin/users") && res.request().method() === "PATCH");
  await lastAdmin.click();
  expect((await refused).status()).toBe(409);
  await expect(users.getByRole("alert")).toBeVisible();
  await expect(lastAdmin).toHaveAttribute("aria-pressed", "true");
  await page.goto("/settings/audit");
  await expectVueAuth(page);
  await expect(page.getByText("엔터프라이즈 기능 사용 권한이 필요합니다")).toBeVisible();
});

test("Vue admin legal editor publishes versions and public legal stays read only", async ({ page }) => {
  const violations = watchCspViolations(page);
  await login(page, admin.email, admin.password);
  await page.goto("/settings/legal");
  await expectVueAuth(page);
  await page.getByLabel("법적 문서 제목").fill("다른 종류의 초안");
  await page.getByRole("button", { name: "개인정보처리방침(privacy)", exact: true }).click();
  await expect(page.getByLabel("법적 문서 제목")).toHaveValue("");
  await page.getByRole("button", { name: "이용약관(terms)", exact: true }).click();
  await page.getByLabel("법적 문서 제목").fill("Vue 콘솔 약관");
  await page.getByLabel("본문(마크다운)").fill("## 적용 범위\n\n관리자가 **발행한** 문서입니다.\n\n<script>window.legalInjection=true</script>");
  await page.getByLabel("발효일").fill("2026-01-01");
  await page.getByLabel("필수 법적 문서").uncheck();
  const published = page.waitForResponse((res) => res.url().endsWith("/api/v1/admin/legal") && res.request().method() === "POST");
  await page.getByRole("button", { name: "발행", exact: true }).click();
  expect((await published).status()).toBe(201);
  await expect(page.getByRole("list", { name: "현재 발행본" })).toContainText("v1");
  await page.reload();
  await expectVueAuth(page);
  await expect(page.getByRole("list", { name: "현재 발행본" })).toContainText("Vue 콘솔 약관");
  await page.goto("/legal/terms");
  await expectVueAuth(page);
  await expect(page.getByRole("heading", { name: "Vue 콘솔 약관" })).toBeVisible();
  await expect(page.getByRole("button", { name: "발행", exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => "legalInjection" in window)).toBe(false);
  expect(authSql("SELECT count(*) FROM fvoci.legal_documents WHERE kind = 'terms' AND title = 'Vue 콘솔 약관'")).toBe("1");
  expect(violations).toEqual([]);
});

test("Vue global admin direct URLs deny non-admins before privileged loads and Rust denies writes", async ({ page, browser }) => {
  await login(page, member.email, member.password);
  const privileged: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith("/api/v1/admin/")) privileged.push(request.url());
  });
  for (const path of ["/settings/admin", "/settings/audit", "/settings/legal"]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/$/);
    await expectVueAuth(page);
  }
  expect(privileged).toEqual([]);
  for (const path of ["system", "users", "audit", "instance-settings"]) {
    expect((await page.request.get(`/api/v1/admin/${path}`)).status()).toBe(404);
  }
  expect((await page.request.patch("/api/v1/admin/instance-settings", { data: { share: { defaultExpiresDays: 1 } } })).status()).toBe(404);
  expect((await page.request.post("/api/v1/admin/legal", { data: { kind: "terms", title: "Denied", bodyMarkdown: "Denied", effectiveAt: "2026-01-01T00:00:00Z", required: false } })).status()).toBe(404);
  const anonymous = await browser.newPage({ baseURL: process.env.PLAYWRIGHT_BASE_URL });
  try {
    await anonymous.goto("/settings/legal");
    await expect(anonymous).toHaveURL(/\/login(?:\?|$)/);
    await expectVueAuth(anonymous);
    await anonymous.goto("/legal/terms");
    await expect(anonymous.getByRole("heading", { name: "Vue 콘솔 약관" })).toBeVisible();
    await expect(anonymous.getByLabel("본문(마크다운)")).toHaveCount(0);
  } finally {
    await anonymous.close();
  }
});
