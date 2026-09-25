import { expect, test } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

test("owner creates a group, adds a member, and grants it on a project", async ({ page }) => {
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
  await page.locator("summary").filter({ hasText: /^그룹$/ }).click();
  await page.getByLabel("그룹 이름").fill("랩팀");
  await page.getByRole("button", { name: "만들기" }).click();
  await expect(page.getByRole("button", { name: "랩팀", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "랩팀", exact: true }).click();
  await expect(page.getByText("멤버가 없습니다", { exact: true })).toBeVisible();
  await page.getByLabel("멤버", { exact: true }).selectOption({ index: 1 });
  await page.getByRole("button", { name: "멤버 추가" }).click();
  await expect(page.getByText("멤버가 없습니다", { exact: true })).toHaveCount(0);
  const groupsPanel = page.locator("details").filter({ has: page.locator("summary", { hasText: /^그룹$/ }) });
  await expect(groupsPanel.getByText("김관리자 (admin@example.com)", { exact: true })).toBeVisible();

  await page.goto("/w/acme/projects");
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("GRP");
  await page.getByLabel("이름").fill("그룹프로젝트");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/GRP\/tasks$/);

  await page.locator("summary").filter({ hasText: /^프로젝트에 추가$/ }).click();
  await page.getByLabel("그룹", { exact: true }).selectOption({ label: "랩팀" });
  await page.getByLabel("역할").selectOption("viewer");
  await page.getByRole("button", { name: "프로젝트에 추가" }).click();
  await expect(page.getByText("랩팀 · 뷰어")).toBeVisible();
});
