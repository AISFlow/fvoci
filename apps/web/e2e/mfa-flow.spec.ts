import { createHmac } from "node:crypto";
import { expect, test, type Page } from "@playwright/test";
import { logout } from "./helpers";
import { qrModules } from "../src/lib/qr";

const owner = {
  email: "mfa-owner@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "보안",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const STEP_SECONDS = 30;

function base32Decode(input: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  const clean = input.replace(/[\s=]/g, "").toUpperCase();
  let bits = 0;
  let value = 0;
  const out: number[] = [];
  for (const char of clean) {
    const index = alphabet.indexOf(char);
    if (index < 0) throw new Error(`invalid base32 character: ${char}`);
    value = (value << 5) | index;
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return Buffer.from(out);
}

// RFC 6238 TOTP: HMAC-SHA1, 30 s step, 6 digits.
function totp(secret: string, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const digest = createHmac("sha1", base32Decode(secret)).update(counter).digest();
  const offset = digest[digest.length - 1] & 0x0f;
  const binary =
    ((digest[offset] & 0x7f) << 24) |
    (digest[offset + 1] << 16) |
    (digest[offset + 2] << 8) |
    digest[offset + 3];
  return String(binary % 1_000_000).padStart(6, "0");
}

function currentStep(): number {
  return Math.floor(Date.now() / 1000 / STEP_SECONDS);
}

// The server rejects a second use of a time step but accepts one step of
// clock drift either way, so the step after `usedStep` is valid right away.
function freshCode(secret: string, usedStep: number): string {
  return totp(secret, Math.max(currentStep(), usedStep + 1));
}

async function passwordStep(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  // No session yet: the second step replaces the login form.
  await expect(page.getByRole("heading", { name: "2단계 인증" })).toBeVisible();
  await expect(page).toHaveURL(/\/login$/);
  expect((await page.request.get("/api/v1/auth/me")).ok()).toBe(false);
}

async function submitMfaCode(page: Page, code: string): Promise<void> {
  await page.getByLabel("인증 코드", { exact: true }).fill(code);
  await page.getByRole("button", { name: "확인", exact: true }).click();
}

test("TOTP MFA: setup, enable, login challenge with TOTP and single-use recovery code", async ({
  page,
}) => {
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

  // Setup re-enters the password, then shows the secret for manual entry.
  await page.getByRole("link", { name: "계정", exact: true }).click();
  await expect(page).toHaveURL(/\/settings\/account$/);
  const mfa = page.getByTestId("mfa-section");
  await expect(mfa.getByTestId("mfa-status")).toHaveText("사용 안 함 — 로그인에 비밀번호만 씁니다.");
  await mfa.locator("#settings-mfa-confirm").fill(owner.password);
  await mfa.getByRole("button", { name: "설정", exact: true }).click();
  const secretText = (await mfa.getByTestId("mfa-secret").textContent())?.trim() ?? "";
  expect(secretText).toMatch(/^[A-Z2-7=\s]+$/i);
  // The otpauth URI is only a link target and a QR code (source QrSvg), not text.
  const otpauthLink = mfa.getByTestId("mfa-otpauth-uri");
  await expect(otpauthLink).toHaveAttribute("href", /^otpauth:\/\/totp\//);
  await expect(otpauthLink).not.toContainText("otpauth");
  const otpauthUri = (await otpauthLink.getAttribute("href")) ?? "";
  await expect(mfa.getByTestId("mfa-qr")).toBeVisible();
  const qrPath = await mfa.getByTestId("mfa-qr").locator("path").getAttribute("d");
  expect(qrPath).toBe(qrModules(otpauthUri).path);

  // A wrong code keeps MFA off.
  await mfa.locator("#settings-mfa-code").fill("abcdef");
  await mfa.getByRole("button", { name: "켜기" }).click();
  await expect(mfa.getByRole("alert")).toBeVisible();

  const enableStep = currentStep();
  await mfa.locator("#settings-mfa-code").fill(totp(secretText, enableStep));
  await mfa.getByRole("button", { name: "켜기" }).click();
  await expect(mfa.getByRole("status").filter({ hasText: "2단계 인증을 켰습니다." })).toBeVisible();
  const recoveryCodes = (await mfa.getByTestId("mfa-recovery-codes").locator("li").allTextContents())
    .map((code) => code.trim());
  expect(recoveryCodes).toHaveLength(10);
  for (const code of recoveryCodes) expect(code).toMatch(/^\S{4}-\S{4}-\S{4}$/);
  await mfa.getByRole("button", { name: "보관했습니다" }).click();
  await expect(mfa.getByTestId("mfa-status")).toHaveText("사용 중 · 복구 코드 10개 남음");

  // Password login now stops at the MFA step; a fresh TOTP step signs in.
  await page.goto("/");
  await logout(page);
  await passwordStep(page);
  await submitMfaCode(page, freshCode(secretText, enableStep));
  await expect(page).toHaveURL(/\/$/);
  expect((await page.request.get("/api/v1/auth/me")).ok()).toBe(true);

  // A recovery code signs in once.
  await logout(page);
  await passwordStep(page);
  await submitMfaCode(page, recoveryCodes[0]);
  await expect(page).toHaveURL(/\/$/);
  await page.goto("/settings/account");
  await expect(page.getByTestId("mfa-status")).toHaveText("사용 중 · 복구 코드 9개 남음");

  // ...and is rejected the second time.
  await page.goto("/");
  await logout(page);
  await passwordStep(page);
  await submitMfaCode(page, recoveryCodes[0]);
  await expect(
    page.getByRole("alert").filter({ hasText: "인증 코드가 맞지 않습니다. 다시 확인해 주세요." }),
  ).toBeVisible();
  await expect(page).toHaveURL(/\/login$/);
  expect((await page.request.get("/api/v1/auth/me")).ok()).toBe(false);

  // Back to the start, then another unused recovery code still works.
  await page.getByRole("button", { name: "처음부터 다시 로그인" }).click();
  await passwordStep(page);
  await submitMfaCode(page, recoveryCodes[1]);
  await expect(page).toHaveURL(/\/$/);
});
