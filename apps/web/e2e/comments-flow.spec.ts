import { expect, test } from "@playwright/test";
import { createE2eUser, login } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "comments-member@example.com",
  password: "memberpass1",
  givenName: "댓글",
  familyName: "멤버",
};

test("member adds and resolves a wiki document comment", async ({ page }) => {
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

  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/WIKI-\d+$/);

  const panel = page.getByTestId("document-comments");
  await expect(panel.getByRole("heading", { name: "댓글" })).toBeVisible();

  const compose = panel.locator("[data-comment-compose] textarea");
  await compose.fill("E2E 댓글입니다");
  await panel.getByRole("button", { name: "등록" }).click();
  await expect(panel.getByText("E2E 댓글입니다")).toBeVisible();

  await panel.getByRole("button", { name: "해결" }).click();
  await expect(panel.getByRole("button", { name: "다시 열기" })).toBeVisible();

  await panel.getByRole("button", { name: "반응 👍" }).click();
  await expect(panel.getByRole("button", { name: "반응 👍", pressed: true })).toContainText("1");
});
