import { execFileSync } from "node:child_process";
import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login, logout, watchCspViolations } from "./helpers";
import { currentStep, freshCode, totp } from "./mfa-helpers";

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

async function inviteAccount(page: Page, email: string): Promise<string> {
  await page.goto("/w/acme/settings");
  await page
    .locator("summary")
    .filter({ hasText: /^멤버$/ })
    .click();
  await page.getByLabel("초대할 이메일").fill(email);
  await page.getByRole("button", { name: "초대", exact: true }).click();
  const link = page.getByRole("link").filter({ hasText: "/invite/" });
  await expect(link).toBeVisible();
  const href = await link.getAttribute("href");
  expect(href).toBeTruthy();
  return href!;
}

test("owner invites a second user who signs up, accepts, and appears in members", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const csp = watchCspViolations(page);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const guardedRequests: string[] = [];
  page.on("request", (request) => {
    if (/\/api\/v1\/(invitations\/|auth\/providers)/.test(request.url())) {
      guardedRequests.push(request.url());
    }
  });
  // Direct entry before installation follows the setup guard, without
  // starting public invitation/provider requests against an unready server.
  await page.goto("/invite/not-installed");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await expect(page.getByRole("button", { name: "시작하기" })).toBeVisible();
  expect(guardedRequests).toEqual([]);

  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/acme/settings");
  await page
    .locator("summary")
    .filter({ hasText: /^멤버$/ })
    .click();
  await page.getByLabel("초대할 이메일").fill(invited.email);
  await page.getByRole("button", { name: "초대", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "초대를 만들었습니다" })).toBeVisible();
  const inviteLink = page.getByRole("link").filter({ hasText: "/invite/" });
  await expect(inviteLink).toBeVisible();
  const href = await inviteLink.getAttribute("href");
  const token = href?.split("/invite/")[1];
  expect(token).toBeTruthy();

  // Expire a second real invitation in this run's isolated DB. The normal
  // invitation API creates it; the browser still reads the Rust endpoint.
  await page.getByLabel("초대할 이메일").fill("expired-invite@example.com");
  await page.getByRole("button", { name: "초대", exact: true }).click();
  await expect(inviteLink).not.toHaveAttribute("href", href!);
  const expiredHref = await inviteLink.getAttribute("href");
  expect(expiredHref).toBeTruthy();
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!adminUrl || !container) throw new Error("isolated PostgreSQL fixture is required");
  const database = new URL(adminUrl).pathname.slice(1);
  const updated = execFileSync(
    "docker",
    [
      "exec",
      container,
      "psql",
      "-U",
      "postgres",
      "-d",
      database,
      "-v",
      "ON_ERROR_STOP=1",
      "-c",
      "UPDATE fvoci.invitations SET expires_at = now() - interval '1 second' WHERE email = 'expired-invite@example.com' AND accepted_at IS NULL",
    ],
    { encoding: "utf8", stdio: "pipe" },
  );
  expect(updated.trim()).toBe("UPDATE 1");

  await logout(page);

  for (const path of ["/invite/not-a-valid-token", expiredHref!]) {
    await page.goto(path);
    await expect(page.getByRole("alert")).toContainText("초대를 찾을 수 없거나 만료되었습니다");
    await expect(page.getByRole("button", { name: "수락", exact: true })).toHaveCount(0);
    await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  }

  await page.goto(`/invite/${token}`);
  await expect(page.getByRole("heading", { name: /초대 수락/ })).toBeVisible();
  await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  await page.reload();
  await expect(page.getByRole("heading", { name: /초대 수락/ })).toBeVisible();
  await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  await page.getByLabel("이메일").fill("wrong-invitee@example.com");
  await page.getByLabel("성").fill(invited.familyName);
  await page.getByLabel("이름", { exact: true }).fill(invited.givenName);
  await page.getByLabel("비밀번호").fill(invited.password);
  const rejected = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/v1/invitations/${token}/accept`) &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "수락" }).click();
  const rejection = await rejected;
  expect(rejection.status()).toBe(401);
  expect((await rejection.json()).code).toBe("cannot_accept_invitation");
  await expect(page.getByRole("alert")).toBeVisible();
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
  // A refused email leaves the invitation usable for its intended account.
  await page.getByLabel("이메일").fill(invited.email);
  await page.getByRole("button", { name: "수락" }).click();

  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByText(owner.workspaceName)).toBeVisible();

  await page.goto("/w/acme/settings");
  await page
    .locator("summary")
    .filter({ hasText: /^멤버$/ })
    .click();
  await expect(page.getByText(invited.email)).toBeVisible();
  await expect(page.getByText("박초대수락")).toBeVisible();
  await expect(page.getByText("멤버", { exact: true }).nth(1)).toBeVisible();
  // The accepted token cannot grant membership a second time.
  await page.goto(`/invite/${token}`);
  await expect(page.getByRole("alert")).toContainText("초대를 찾을 수 없거나 만료되었습니다");
  await expect(page.getByRole("button", { name: "수락", exact: true })).toHaveCount(0);
  expect(errors).toEqual([]);
  expect(csp).toEqual([]);
});

test("an existing account accepts with its password and keeps its profile", async ({ page }) => {
  const existing = {
    email: "existing-invite@example.com",
    password: "existingpass1",
    givenName: "기존사용자",
  };
  createE2eUser(existing.email, existing.password, existing.givenName, { familyName: "최" });
  await login(page, owner.email, owner.password);
  const href = await inviteAccount(page, existing.email);
  await logout(page);

  await page.goto(href);
  await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  // Existing accounts need only a password; optional signup fields stay empty.
  await expect(page.getByLabel("이메일")).toHaveValue("");
  await expect(page.getByLabel("이름", { exact: true })).toHaveValue("");
  await page.getByLabel("비밀번호").fill("wrongpassword1");
  const rejected = page.waitForResponse(
    (response) => response.url().endsWith("/accept") && response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "수락", exact: true }).click();
  expect((await rejected).status()).toBe(401);
  await expect(page.getByRole("alert")).toBeVisible();
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
  await page.getByLabel("비밀번호").fill(existing.password);
  await page.getByRole("button", { name: "수락", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  const me = await page.request.get("/api/v1/auth/me");
  expect(me.status()).toBe(200);
  expect(await me.json()).toMatchObject({
    email: existing.email,
    givenName: existing.givenName,
    familyName: "최",
  });
  await page.goto("/w/acme/settings");
  await page
    .locator("summary")
    .filter({ hasText: /^멤버$/ })
    .click();
  await expect(page.getByText(existing.email)).toBeVisible();
  await expect(page.getByText("최기존사용자")).toBeVisible();
});

test("required legal consent gates invitation acceptance before the real MFA challenge", async ({
  page,
}) => {
  const existing = {
    email: "mfa-invite@example.com",
    password: "mfainvitepass1",
    givenName: "초대MFA",
  };
  createE2eUser(existing.email, existing.password, existing.givenName);
  await login(page, existing.email, existing.password);
  await page.goto("/settings/account");
  const mfa = page.getByTestId("mfa-section");
  await mfa.locator("#settings-mfa-confirm").fill(existing.password);
  await mfa.getByRole("button", { name: "설정", exact: true }).click();
  const secret = (await mfa.getByTestId("mfa-secret").textContent())?.trim() ?? "";
  expect(secret).toMatch(/^[A-Z2-7=\s]+$/i);
  const enableStep = currentStep();
  await mfa.locator("#settings-mfa-code").fill(totp(secret, enableStep));
  await mfa.getByRole("button", { name: "켜기", exact: true }).click();
  await expect(mfa.getByRole("status").filter({ hasText: "2단계 인증을 켰습니다." })).toBeVisible();
  await mfa.getByRole("button", { name: "보관했습니다", exact: true }).click();
  await page.goto("/");
  await logout(page);

  // Publish a real required document through the existing administration UI.
  await login(page, owner.email, owner.password);
  await page.goto("/settings/legal");
  await page.getByLabel("법적 문서 제목").fill("초대 이용약관");
  await page
    .getByLabel("본문(마크다운)")
    .fill("## 초대 약관\n\n초대 수락에는 이 약관의 동의가 필요합니다.");
  await page.getByLabel("발효일").fill("2026-01-01");
  await expect(page.getByLabel("필수 법적 문서")).toBeChecked();
  await page.getByRole("button", { name: "발행", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "발행되었습니다." })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { name: "법적 문서 동의" })).toBeVisible();
  await page.getByRole("checkbox", { name: "동의합니다" }).check();
  await page.getByRole("button", { name: "동의하고 계속" }).click();
  await expect(page).toHaveURL(/\/settings\/legal$/);
  const href = await inviteAccount(page, existing.email);
  await logout(page);

  const csp = watchCspViolations(page);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(href);
  await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  const consent = page.getByRole("checkbox", { name: "초대 이용약관", exact: true });
  const accept = page.getByRole("button", { name: "수락", exact: true });
  await expect(consent).not.toBeChecked();
  await page.getByLabel("비밀번호").fill(existing.password);
  await expect(accept).toBeDisabled();
  const legalLink = page.getByRole("link", { name: "보기", exact: true });
  await expect(legalLink).toHaveAttribute("href", "/legal/terms");
  await expect(legalLink).toHaveAttribute("target", "_blank");
  const popupPromise = page.waitForEvent("popup");
  await legalLink.click();
  const popup = await popupPromise;
  await expect(popup.getByRole("heading", { name: "초대 이용약관", exact: true })).toBeVisible();
  await popup.close();
  await consent.check();
  await expect(accept).toBeEnabled();
  await consent.uncheck();
  await expect(accept).toBeDisabled();
  await consent.check();
  const accepted = page.waitForResponse(
    (response) => response.url().endsWith("/accept") && response.request().method() === "POST",
  );
  await accept.click();
  const result = await accepted;
  expect(result.status()).toBe(200);
  expect((await result.json()).mfaToken).toBeTruthy();
  await expect(page.getByRole("heading", { name: "2단계 인증", exact: true })).toBeVisible();
  await expect(page).toHaveURL(href);
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
  await page.getByLabel("인증 코드", { exact: true }).fill("abcdef");
  await page.getByRole("button", { name: "확인", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
  await page.getByLabel("인증 코드", { exact: true }).fill(freshCode(secret, enableStep));
  await page.getByRole("button", { name: "확인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  const me = await page.request.get("/api/v1/auth/me");
  expect(me.status()).toBe(200);
  expect((await me.json()).email).toBe(existing.email);
  const pending = await page.request.get("/api/v1/auth/consents/pending");
  expect(pending.status()).toBe(200);
  expect((await pending.json()).pending).toEqual([]);
  await expect(page.getByText(owner.workspaceName)).toBeVisible();
  await page.goto(href);
  await expect(page.getByRole("alert")).toContainText("초대를 찾을 수 없거나 만료되었습니다");
  expect(errors).toEqual([]);
  expect(csp).toEqual([]);
});
