// Presigned attachment transfer (#149 B) in a real browser: the app on
// 127.0.0.1 and MinIO as a separate storage origin (localhost), so part PUTs,
// the download redirect and ranged fetches cross origins under the real CORS
// and CSP rules. Proxy mode is checked on the same server afterwards.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { crc32, deflateSync } from "node:zlib";
import { expect, test, type Page, type Request } from "@playwright/test";
import { watchCspViolations } from "../e2e/helpers";

const owner = {
  email: "transfer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "전송",
  workspaceSlug: "xfer",
  workspaceName: "Transfer",
};

const PART_SIZE = 5 * 1024 * 1024;

function storageOrigin(): string {
  const endpoint = process.env.S3_PUBLIC_ENDPOINT;
  if (!endpoint) throw new Error("S3_PUBLIC_ENDPOINT is required (scripts/run-web-e2e-s3.sh)");
  return new URL(endpoint).origin;
}

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

/** Two parts at the 5 MiB part size the runner configures. */
function patterned(seed: number): Buffer {
  const out = Buffer.alloc(PART_SIZE + 123_457);
  for (let i = 0; i < out.length; i += 1) out[i] = (i * 31 + seed) & 0xff;
  return out;
}

const sha256 = (bytes: Buffer | Uint8Array) => createHash("sha256").update(bytes).digest("hex");

async function setTransferMode(page: Page, mode: "proxy" | "presigned", label: string): Promise<void> {
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

type PageFetch = { status: number; redirected: boolean; url: string; range: string | null; sha: string; length: number };

/** `fetch` from the page, like the viewers do; hashes the body in the page. */
async function pageFetch(page: Page, url: string, init: RequestInit): Promise<PageFetch> {
  return page.evaluate(
    async ({ url, init }) => {
      const res = await fetch(url, init);
      const body = await res.arrayBuffer();
      const digest = await crypto.subtle.digest("SHA-256", body);
      return {
        status: res.status,
        redirected: res.redirected,
        url: res.url,
        range: res.headers.get("content-range"),
        sha: [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join(""),
        length: body.byteLength,
      };
    },
    { url, init },
  );
}

test("presigned mode moves bytes between the browser and storage; proxy mode is unchanged", async ({ page }) => {
  test.setTimeout(180_000);
  const storage = storageOrigin();
  const csp = watchCspViolations(page);
  const storagePuts: Request[] = [];
  const apiParts: string[] = [];
  page.on("request", (req) => {
    if (req.method() === "PUT" && req.url().startsWith(`${storage}/`)) storagePuts.push(req);
    if (req.method() === "PUT" && req.url().includes("/parts/")) apiParts.push(req.url());
  });

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

  // The page policy names the storage origin for fetches and images only.
  const shell = await page.request.get("/");
  const policy = shell.headers()["content-security-policy"] ?? "";
  expect(policy).toContain(`connect-src 'self' ${storage};`);
  expect(policy).toContain(`img-src 'self' data: blob: ${storage};`);

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
  const taskUrl = page.url();

  // ---- presigned
  await setTransferMode(page, "presigned", "직접 전송 (S3 서명 URL)");
  await page.goto(taskUrl);
  const panel = page.getByRole("region", { name: "첨부" });
  const big = patterned(3);
  const photo = pngBytes(2400, 600);
  const created: Record<string, unknown>[] = [];
  page.on("response", async (res) => {
    if (res.url().endsWith("/uploads") && res.request().method() === "POST" && res.status() === 201) {
      created.push((await res.json()) as Record<string, unknown>);
    }
  });
  await panel.getByLabel("파일 첨부").setInputFiles([
    { name: "big.bin", mimeType: "application/octet-stream", buffer: big },
    { name: "사진.png", mimeType: "image/png", buffer: photo },
  ]);
  await expect(panel.getByRole("link", { name: "big.bin" })).toBeVisible({ timeout: 60_000 });
  await expect(panel.getByRole("link", { name: "사진.png" })).toBeVisible();
  expect(created.map((c) => c.transfer)).toEqual(["presigned", "presigned"]);

  // Three part PUTs (two for big.bin, one for the PNG), all to storage with
  // nothing but the signed URL, and none through the API.
  expect(apiParts).toEqual([]);
  expect(storagePuts).toHaveLength(3);
  for (const req of storagePuts) {
    const headers = await req.allHeaders();
    expect(headers.cookie).toBeUndefined();
    expect(headers.authorization).toBeUndefined();
    expect(headers["content-type"]).toBeUndefined();
    const res = await req.response();
    expect(res?.status()).toBe(200);
    expect(res?.headers().etag).toMatch(/^"[0-9a-f-]+"$/);
    // The browser read the ETag through CORS (the upload could not complete
    // otherwise); the response allowed this origin.
    expect(res?.headers()["access-control-allow-origin"]).toBe(new URL(page.url()).origin);
  }

  const bigHref = await panel.getByRole("link", { name: "big.bin" }).getAttribute("href");
  expect(bigHref).toBeTruthy();
  const photoHref = await panel.getByRole("link", { name: "사진.png" }).getAttribute("href");
  const photoId = (photoHref as string).split("/attachments/")[1]?.split("/")[0] ?? "";

  // The download link: the API answers 302 and storage sends the bytes as an
  // attachment.
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    panel.getByRole("link", { name: "big.bin" }).click(),
  ]);
  const saved = await download.path();
  expect(sha256(readFileSync(saved))).toBe(sha256(big));

  // Viewer-style fetches follow the redirect: the whole file with the
  // session (like viewer-download.ts), and a range across the part boundary.
  const whole = await pageFetch(page, bigHref as string, { credentials: "include" });
  expect(whole).toMatchObject({ status: 200, redirected: true, length: big.length, sha: sha256(big) });
  expect(whole.url.startsWith(`${storage}/`)).toBe(true);
  const start = PART_SIZE - 10;
  const end = PART_SIZE + 9;
  const ranged = await pageFetch(page, bigHref as string, { headers: { Range: `bytes=${start}-${end}` } });
  expect(ranged).toMatchObject({
    status: 206,
    redirected: true,
    range: `bytes ${start}-${end}/${big.length}`,
    length: 20,
    sha: sha256(big.subarray(start, end + 1)),
  });

  // The original image viewer loads the PNG through the redirect (img-src).
  await page.goto(`/w/${owner.workspaceSlug}/a/${photoId}/view`);
  const img = page.locator("img.attachment-viewer__image");
  await expect(img).toBeVisible();
  await expect.poll(() => img.evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(2400);

  // ---- proxy: the same server switched back; bytes stay on the app origin.
  await setTransferMode(page, "proxy", "프록시 (API 서버 경유)");
  await page.goto(taskUrl);
  const proxied = patterned(9);
  const putsBefore = storagePuts.length;
  await page.getByRole("region", { name: "첨부" }).getByLabel("파일 첨부").setInputFiles([
    { name: "proxy.bin", mimeType: "application/octet-stream", buffer: proxied },
  ]);
  const proxyLink = page.getByRole("region", { name: "첨부" }).getByRole("link", { name: "proxy.bin" });
  await expect(proxyLink).toBeVisible({ timeout: 60_000 });
  expect(created.at(-1)?.transfer).toBe("proxy");
  expect(storagePuts).toHaveLength(putsBefore);
  expect(apiParts).toHaveLength(2);
  const proxyHref = (await proxyLink.getAttribute("href")) as string;
  const viaApi = await pageFetch(page, proxyHref, { credentials: "include" });
  expect(viaApi).toMatchObject({ status: 200, redirected: false, length: proxied.length, sha: sha256(proxied) });
  expect(viaApi.url.startsWith(new URL(page.url()).origin)).toBe(true);
  // Originals uploaded in presigned mode are now served by the API too.
  const bigViaApi = await pageFetch(page, bigHref as string, { credentials: "include" });
  expect(bigViaApi).toMatchObject({ status: 200, redirected: false, sha: sha256(big) });

  // Reset through the existing button returns to the default.
  await page.goto("/settings/admin");
  const card = page.getByRole("region", { name: "첨부 전송 방식", exact: true });
  await card.getByRole("button", { name: "기본값으로" }).click();
  await expect(card.getByText("오버라이드됨")).toHaveCount(0);
  await expect(card.getByText("현재 적용: 프록시 (API 서버 경유)")).toBeVisible();

  expect(csp).toEqual([]);
});
