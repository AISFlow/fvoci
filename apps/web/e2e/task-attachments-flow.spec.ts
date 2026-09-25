import { crc32, deflateSync } from "node:zlib";
import { expect, test, type Page } from "@playwright/test";

const owner = {
  email: "owner@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "첨부",
  workspaceSlug: "tatt",
  workspaceName: "Task Attachments",
};

/** A real width×height RGB PNG (gradient), built without extra packages. */
function pngBytes(width: number, height: number): Buffer {
  const chunk = (type: string, data: Buffer): Buffer => {
    const head = Buffer.alloc(4);
    head.writeUInt32BE(data.length);
    const typed = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(typed));
    return Buffer.concat([head, typed, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr.set([8, 2, 0, 0, 0], 8);
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y += 1) {
    const row = y * (width * 3 + 1);
    for (let x = 0; x < width; x += 1) {
      raw[row + 1 + x * 3] = x % 256;
      raw[row + 2 + x * 3] = y % 256;
      raw[row + 3 + x * 3] = 128;
    }
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

async function workspaceId(page: Page, slug: string): Promise<string> {
  const res = await page.request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const ws = (await res.json()).items.find((item: { slug: string }) => item.slug === slug);
  expect(ws).toBeTruthy();
  return ws.id;
}

test("task attachments: pick, preview thumbnail, download, delete", async ({ page }) => {
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

  await page.goto(`/w/${owner.workspaceSlug}/projects`);
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("att");
  await page.getByLabel("이름", { exact: true }).fill("Attachments");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/ATT/tasks$`));
  await page.getByRole("button", { name: "새 태스크" }).click();
  await page.getByLabel("제목").fill("첨부 대상");
  await page.getByRole("dialog").getByRole("button", { name: "태스크 만들기" }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/ATT-2$`));

  const panel = page.getByRole("region", { name: "첨부" });
  await expect(panel.getByText("파일 첨부")).toBeVisible();
  await panel.getByLabel("파일 첨부").setInputFiles([
    { name: "사진.png", mimeType: "image/png", buffer: pngBytes(2400, 600) },
    { name: "메모.txt", mimeType: "text/plain", buffer: Buffer.from("첨부 메모") },
  ]);
  const photo = panel.getByRole("link", { name: "사진.png" });
  await expect(photo).toBeVisible();
  await expect(panel.getByRole("link", { name: "메모.txt" })).toBeVisible();

  // The download link serves the original bytes.
  const href = await photo.getAttribute("href");
  expect(href).toBeTruthy();
  const original = await page.request.get(href as string);
  expect(original.status()).toBe(200);
  expect((await original.body()).subarray(1, 4).toString("ascii")).toBe("PNG");

  // The in-process preview job publishes a 1600px-wide WebP; the panel then
  // renders it from ?variant=preview.
  const attachmentId = (href as string).split("/attachments/")[1]?.split("/")[0] ?? "";
  const wsId = await workspaceId(page, owner.workspaceSlug);
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/v1/workspaces/${wsId}/attachments/${attachmentId}`)).json())
          .preview,
      { timeout: 30_000 },
    )
    .toEqual({ width: 1600, height: 400 });
  await page.reload();
  const thumb = panel.locator(`img[src$="/attachments/${attachmentId}/download?variant=preview"]`);
  await expect(thumb).toBeVisible();
  await expect
    .poll(() => thumb.evaluate((img: HTMLImageElement) => img.naturalWidth))
    .toBe(1600);
  const preview = await page.request.get(`${href}?variant=preview`);
  expect(preview.status()).toBe(200);
  expect(preview.headers()["content-type"]).toBe("image/webp");

  // Delete with confirmation; the item and its download disappear.
  const memoRow = panel.getByRole("listitem").filter({ hasText: "메모.txt" });
  await memoRow.getByRole("button", { name: "삭제" }).click();
  const confirm = page.getByRole("alertdialog", { name: "첨부를 삭제할까요?" });
  await expect(confirm).toContainText("메모.txt 파일이 삭제됩니다.");
  await confirm.getByRole("button", { name: "삭제" }).click();
  await expect(panel.getByRole("link", { name: "메모.txt" })).toHaveCount(0);
  await expect(photo).toBeVisible();
});
