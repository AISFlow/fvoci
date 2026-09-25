import { expect, test } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

test("owner creates an API token, sees the secret once, then revokes it", async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });

  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/acme/settings");
  await page.locator("summary").filter({ hasText: /^토큰$/ }).click();
  await page.getByRole("textbox", { name: "이름", exact: true }).fill("CI 연동");
  await page.getByLabel("문서 조회").check();
  await page.getByRole("button", { name: "발급" }).click();
  await expect(page.getByRole("status").filter({ hasText: "이 토큰 값은 지금만 보입니다" })).toBeVisible();
  const revealed = page.getByRole("textbox", { name: "이 토큰 값은 지금만 보입니다" });
  await expect(revealed).toBeVisible();
  const secret = await revealed.inputValue();
  expect(secret.length).toBeGreaterThan(16);
  await expect(page.getByText("CI 연동")).toBeVisible();

  await page.getByRole("button", { name: "폐기" }).click();
  await expect(page.getByRole("heading", { name: "토큰을 폐기할까요?" })).toBeVisible();
  await page.getByRole("alertdialog").getByRole("button", { name: "폐기" }).click();
  await expect(page.getByText("토큰이 없습니다")).toBeVisible();
  await expect(page.getByText("CI 연동")).toHaveCount(0);
});
