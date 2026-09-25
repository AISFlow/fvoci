import { expect, test } from "@playwright/test";
import { waitForCapturedMail } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  newPassword: "newsecret12",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const invited = {
  email: "invitee@example.com",
};

test("invitation email is delivered and password reset uses the captured link", async ({
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

  const inviteMail = await waitForCapturedMail(
    (mail) => mail.to === invited.email && mail.text.includes("/invite/"),
  );
  expect(inviteMail.text).toContain("Subject: 워크스페이스 초대");

  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);

  await page.getByRole("button", { name: "비밀번호를 잊으셨나요?" }).click();
  await page.locator("#password-reset-email").fill(owner.email);
  await page.getByRole("button", { name: "재설정 링크 받기" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "계정이 있으면 재설정 링크를 보냈습니다" }),
  ).toBeVisible();

  const resetMail = await waitForCapturedMail((mail) =>
    mail.text.includes("/reset-password?token="),
  );
  expect(resetMail.to.toLowerCase()).toBe("admin@example.com");
  expect(resetMail.text).toContain("Subject: FVOCI 비밀번호 재설정");
  const tokenMatch = resetMail.text.match(/reset-password\?token=([A-Za-z0-9_-]+)/);
  expect(tokenMatch?.[1]).toBeTruthy();
  const token = tokenMatch![1];

  await page.goto(`/reset-password?token=${token}`);
  await expect(page.getByRole("heading", { name: "비밀번호 재설정" })).toBeVisible();
  await page.getByLabel("새 비밀번호").fill(owner.newPassword);
  await page.getByRole("button", { name: "비밀번호 변경" }).click();
  await expect(page).toHaveURL(/\/login\?reset=1/);
  await expect(
    page.getByRole("status").filter({ hasText: "비밀번호가 재설정되었습니다" }),
  ).toBeVisible();

  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(/\/login/);

  await page.getByLabel("비밀번호").fill(owner.newPassword);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
});
