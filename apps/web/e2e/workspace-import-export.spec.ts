import path from "node:path";
import { expect, test } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const importZip = path.resolve(import.meta.dirname, "fixtures/markdown-import.zip");

test("owner imports markdown zip and exports document markdown", async ({ page }) => {
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
  await expect(page.getByLabel("가져올 형식")).toBeVisible();
  await page.locator('input[type="file"]').setInputFiles(importZip);
  await expect(page.getByText("가져오기를 시작했습니다")).toBeVisible({ timeout: 30_000 });

  await page.goto("/w/acme/wiki");
  await page.getByRole("link", { name: "e2e-note" }).click();
  await expect(page.getByLabel("문서 제목")).toHaveValue("e2e-note");

  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("button", { name: "Markdown" }).click();
  const download = await downloadPromise;
  const suggested = download.suggestedFilename();
  expect(suggested.endsWith(".md")).toBe(true);
  const text = await download.createReadStream().then(async (stream) => {
    const chunks: Buffer[] = [];
    for await (const chunk of stream) {
      chunks.push(Buffer.from(chunk));
    }
    return Buffer.concat(chunks).toString("utf8");
  });
  expect(text).toContain("E2E note");
});
