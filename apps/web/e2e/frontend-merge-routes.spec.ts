import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { buildFixtureHwpx, FIXTURE_PAGES } from "../src/features/attachments/hwp-test-fixture";
import { expectVueViewer } from "./viewer-app";

// Integration-only coverage: keep the accepted parent suites unchanged and
// exercise the viewer guard against destinations added by the other merges.
async function linkTo(page: Page, href: string): Promise<void> {
  await page.evaluate((destination) => {
    document.querySelector("#merge-destination")?.remove();
    const link = document.createElement("a");
    link.id = "merge-destination";
    link.href = destination;
    link.textContent = "Merge destination";
    document.body.prepend(link);
  }, href);
  await page.locator("#merge-destination").click();
}

test("merged viewer guards auth, wiki and project destinations within the same Vue runtime", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  // A Vue-to-Vue confirmation must never fall through to native unloading.
  page.on("dialog", async (dialog) => {
    errors.push(`unexpected native dialog: ${dialog.type()}`);
    await dialog.dismiss();
  });
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("통합");
  await page.getByLabel("이메일").fill("merge-routes@example.com");
  await page.getByLabel("비밀번호").fill("mergepass123");
  await page.getByLabel("워크스페이스 이름").fill("Merged routes");
  await page.getByLabel("주소(영문)").fill("merged");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/merged/projects");
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("MERGE");
  await page.getByLabel("이름", { exact: true }).fill("Merged project");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(/\/MERGE\/tasks$/);
  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("Merged task");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page.getByRole("heading", { name: "Merged task" })).toBeVisible();
  const taskPath = new URL(page.url()).pathname;
  const workspaces = await (await page.request.get("/api/v1/me/workspaces")).json();
  const wsId = workspaces.items.find((item: { slug: string }) => item.slug === "merged").id;
  const created = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: "Merged wiki" },
  });
  expect(created.ok(), await created.text()).toBe(true);
  const document = await created.json();
  const sample = fs.readFileSync(path.resolve(import.meta.dirname, "../../../compat/fixtures/sample.hwpx"));
  const bytes = Buffer.from(buildFixtureHwpx(sample, FIXTURE_PAGES));
  const reserved = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${document.id}/uploads`, {
    data: { name: "merged.hwpx", sizeBytes: bytes.length },
  });
  expect(reserved.ok(), await reserved.text()).toBe(true);
  const upload = await reserved.json();
  const parts: { partNumber: number; etag: string }[] = [];
  for (const part of upload.parts) {
    const put = await page.request.put(part.url, {
      headers: { "content-type": "application/octet-stream" },
      data: bytes.subarray((part.partNumber - 1) * upload.partSizeBytes, part.partNumber * upload.partSizeBytes),
    });
    expect(put.ok(), await put.text()).toBe(true);
    parts.push({ partNumber: part.partNumber, etag: put.headers()["etag"]! });
  }
  const completed = await page.request.post(`/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`, {
    data: { parts },
  });
  expect(completed.ok(), await completed.text()).toBe(true);
  const viewerPath = `/w/merged/a/${upload.attachmentId}/view`;
  const destinations = [
    { href: "/reset-password?token=merge-only#form", ready: () => page.getByRole("button", { name: "비밀번호 변경", exact: true }) },
    { href: `/w/merged/${document.displayId}?from=viewer#wiki`, ready: () => page.locator(".tiptap") },
    { href: `${taskPath}?from=viewer#task-comments`, ready: () => page.getByRole("heading", { name: "Merged task" }) },
    { href: "/w/merged/MERGE/tasks?from=viewer#list", ready: () => page.getByRole("heading", { name: "Merged project" }) },
  ];
  for (const { href, ready } of destinations) {
    await page.goto(viewerPath);
    await expectVueViewer(page);
    const viewer = page.locator("[data-hwp-viewer]");
    await expect(viewer.getByText("1 / 3")).toBeVisible();
    await viewer.getByRole("button", { name: "간단 편집" }).click();
    const bar = page.locator("[data-hwp-edit-bar]");
    await bar.getByLabel("찾을 문자열").fill("첫째");
    await bar.getByLabel("바꿀 문자열").fill("통합");
    await bar.getByRole("button", { name: "모두 바꾸기" }).click();
    await expect(bar.getByRole("button", { name: "편집본 저장" })).toBeEnabled();
    const ink = await viewer.locator("img.hwp-viewer__page").getAttribute("src");
    await page.evaluate(() => { (window as unknown as { mergeMarker: string }).mergeMarker = "same-runtime"; });
    await linkTo(page, href);
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "취소", exact: true }).click();
    expect(new URL(page.url()).pathname).toBe(viewerPath);
    await expect(bar.getByRole("button", { name: "편집본 저장" })).toBeEnabled();
    await expect(viewer.locator("img.hwp-viewer__page")).toHaveAttribute("src", ink!);
    await page.locator("#merge-destination").click();
    await dialog.getByRole("button", { name: "나가기", exact: true }).click();
    await expect(page).toHaveURL(new URL(href, page.url()).href);
    await expect(ready()).toBeVisible();
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
    expect(await page.evaluate(() => (window as unknown as { mergeMarker?: string }).mergeMarker)).toBe("same-runtime");
    await expect(page.locator("[data-hwp-viewer]")).toHaveCount(0);
    await expect(dialog).toHaveCount(0);
  }
  expect(errors).toEqual([]);
});
