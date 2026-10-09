import { expectVueViewer } from "./viewer-app";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { HwpDocument, initSync } from "@rhwp/core";
import {
  buildFixtureHwpx,
  FIXTURE_PAGES,
  readZip,
  writeZip,
} from "../src/features/attachments/hwp-test-fixture";
import { readJson, flowSchemas, watchCspViolations } from "./helpers";

const owner = {
  email: "hwp-viewer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "한글",
  workspaceSlug: "acme",
  workspaceName: "HWP Viewer",
};

const repoRoot = path.resolve(import.meta.dirname, "../../..");
const hancomHwp = fs.readFileSync(path.join(repoRoot, "compat/fixtures/sample.hwp"));
const hancomHwpx = fs.readFileSync(path.join(repoRoot, "compat/fixtures/sample.hwpx"));

/** Three-page Korean HWPX rewritten from the Hancom sample package (no HWP library involved). */
const threePageHwpx = Buffer.from(buildFixtureHwpx(new Uint8Array(hancomHwpx), FIXTURE_PAGES));

/** The same three pages as binary HWP 5.0, written by the pinned rhwp in Node. */
function threePageHwp(): Buffer {
  const coreDir = path.dirname(createRequire(import.meta.url).resolve("@rhwp/core"));
  initSync({ module: fs.readFileSync(path.join(coreDir, "rhwp_bg.wasm")) });
  const doc = new HwpDocument(new Uint8Array(threePageHwpx));
  try {
    return Buffer.from(doc.exportHwp());
  } finally {
    doc.free();
  }
}

/**
 * The review's HWPX counterexample: the Hancom sample plus four unreferenced
 * 32 MiB `Scripts/*` parts (about 140 KB on disk) that rhwp 0.8.6 would
 * inflate eagerly. The viewer's package budget refuses it before rhwp runs.
 */
const scriptsBombHwpx = Buffer.from(
  writeZip([
    ...readZip(new Uint8Array(hancomHwpx)),
    ...[0, 1, 2, 3].map((n) => ({
      name: `Scripts/s${String(n)}.js`,
      data: new Uint8Array(32 * 1024 * 1024),
    })),
  ]),
);

/**
 * A valid HWPX whose parse is slow on its own (about 18 s in Node for rhwp
 * 0.8.6; about 235 KB on disk, well inside the package budget), so the spec
 * can leave the viewer while the worker is still parsing — no test hook.
 */
const slowHwpx = Buffer.from(
  buildFixtureHwpx(
    new Uint8Array(hancomHwpx),
    Array.from({ length: 15000 }, (_, n) =>
      `${String(n + 1)}쪽 ${"가나다라마바사아자차 ".repeat(90)}`.trim(),
    ),
  ),
);

/** Running rhwp document workers (one per open HWP viewer). */
function hwpWorkers(page: Page): number {
  return page
    .workers()
    .filter((worker) => /\/assets\/hwp-worker-[^/]+\.js$/.test(new URL(worker.url()).pathname))
    .length;
}

function sha256(bytes: Buffer | Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

async function uploadAttachment(
  page: Page,
  wsId: string,
  documentId: string,
  name: string,
  bytes: Buffer,
): Promise<string> {
  const uploadRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/uploads`,
    { data: { name, sizeBytes: bytes.length } },
  );
  expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
  const upload = (await readJson(uploadRes, flowSchemas.upload)) as {
    attachmentId: string;
    partSizeBytes: number;
    parts: Array<{
      partNumber: number;
      url: string;
    }>;
  };
  const parts: {
    partNumber: number;
    etag: string;
  }[] = [];
  for (const part of upload.parts) {
    const put = await page.request.put(part.url, {
      headers: { "content-type": "application/octet-stream" },
      data: bytes.subarray(
        (part.partNumber - 1) * upload.partSizeBytes,
        part.partNumber * upload.partSizeBytes,
      ),
    });
    expect(put.ok(), await put.text()).toBeTruthy();
    const etag = put.headers()["etag"];
    expect(etag).toBeTruthy();
    const required1 = etag;
    if (required1 === undefined) {
      throw new Error("Missing fixture value: etag");
    }
    parts.push({ partNumber: part.partNumber, etag: required1 });
  }
  const completeRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`,
    { data: { parts } },
  );
  expect(completeRes.ok(), await completeRes.text()).toBeTruthy();
  return upload.attachmentId;
}

type PageProbe = {
  src: string;
  naturalWidth: number;
  cssWidth: number;
  /** Dark (ink) pixels on the rasterized page. */
  dark: number;
  /** 16×16 ink map; different text gives a different map. */
  signature: string;
};

/** Rasterizes the shown page SVG at `scale`× its natural size and measures its ink. */
async function probePage(
  page: Page,
  root = page.locator("[data-hwp-viewer]"),
  scale = 1,
): Promise<PageProbe> {
  const img = root.locator("img.hwp-viewer__page");
  await expect(img).toHaveJSProperty("complete", true);
  return img.evaluate((node, factor) => {
    const el = node as HTMLImageElement;
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(el.naturalWidth * factor);
    canvas.height = Math.round(el.naturalHeight * factor);
    const required2 = canvas.getContext("2d");
    if (required2 === null) {
      throw new Error('Missing fixture value: canvas.getContext("2d")');
    }
    const ctx = required2;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(el, 0, 0, canvas.width, canvas.height);
    const { data, width, height } = ctx.getImageData(0, 0, canvas.width, canvas.height);
    const grid = 16;
    const cells = new Array<number>(grid * grid).fill(0);
    let dark = 0;
    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const at = (y * width + x) * 4;
        const required3 = data[at];
        if (required3 === undefined) {
          throw new Error("Missing fixture value: data[at]");
        }
        const required4 = data[at + 1];
        if (required4 === undefined) {
          throw new Error("Missing fixture value: data[at + 1]");
        }
        const required5 = data[at + 2];
        if (required5 === undefined) {
          throw new Error("Missing fixture value: data[at + 2]");
        }
        if (required3 < 110 && required4 < 110 && required5 < 110) {
          dark += 1;
          const required6 =
            cells[Math.floor((y * grid) / height) * grid + Math.floor((x * grid) / width)];
          if (required6 === undefined) {
            throw new Error(
              "Missing fixture value: cells[Math.floor((y * grid) / height) * grid + Math.floor((x * grid) / width)]",
            );
          }
          cells[Math.floor((y * grid) / height) * grid + Math.floor((x * grid) / width)] =
            required6 + 1;
        }
      }
    }
    return {
      src: el.currentSrc,
      naturalWidth: el.naturalWidth,
      cssWidth: el.getBoundingClientRect().width,
      dark,
      signature: cells.map((n) => (n > 0 ? "1" : "0")).join(""),
    };
  }, scale);
}

/** Whether a blob URL still loads as an image (CSP allows blob: only for img-src). */
async function blobLoads(page: Page, src: string): Promise<boolean> {
  return page.evaluate(
    (url) =>
      new Promise<boolean>((resolve) => {
        const probe = new Image();
        probe.onload = () => {
          resolve(true);
        };
        probe.onerror = () => {
          resolve(false);
        };
        probe.src = url;
      }),
    src,
  );
}

test("HWP/HWPX attachments: rhwp layout pages, zoom, chunk jump, original download, session and share isolation", async ({
  page,
  browser,
}) => {
  test.setTimeout(180000);
  const csp = watchCspViolations(page);
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  const requested: string[] = [];
  const wasmResponses: {
    url: string;
    status: number;
    type: string;
    nosniff: string;
  }[] = [];
  page.on("request", (request) => requested.push(request.url()));
  page.on("response", (response) => {
    if (/\/assets\/rhwp_bg-[^/]+\.wasm$/.test(new URL(response.url()).pathname)) {
      const headers = response.headers();
      wasmResponses.push({
        url: response.url(),
        status: response.status(),
        type: headers["content-type"] ?? "",
        nosniff: headers["x-content-type-options"] ?? "",
      });
    }
  });

  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15000 });
  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const workspaces = (await readJson(
    await page.request.get("/api/v1/me/workspaces"),
    flowSchemas.workspaces,
  )) as {
    items: {
      id: string;
      slug: string;
    }[];
  };
  const required7 = workspaces.items.find((item) => item.slug === owner.workspaceSlug);
  if (required7 === undefined) {
    throw new Error(
      "Missing fixture value: workspaces.items.find((item) => item.slug === owner.workspaceSlug)",
    );
  }
  const wsId = required7.id;
  const docRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { commandId: crypto.randomUUID(), parentId: null, title: "HWP 첨부" },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const documentId = (
    (await readJson(docRes, flowSchemas.document)) as {
      id: string;
    }
  ).id;
  const hwpxId = await uploadAttachment(page, wsId, documentId, "품의서.hwpx", threePageHwpx);
  const binaryHwp = threePageHwp();
  const hwpId = await uploadAttachment(page, wsId, documentId, "보고서.hwp", binaryHwp);
  const hancomId = await uploadAttachment(page, wsId, documentId, "안녕.hwp", hancomHwp);
  const textId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "notes.txt",
    Buffer.from("plain attachment body\n", "utf8"),
  );
  const previewHtmlRequests: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/preview-html")) {
      previewHtmlRequests.push(request.url());
    }
  });

  // HWPX: three laid-out pages with distinct Korean ink, page navigation and zoom.
  await page.goto(`/w/acme/a/${hwpxId}/view`);
  await expectVueViewer(page);
  const shell = page.locator("[data-attachment-viewer]");
  const viewer = page.locator("[data-hwp-viewer]");
  await expect(viewer).toBeVisible({ timeout: 30000 });
  await expect(shell.locator(".attachment-viewer__name")).toHaveText("품의서.hwpx");
  await expect(viewer.getByText("1 / 3")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "이전 쪽" })).toBeDisabled();
  const first = await probePage(page);
  expect(first.src.startsWith("blob:")).toBe(true);
  // rhwp runs in one module worker for this document.
  expect(hwpWorkers(page)).toBe(1);
  expect(first.naturalWidth).toBeGreaterThan(700);
  expect(first.dark).toBeGreaterThan(2000);
  expect(Math.round(first.cssWidth)).toBe(Math.round(first.naturalWidth));

  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await expect(viewer.getByText("2 / 3")).toBeVisible();
  await expect.poll(async () => (await probePage(page)).src).not.toBe(first.src);
  const second = await probePage(page);
  expect(second.dark).toBeGreaterThan(2000);
  expect(second.signature).not.toBe(first.signature);
  // The previous page's blob URL is released.
  expect(await blobLoads(page, second.src)).toBe(true);
  expect(await blobLoads(page, first.src)).toBe(false);

  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await expect(viewer.getByText("3 / 3")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "다음 쪽" })).toBeDisabled();

  await viewer.getByRole("button", { name: "확대" }).click();
  await expect(viewer.getByText("125%")).toBeVisible();
  await expect
    .poll(async () => Math.round((await probePage(page)).cssWidth))
    .toBe(Math.round(first.naturalWidth * 1.25));
  await viewer.getByRole("button", { name: "축소" }).click();
  await viewer.getByRole("button", { name: "축소" }).click();
  await viewer.getByRole("button", { name: "축소" }).click();
  await expect(viewer.getByText("50%")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "축소" })).toBeDisabled();
  await expect
    .poll(async () => Math.round((await probePage(page)).cssWidth))
    .toBe(Math.round(first.naturalWidth * 0.5));
  await viewer.getByRole("button", { name: "원래 크기" }).click();
  await expect(viewer.getByText("100%")).toBeVisible();

  // One same-origin wasm module, served as application/wasm with nosniff.
  expect(wasmResponses.length).toBeGreaterThanOrEqual(1);
  for (const wasm of wasmResponses) {
    expect(wasm.status).toBe(200);
    expect(wasm.type).toContain("application/wasm");
    expect(wasm.nosniff).toBe("nosniff");
    expect(new URL(wasm.url).origin).toBe(new URL(page.url()).origin);
  }

  // Original download is the uploaded file, byte for byte.
  const downloadLink = shell.getByRole("link", { name: "다운로드" }).first();
  await expect(downloadLink).toHaveAttribute(
    "href",
    `/api/v1/workspaces/${wsId}/attachments/${hwpxId}/download`,
  );
  const original = await page.request.get(
    `/api/v1/workspaces/${wsId}/attachments/${hwpxId}/download`,
  );
  expect(sha256(await original.body())).toBe(sha256(threePageHwpx));

  // Search chunk N opens page N+1 (one chunk per fixture page); unknown chunks open page 1.
  for (const [chunk, label] of [
    [1, "2 / 3"],
    [2, "3 / 3"],
    [9, "1 / 3"],
  ] as const) {
    await page.goto(`/w/acme/a/${hwpxId}/view?chunk=${String(chunk)}`);
    await expect(viewer.getByText(label)).toBeVisible({ timeout: 30000 });
  }
  // Default (auto) preview mode: no search supplement and no preview-html call.
  await expect(page.locator("[data-chunk-supplement]")).toHaveCount(0);
  expect(previewHtmlRequests).toEqual([]);

  // Binary HWP 5.0: the same three pages, chunk jump included.
  await page.goto(`/w/acme/a/${hwpId}/view?chunk=1`);
  await expect(viewer.getByText("2 / 3")).toBeVisible({ timeout: 30000 });
  const hwpPage = await probePage(page);
  expect(hwpPage.dark).toBeGreaterThan(2000);
  expect(hwpPage.signature).not.toBe(first.signature);
  const hwpOriginal = await page.request.get(
    `/api/v1/workspaces/${wsId}/attachments/${hwpId}/download`,
  );
  expect(sha256(await hwpOriginal.body())).toBe(sha256(binaryHwp));

  // A Hancom-authored HWP (body "안녕").
  await page.goto(`/w/acme/a/${hancomId}/view`);
  await expect(viewer.getByText("1 / 1")).toBeVisible({ timeout: 30000 });
  const hancomPng = await viewer.locator("img.hwp-viewer__page").screenshot();
  await test.info().attach("hancom-hwp-page", { body: hancomPng, contentType: "image/png" });
  // Two 10pt glyphs: rasterize at 3× so antialiased strokes register as ink.
  const hancom = await probePage(page, viewer, 3);
  expect(hancom.dark).toBeGreaterThan(150);
  // The ink sits in the top-left body area (A4, 30 mm left / 35 mm top margins), nowhere else.
  expect(hancom.signature.indexOf("1")).toBeGreaterThanOrEqual(16 * 1);
  expect(hancom.signature.lastIndexOf("1")).toBeLessThan(16 * 4);
  const evidenceDir = process.env.FVOCI_HWP_EVIDENCE_DIR;
  if (evidenceDir) {
    fs.mkdirSync(evidenceDir, { recursive: true });
    fs.writeFileSync(path.join(evidenceDir, "hancom-hwp-page.png"), hancomPng);
    await page.goto(`/w/acme/a/${hwpxId}/view`);
    await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30000 });
    fs.writeFileSync(
      path.join(evidenceDir, "fixture-hwpx-page1.png"),
      await viewer.locator("img.hwp-viewer__page").screenshot(),
    );
  }

  // Server preview mode: the search supplement appears above the layout, never instead of it.
  const toServer = await page.request.patch("/api/v1/admin/instance-settings", {
    data: { attachmentPreview: { mode: "server" } },
  });
  expect(toServer.ok(), await toServer.text()).toBeTruthy();
  try {
    await page.goto(`/w/acme/a/${hwpxId}/view?chunk=1`);
    const supplement = page.locator("[data-chunk-supplement]");
    await expect(supplement).toContainText("레이아웃 없음", { timeout: 30000 });
    await expect(viewer.getByText("2 / 3")).toBeVisible({ timeout: 30000 });
    await expect(viewer.locator("img.hwp-viewer__page")).toBeVisible();
    const preview = await page.request.get(
      `/api/v1/workspaces/${wsId}/attachments/${hwpxId}/preview-html`,
    );
    if (process.env.FVOCI_EXTRACTOR_BIN) {
      // Native extract helper configured: the server parses the HWPX and the
      // supplement highlights the chunk's own text above the layout.
      expect(preview.status(), await preview.text()).toBe(200);
      const html = (
        (await readJson(preview, flowSchemas.unknown)) as {
          html: string;
        }
      ).html;
      expect(html).toContain("둘째 쪽 검색 대상");
      // Chunk 1 covers page 2 whichever paragraph separators the extractor emits.
      await expect(supplement.locator("mark")).toContainText("하늘과 바람과 별과 시");
      test
        .info()
        .annotations.push({ type: "supplement", description: "extracted text, chunk 1 marked" });
      if (evidenceDir) {
        fs.writeFileSync(
          path.join(evidenceDir, "server-mode-supplement.json"),
          `${JSON.stringify({ previewStatus: preview.status(), mark: (await supplement.locator("mark").textContent())?.slice(0, 80), labels: await viewer.locator(".attachment-viewer__page-label").allTextContents() }, null, 2)}\n`,
        );
      }
    } else {
      // No helper (the default CI job): no extract text, the supplement says so and the layout stays.
      expect([404, 413]).toContain(preview.status());
      await expect(supplement).toContainText("이 파일을 뷰어로 열 수 없습니다");
      test.info().annotations.push({
        type: "supplement",
        description: `no extractor, preview-html ${String(preview.status())}`,
      });
    }
    // Without a chunk the server mode adds nothing.
    previewHtmlRequests.length = 0;
    await page.goto(`/w/acme/a/${hwpxId}/view`);
    await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30000 });
    await expect(supplement).toHaveCount(0);
    expect(previewHtmlRequests).toEqual([]);
  } finally {
    const restore = await page.request.patch("/api/v1/admin/instance-settings", {
      data: { attachmentPreview: { mode: "auto" } },
    });
    expect(restore.ok(), await restore.text()).toBeTruthy();
  }

  // Switching attachments while the HWP bytes are in flight never paints the old file.
  // Leaving an open document terminates its worker (SPA navigation, same page).
  await page.goto(`/w/acme/a/${hwpxId}/view`);
  await expect(viewer.getByText("1 / 3")).toBeVisible({ timeout: 30000 });
  await expect.poll(() => hwpWorkers(page)).toBe(1);
  await page.evaluate((to) => {
    window.history.pushState({}, "", to);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${textId}/view`);
  await expect(page.getByText("plain attachment body")).toBeVisible();
  await expect.poll(() => hwpWorkers(page)).toBe(0);

  let releaseHwp!: () => void;
  const held = new Promise<void>((resolve) => {
    releaseHwp = resolve;
  });
  let hwpRequested!: () => void;
  const hwpStarted = new Promise<void>((resolve) => {
    hwpRequested = resolve;
  });
  const hwpDownload = `**/api/v1/workspaces/${wsId}/attachments/${hwpId}/download`;
  await page.route(hwpDownload, async (route) => {
    hwpRequested();
    await held;
    await route.continue().catch(() => undefined);
  });
  await page.goto(`/w/acme/a/${hwpId}/view`);
  await hwpStarted;
  await page.evaluate((to) => {
    window.history.pushState({}, "", to);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${textId}/view`);
  await expect(page.getByText("plain attachment body")).toBeVisible();
  releaseHwp();
  await page.waitForTimeout(500);
  await expect(page.locator("[data-hwp-viewer]")).toHaveCount(0);
  await expect(page.locator("img.hwp-viewer__page")).toHaveCount(0);
  await page.unroute(hwpDownload);
  expect(hwpWorkers(page)).toBe(0);

  // Leaving mid-parse terminates the worker at once, not when the parse or its deadline ends.
  const slowId = await uploadAttachment(page, wsId, documentId, "긴문서.hwpx", slowHwpx);
  await page.goto(`/w/acme/a/${slowId}/view`);
  await expect.poll(() => hwpWorkers(page), { timeout: 30000 }).toBe(1);
  await expect(viewer).toHaveCount(0);
  await page.evaluate((to) => {
    window.history.pushState({}, "", to);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${textId}/view`);
  await expect(page.getByText("plain attachment body")).toBeVisible();
  await expect.poll(() => hwpWorkers(page), { timeout: 3000 }).toBe(0);
  await expect(page.locator("img.hwp-viewer__page")).toHaveCount(0);

  // Share: the anonymous reader gets the same layout from share URLs only.
  const shareRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/share-links`,
    { data: { expiresInDays: 7 } },
  );
  expect(shareRes.status(), await shareRes.text()).toBe(201);
  const share = (await readJson(shareRes, flowSchemas.share)) as {
    id: string;
    url: string;
  };
  const sharePath = new URL(share.url).pathname;
  const required8 = sharePath.split("/")[2];
  if (required8 === undefined) {
    throw new Error('Missing fixture value: sharePath.split("/")[2]');
  }
  const token = required8;
  const anon = await browser.newContext();
  const reader = await anon.newPage();
  const readerCsp = watchCspViolations(reader);
  const readerApi: string[] = [];
  reader.on("request", (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith("/api/")) {
      readerApi.push(url.pathname);
    }
  });
  await reader.goto(`${sharePath}/attachments/${hwpxId}/view?chunk=2`);
  const readerViewer = reader.locator("[data-hwp-viewer]");
  await expect(readerViewer.getByText("3 / 3")).toBeVisible({ timeout: 30000 });
  const shared = await probePage(reader, readerViewer);
  expect(shared.dark).toBeGreaterThan(2000);
  await expect(reader.locator("[data-chunk-supplement]")).toHaveCount(0);
  await expect(reader.getByRole("button", { name: /편집/ })).toHaveCount(0);
  await expect(
    reader.locator("[data-attachment-viewer]").getByRole("link", { name: "다운로드" }).first(),
  ).toHaveAttribute("href", `/api/v1/share/${token}/attachments/${hwpxId}/download`);
  const revoke = await page.request.delete(`/api/v1/workspaces/${wsId}/share-links/${share.id}`);
  expect(revoke.ok(), await revoke.text()).toBeTruthy();
  await reader.goto(`${sharePath}/attachments/${hwpxId}/view`);
  await expect(reader.getByRole("alert")).toHaveText(
    "접근 권한이 없거나 존재하지 않는 항목입니다.",
  );
  await expect(reader.locator("[data-hwp-viewer]")).toHaveCount(0);
  expect(readerApi.length).toBeGreaterThan(0);
  for (const apiPath of readerApi) {
    expect(apiPath.startsWith("/api/v1/share/")).toBe(true);
    expect(apiPath).not.toMatch(/preview-html|edit-context|edit-copy/);
  }
  expect(readerCsp).toEqual([]);
  await anon.close();

  // The Scripts bomb stays download-only and leaves no worker behind.
  const bombId = await uploadAttachment(page, wsId, documentId, "스크립트.hwpx", scriptsBombHwpx);
  await page.goto(`/w/acme/a/${bombId}/view`);
  await expect(shell.getByRole("alert")).toHaveText(
    "이 파일을 뷰어로 열 수 없습니다. 원본을 다운로드하세요.",
    {
      timeout: 30000,
    },
  );
  await expect(page.locator("img.hwp-viewer__page")).toHaveCount(0);
  await expect.poll(() => hwpWorkers(page)).toBe(0);

  // Session revoked while the bytes are in flight: the viewer shows an error, not the document.
  let releaseRevoked!: () => void;
  const revokedHeld = new Promise<void>((resolve) => {
    releaseRevoked = resolve;
  });
  let revokedRequested!: () => void;
  const revokedStarted = new Promise<void>((resolve) => {
    revokedRequested = resolve;
  });
  const hwpxDownload = `**/api/v1/workspaces/${wsId}/attachments/${hwpxId}/download`;
  await page.route(hwpxDownload, async (route) => {
    revokedRequested();
    await revokedHeld;
    await route.continue().catch(() => undefined);
  });
  await page.goto(`/w/acme/a/${hwpxId}/view`);
  await revokedStarted;
  const cookies = await page.context().cookies();
  expect((await page.request.post("/api/v1/auth/logout")).ok()).toBe(true);
  await page.context().addCookies(cookies);
  releaseRevoked();
  await expect(shell.getByRole("alert")).toHaveText("불러오지 못했습니다.", { timeout: 30000 });
  await expect(page.locator("img.hwp-viewer__page")).toHaveCount(0);
  await page.unroute(hwpxDownload);
  const denied = await page.request.get(
    `/api/v1/workspaces/${wsId}/attachments/${hwpxId}/download`,
  );
  expect(denied.status()).toBe(401);

  // Everything the session page loaded (app, wasm, bytes, blobs) stayed on this origin.
  const origin = new URL(page.url()).origin;
  for (const url of requested) {
    const parsed = new URL(url);
    if (parsed.protocol === "blob:" || parsed.protocol === "data:") {
      continue;
    }
    expect(parsed.origin, url).toBe(origin);
  }
  expect(csp).toEqual([]);
  expect(pageErrors).toEqual([]);
});
