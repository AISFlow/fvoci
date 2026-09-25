import { expect, test } from "@playwright/test";
import { createE2eUser, login, logout, waitForCapturedMail } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  newPassword: "newsecret12",
  newEmail: "owner-new@example.com",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "leaver@example.com",
  password: "leaverpass1",
  givenName: "탈퇴자",
};

function tokenFrom(text: string, pattern: RegExp): string {
  const match = text.match(pattern);
  expect(match?.[1]).toBeTruthy();
  return match![1];
}

test("account settings: password, email change, magic link, withdraw and cancel", async ({
  page,
}) => {
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

  // Password change keeps the current session.
  await page.getByRole("link", { name: "계정", exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/account$/);
  await expect(page.getByRole("heading", { name: "계정 설정" })).toBeVisible();
  await page.locator("#settings-current-password").fill(owner.password);
  await page.locator("#settings-new-password").fill(owner.newPassword);
  await page.getByRole("button", { name: "비밀번호 변경" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "비밀번호가 변경되었습니다." }),
  ).toBeVisible();

  await page.goto("/");
  await logout(page);
  await login(page, owner.email, owner.newPassword);

  // Email change is confirmed from the link mailed to the new address.
  await page.goto("/settings/account");
  await page.getByLabel("새 이메일").fill(owner.newEmail);
  await page.getByRole("button", { name: "변경", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "확인 메일을 새 주소로 보냈습니다" }),
  ).toBeVisible();
  const changeMail = await waitForCapturedMail(
    (mail) => mail.to === owner.newEmail && mail.text.includes("/confirm-email?token="),
  );
  expect(changeMail.text).toContain("Subject: FVOCI 이메일 변경 확인");
  const changeToken = tokenFrom(changeMail.text, /confirm-email\?token=([A-Za-z0-9_-]+)/);

  await page.goto(`/confirm-email?token=${changeToken}`);
  await expect(page.getByRole("heading", { name: "이메일 변경 확인" })).toBeVisible();
  await page.getByRole("button", { name: "이메일 변경 확정" }).click();
  await expect(page).toHaveURL(/\/settings\/account\?email_changed=1$/);
  await expect(
    page.getByRole("status").filter({ hasText: "이메일이 변경되었습니다." }),
  ).toBeVisible();
  await expect(page.getByTestId("account-email")).toHaveText(owner.newEmail);

  // Magic-link login with the new address.
  await page.goto("/");
  await logout(page);
  await page.getByRole("button", { name: "이메일로 로그인 링크 받기" }).click();
  await page.locator("#magic-link-email").fill(owner.newEmail);
  await page.getByRole("button", { name: "링크 받기", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "계정이 있으면 로그인 링크를 보냈습니다" }),
  ).toBeVisible();
  const magicMail = await waitForCapturedMail(
    (mail) => mail.to === owner.newEmail && mail.text.includes("/magic-link?token="),
  );
  expect(magicMail.text).toContain("Subject: FVOCI 로그인 링크");
  const magicToken = tokenFrom(magicMail.text, /magic-link\?token=([A-Za-z0-9_-]+)/);

  await page.goto(`/magic-link?token=${magicToken}`);
  await expect(page.getByRole("heading", { name: "매직 링크 로그인" })).toBeVisible();
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);

  // The owner of a team workspace is refused before the last-admin check.
  await page.goto("/settings/account");
  await page.locator("#withdraw-confirm").fill(owner.newPassword);
  await page.getByRole("button", { name: "탈퇴", exact: true }).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "워크스페이스 소유자는 이관 후 탈퇴할 수 있습니다." }),
  ).toBeVisible();
  await expect(page).toHaveURL(/\/settings\/account$/);

  // A plain member withdraws, then cancels from the mailed link.
  createE2eUser(member.email, member.password, member.givenName, {
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });
  await page.goto("/");
  await logout(page);
  await login(page, member.email, member.password);
  await page.goto("/settings/account");
  await page.locator("#withdraw-confirm").fill(member.password);
  await page.getByRole("button", { name: "탈퇴", exact: true }).click();
  await expect(page).toHaveURL(/\/cancel-withdraw#token=/);
  await expect(page.getByRole("heading", { name: "탈퇴 예약" })).toBeVisible();
  await expect(
    page.getByRole("status").filter({ hasText: "까지 탈퇴를 취소할 수 있습니다." }),
  ).toBeVisible();

  const cancelMail = await waitForCapturedMail(
    (mail) => mail.to === member.email && mail.text.includes("/cancel-withdraw#token="),
  );
  expect(cancelMail.text).toContain("Subject: FVOCI 탈퇴 취소");
  const cancelToken = tokenFrom(cancelMail.text, /cancel-withdraw#token=([A-Za-z0-9_-]+)/);

  // The session is gone, so login refuses the withdrawn account.
  await page.goto("/login");
  await page.getByLabel("이메일").fill(member.email);
  await page.getByLabel("비밀번호").fill(member.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(/\/login/);

  // Opening the link must not cancel by itself; only the button does.
  await page.goto(`/cancel-withdraw#token=${cancelToken}`);
  await expect(page.getByRole("heading", { name: "탈퇴 예약" })).toBeVisible();
  await expect(page.getByText("탈퇴 예약이 취소되었습니다.")).toHaveCount(0);
  await page.getByRole("button", { name: "탈퇴 취소" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "탈퇴 예약이 취소되었습니다." }),
  ).toBeVisible();

  await login(page, member.email, member.password);
});
