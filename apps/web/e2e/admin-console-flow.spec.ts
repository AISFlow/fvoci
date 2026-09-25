import { expect, test } from "@playwright/test";
import { createE2eUser } from "./helpers";

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "console-member@example.com",
  password: "memberpass1",
  givenName: "멤버",
};

test("instance admin edits settings and publishes terms; members consent before continuing", async ({
  page,
  browser,
}) => {
  test.setTimeout(90_000);

  // Setup makes the first user the instance admin.
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await page.getByLabel("성").fill(admin.familyName);
  await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
  await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  createE2eUser(member.email, member.password, member.givenName);

  // Console entry → system counts, users and workspaces.
  await page.getByRole("link", { name: "인스턴스 관리" }).click();
  await expect(page).toHaveURL(/\/settings\/admin$/);
  const system = page.getByRole("region", { name: "시스템", exact: true });
  await expect(system.getByText("사용자: 2")).toBeVisible();
  await expect(system.getByText(/^워크스페이스: \d+$/)).toBeVisible();
  const users = page.getByRole("region", { name: "사용자", exact: true });
  await expect(users.getByText("admin@example.com")).toBeVisible();
  await expect(users.getByText(member.email)).toBeVisible();
  await expect(page.getByRole("region", { name: "워크스페이스", exact: true }).getByText(admin.workspaceName)).toBeVisible();

  // Instance setting: branding name persists across a reload and reaches the public view.
  const branding = page.getByRole("region", { name: "브랜딩", exact: true });
  const brandName = "FVOCI 테스트 인스턴스";
  await branding.getByLabel("branding.name").fill(brandName);
  const saved = page.waitForResponse(
    (res) => res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH",
  );
  await branding.getByRole("button", { name: "저장" }).click();
  expect((await saved).status()).toBe(200);
  await page.reload();
  await expect(page.getByRole("region", { name: "브랜딩", exact: true }).getByLabel("branding.name")).toHaveValue(brandName);
  const instance = await page.request.get("/api/v1/instance");
  expect(instance.ok()).toBe(true);
  expect((await instance.json()).values.branding.name).toBe(brandName);

  // Publish a required terms document.
  await page.getByRole("link", { name: "법적 문서", exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/legal$/);
  await expect(page.getByLabel("문서 종류")).toHaveValue("terms");
  await page.getByLabel("법적 문서 제목").fill("서비스 이용약관");
  await page.getByLabel("본문(마크다운)").fill("## 제1조\n\n이 약관은 **서비스** 이용 조건을 정합니다.");
  await page.getByLabel("발효일").fill("2026-01-01");
  await expect(page.getByLabel("필수 법적 문서")).toBeChecked();
  await page.getByRole("button", { name: "발행", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "발행되었습니다." })).toBeVisible();
  await expect(page.getByRole("list", { name: "현재 발행본" })).toContainText("v1");

  // The admin is gated too: the next gated request lands on the prompt and returns here.
  await page.reload();
  await expect(page).toHaveURL(/\/consent\?returnTo=%2Fsettings%2Flegal$/);
  await expect(page.getByRole("heading", { name: "법적 문서 동의" })).toBeVisible();
  await page.getByRole("checkbox", { name: "동의합니다" }).check();
  await page.getByRole("button", { name: "동의하고 계속" }).click();
  await expect(page).toHaveURL(/\/settings\/legal$/);
  await expect(page.getByRole("heading", { name: "법적 문서 관리" })).toBeVisible();

  // A member signing in is sent to the prompt on the first gated request.
  const memberContext = await browser.newContext();
  const memberPage = await memberContext.newPage();
  await memberPage.goto("/login");
  await memberPage.getByLabel("이메일").fill(member.email);
  await memberPage.getByLabel("비밀번호").fill(member.password);
  await memberPage.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(memberPage).toHaveURL(/\/consent\?returnTo=%2F$/);
  const gated = await memberPage.request.get("/api/v1/auth/me");
  expect(gated.status()).toBe(428);
  expect((await gated.json()).code).toBe("consent_required");
  await expect(memberPage.getByRole("heading", { name: "서비스 이용약관" })).toBeVisible();
  await expect(memberPage.getByText("이 약관은")).toBeVisible();
  const submit = memberPage.getByRole("button", { name: "동의하고 계속" });
  await expect(submit).toBeDisabled();
  await memberPage.getByRole("checkbox", { name: "동의합니다" }).check();
  await submit.click();
  await expect(memberPage).toHaveURL(/\/$/);
  await expect(memberPage.getByText("소속 워크스페이스가 없습니다.")).toBeVisible();
  expect((await memberPage.request.get("/api/v1/auth/me")).status()).toBe(200);

  // The member has no console: no entry, the route sends them home, the API is 404.
  await expect(memberPage.getByRole("link", { name: "인스턴스 관리" })).toHaveCount(0);
  await memberPage.goto("/settings/admin");
  await expect(memberPage).toHaveURL(/\/$/);
  expect((await memberPage.request.get("/api/v1/admin/system")).status()).toBe(404);
  expect((await memberPage.request.get("/api/v1/admin/users")).status()).toBe(404);
  await memberContext.close();
});
