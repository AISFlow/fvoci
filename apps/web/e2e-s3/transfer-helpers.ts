// Shared steps of the S3 transfer checks (run by scripts/run-web-e2e-s3.sh).
import { expect, type Page } from "@playwright/test";

const owner = {
  email: "transfer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "전송",
  workspaceSlug: "xfer",
  workspaceName: "Transfer",
};

export const WORKSPACE_SLUG = owner.workspaceSlug;

export function storageOrigin(): string {
  const endpoint = process.env.S3_PUBLIC_ENDPOINT;
  if (!endpoint) throw new Error("S3_PUBLIC_ENDPOINT is required (scripts/run-web-e2e-s3.sh)");
  return new URL(endpoint).origin;
}

/**
 * First-run setup of the owner and workspace on a fresh server, then one
 * project with one task; returns the task page URL.
 */
export async function setUpOwnerTask(page: Page): Promise<string> {
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

  await page.goto(`/w/${owner.workspaceSlug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("xfr");
  await page.getByLabel("이름", { exact: true }).fill("Transfer");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/XFR/tasks$`));
  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("전송 대상");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/XFR-2$`));
  return page.url();
}

export async function setTransferMode(page: Page, mode: "proxy" | "presigned", label: string): Promise<void> {
  await page.goto("/settings/admin");
  const card = page.getByRole("region", { name: "첨부 전송 방식", exact: true });
  await expect(card.locator('option[value="presigned"]')).toBeEnabled();
  await card.getByLabel("attachmentTransfer.mode").selectOption(mode);
  const saved = page.waitForResponse(
    (res) => res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH",
  );
  await card.getByRole("button", { name: "저장" }).click();
  expect((await saved).status()).toBe(200);
  await expect(card.getByText(`현재 적용: ${label}`)).toBeVisible();
  await expect(card.getByText("오버라이드됨")).toBeVisible();
}
