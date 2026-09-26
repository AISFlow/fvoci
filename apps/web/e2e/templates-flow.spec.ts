import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tplx",
  workspaceName: "Templates Flow",
};

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
  if ((await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0) {
    await login(page, admin.email, admin.password);
  }
}

test("workspace templates create list and apply document", async ({ page }) => {
  await ensureSetup(page);
  const slug = admin.workspaceSlug;

  await page.goto(`/w/${slug}/settings`);
  await page.getByRole("link", { name: "템플릿" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/settings/templates$`));
  await expect(page.getByRole("heading", { name: "템플릿" })).toBeVisible();

  await page.getByLabel("제목").fill("E2E 문서 템플릿");
  await page.getByRole("button", { name: "추가" }).click();
  await expect(page.getByRole("cell", { name: "E2E 문서 템플릿" })).toBeVisible();

  await page.getByRole("button", { name: "적용" }).first().click();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/WIKI-\\d+`));
  await expect(page.getByRole("heading", { name: "E2E 문서 템플릿" })).toBeVisible();
});
