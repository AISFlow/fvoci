import { expect, test } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const invited = {
  email: "second-human@example.com",
  password: "invitepass1",
  familyName: "박",
  givenName: "초대수락",
};

test("owner grants a group; a group-only member sees the private project", async ({ page }) => {
  test.setTimeout(120_000);
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
  await page.locator("summary").filter({ hasText: /^멤버$/ }).click();
  await page.getByLabel("초대할 이메일").fill(invited.email);
  await page.getByRole("button", { name: "초대", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "초대를 만들었습니다" })).toBeVisible();
  const inviteLink = page.getByRole("link").filter({ hasText: "/invite/" });
  await expect(inviteLink).toBeVisible();
  const href = await inviteLink.getAttribute("href");
  const token = href?.split("/invite/")[1];
  expect(token).toBeTruthy();

  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await page.goto(`/invite/${token}`);
  await expect(page.getByRole("heading", { name: /초대 수락/ })).toBeVisible();
  await page.getByLabel("이메일").fill(invited.email);
  await page.getByLabel("성").fill(invited.familyName);
  await page.getByLabel("이름", { exact: true }).fill(invited.givenName);
  await page.getByLabel("비밀번호").fill(invited.password);
  await page.getByRole("button", { name: "수락" }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/acme/settings");
  await page.locator("summary").filter({ hasText: /^그룹$/ }).click();
  await page.getByLabel("그룹 이름").fill("랩팀");
  await page.getByRole("button", { name: "만들기" }).click();
  await expect(page.getByRole("button", { name: "랩팀", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "랩팀", exact: true }).click();
  await expect(page.getByText("멤버가 없습니다", { exact: true })).toBeVisible();
  await page.getByLabel("멤버", { exact: true }).selectOption({ label: "박초대수락 (second-human@example.com)" });
  await page.getByRole("button", { name: "멤버 추가" }).click();
  await expect(page.getByText("멤버가 없습니다", { exact: true })).toHaveCount(0);
  const groupsPanel = page.locator("details").filter({ has: page.locator("summary", { hasText: /^그룹$/ }) });
  await expect(groupsPanel.getByText("박초대수락 (second-human@example.com)", { exact: true })).toBeVisible();

  await page.goto("/w/acme/projects");
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("GRP");
  await page.getByLabel("이름").fill("그룹프로젝트");
  await page.getByLabel("공개 범위").selectOption("private");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/GRP\/tasks$/);

  await page.locator("summary").filter({ hasText: /^프로젝트에 추가$/ }).click();
  await page.getByLabel("그룹", { exact: true }).selectOption({ label: "랩팀" });
  await page.getByLabel("역할").selectOption("viewer");
  await page.getByRole("button", { name: "프로젝트에 추가" }).click();
  await expect(page.getByText("랩팀 · 뷰어")).toBeVisible();
  await expect(page.getByRole("button", { name: "접근 권한 제거" })).toBeVisible();

  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await page.getByLabel("이메일").fill(invited.email);
  await page.getByLabel("비밀번호").fill(invited.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/acme/projects");
  await expect(page.getByText("그룹프로젝트")).toBeVisible();
  await expect(page.getByText("비공개")).toBeVisible();
  await page.getByRole("link", { name: /그룹프로젝트/ }).click();
  await expect(page).toHaveURL(/\/w\/acme\/GRP\/tasks$/);
  await page.locator("summary").filter({ hasText: /^프로젝트에 추가$/ }).click();
  await expect(page.getByText("랩팀 · 뷰어")).toBeVisible();
  await expect(page.getByRole("button", { name: "프로젝트에 추가" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "접근 권한 제거" })).toHaveCount(0);
});
