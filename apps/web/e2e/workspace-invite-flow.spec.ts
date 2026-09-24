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

test("owner invites a second user who signs up, accepts, and appears in members", async ({
  page,
}) => {
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
  await expect(page.getByText(owner.workspaceName)).toBeVisible();

  await page.goto("/w/acme/settings");
  await page.locator("summary").filter({ hasText: /^멤버$/ }).click();
  await expect(page.getByText(invited.email)).toBeVisible();
  await expect(page.getByText("박초대수락")).toBeVisible();
  await expect(page.getByText("멤버", { exact: true }).nth(1)).toBeVisible();
});
