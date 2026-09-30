import { expect, test, type Page } from "@playwright/test";

async function expectVue(page: Page): Promise<void> {
  expect(await page.locator("#root").evaluate((root) => "__vue_app__" in root)).toBe(true);
}

test("home and public pages boot Vue with real persisted workspaces, legal versions and operator settings", async ({ page, browser }) => {
  // The public route is accessible even before installation, without a session.
  await page.goto("/service-info");
  await expect(page.getByRole("heading", { name: "서비스 정보" })).toBeVisible();
  await expectVue(page);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("관리자");
  await page.getByLabel("이메일").fill("home-admin@example.com");
  await page.getByLabel("비밀번호").fill("supersecret1");
  await page.getByLabel("워크스페이스 이름").fill("홈 연결 팀");
  await page.getByLabel("주소(영문)").fill("home-team");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page.getByRole("heading", { name: "나의 대시보드" })).toBeVisible();
  await expectVue(page);
  await page.reload();
  await expect(page.getByRole("link", { name: /홈 연결 팀/ })).toBeVisible();
  await expectVue(page);

  // Writes go to the real Rust API; Markdown is rendered by its helper.
  for (const version of [1, 2]) {
    const published = await page.request.post("/api/v1/admin/legal", { data: {
      kind: "terms", title: `공개 약관 ${version}`, bodyMarkdown: `## 조항 ${version}\n\n**안전한 약관**과 [도움말](https://example.com/help).\n\n[위험한 링크](javascript:alert(1))\n\n<script>window.legalInjected = true</script>`,
      effectiveAt: "2026-01-01T00:00:00Z", required: false,
    } });
    expect(published.status()).toBe(201);
    expect((await published.json()).version).toBe(version);
  }
  const operator = await page.request.patch("/api/v1/admin/instance-settings", { data: { operator: {
    businessName: "공개 운영사", supportEmail: "support@example.com", phone: "02-1234-5678",
    representative: null, registrationNumber: null, mailOrderNumber: null, address: null, businessInfoUrl: null, hostingProvider: null,
  } } });
  expect(operator.status()).toBe(200);

  const anonymous = await browser.newContext();
  const publicPage = await anonymous.newPage();
  try {
    await publicPage.goto("/legal/terms");
    await expect(publicPage.getByRole("heading", { name: "공개 약관 2", exact: true })).toBeVisible();
    await expect(publicPage.getByRole("heading", { name: "조항 2", exact: true })).toBeVisible();
    await expect(publicPage.locator("strong").filter({ hasText: "안전한 약관" })).toBeVisible();
    await expect(publicPage.getByRole("link", { name: "도움말" })).toHaveAttribute("href", "https://example.com/help");
    await expect(publicPage.locator('a[href^="javascript:"]')).toHaveCount(0);
    expect(await publicPage.evaluate(() => "legalInjected" in window)).toBe(false);
    await expectVue(publicPage);
    await publicPage.reload();
    await expect(publicPage.getByRole("heading", { name: "공개 약관 2", exact: true })).toBeVisible();
    await publicPage.getByRole("link", { name: /^v1/ }).click();
    await expect(publicPage).toHaveURL(/\/legal\/terms\?version=1$/);
    await expect(publicPage.getByRole("heading", { name: "공개 약관 1", exact: true })).toBeVisible();
    await publicPage.reload();
    await expect(publicPage.getByRole("heading", { name: "조항 1", exact: true })).toBeVisible();
    await expectVue(publicPage);
    await publicPage.goto("/legal/privacy");
    await expect(publicPage.getByRole("alert")).toBeVisible();
    await expectVue(publicPage);
    await publicPage.goto("/service-info");
    await expect(publicPage.getByText("공개 운영사")).toBeVisible();
    await expect(publicPage.getByRole("link", { name: "support@example.com" })).toHaveAttribute("href", "mailto:support@example.com");
    await publicPage.reload();
    await expect(publicPage.getByText("공개 운영사")).toBeVisible();
    await expectVue(publicPage);
    await publicPage.goto("/?denied=workspace");
    await expect(publicPage).toHaveURL(/\/login\?returnTo=%2F%3Fdenied%3Dworkspace$/);
    await expect(publicPage.getByLabel("이메일")).toBeVisible();
    await publicPage.reload();
    await expect(publicPage.getByLabel("이메일")).toBeVisible();
  } finally {
    await anonymous.close();
  }

  await page.goto("/settings/legal");
  await expect(page.getByRole("heading", { name: "법적 문서 관리" })).toBeVisible();
  expect(await page.locator("#root").evaluate((root) => "__vue_app__" in root)).toBe(false);
  await page.goto("/");
  await page.locator("footer").getByRole("link", { name: "이용약관" }).click();
  await expect(page.getByRole("heading", { name: "공개 약관 2", exact: true })).toBeVisible();
  await expectVue(page);
});
