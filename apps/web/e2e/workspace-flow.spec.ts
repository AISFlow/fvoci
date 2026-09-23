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
  email: "member@example.com",
  password: "memberpass1",
  givenName: "멤버",
  familyName: "이",
};

test("setup → home → rename → logout → login → denied workspace", async ({ page }) => {
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
  await expect(page.getByRole("heading", { name: "나의 대시보드" })).toBeVisible();
  await expect(page.getByText(admin.workspaceName)).toBeVisible();

  const me = await page.request.get("/api/v1/auth/me");
  expect(me.ok()).toBe(true);
  const meBody = await me.json();
  expect(meBody.email).toBe("admin@example.com");

  await page.getByRole("link", { name: admin.workspaceName }).click();
  await expect(page).toHaveURL(/\/w\/acme\/settings$/);
  const renamed = "Renamed 워크스페이스";
  await page.getByLabel("워크스페이스 이름", { exact: true }).fill(renamed);
  await page.getByRole("button", { name: "저장" }).click();
  await expect(page.getByText("저장했습니다")).toBeVisible();

  await page.goto("/");
  await expect(page.getByText(renamed)).toBeVisible();

  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);

  await page.getByLabel("이메일").fill("ADMIN@example.com");
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByText(renamed)).toBeVisible();
});

test("second user cannot access foreign workspace settings", async ({ page }) => {
  createE2eUser(member.email, member.password, member.givenName);

  await page.goto("/login");
  await page.getByLabel("이메일").fill(member.email);
  await page.getByLabel("비밀번호").fill(member.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/acme/settings");
  await expect(page).toHaveURL(/\?denied=workspace$/);
  await expect(page.getByRole("alert")).toContainText("접근 권한");
});
