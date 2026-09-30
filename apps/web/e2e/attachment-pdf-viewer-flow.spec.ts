import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { buildFixturePdf, FIXTURE_PAGE_W } from "../src/features/attachments/pdf-test-fixture";
import { readJson, flowSchemas, watchCspViolations } from "./helpers";
import { expectVueViewer } from "./viewer-app";

const owner = {
  email: "pdf-viewer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "뷰어",
  workspaceSlug: "acme",
  workspaceName: "PDF Viewer",
};

const PAGE_W = FIXTURE_PAGE_W;
const PDFJS_ASSETS = "/assets/pdfjs-dist-6.3.289/";

async function uploadAttachment(
  page: Page,
  wsId: string,
  documentId: string,
  name: string,
  bytes: Buffer | Uint8Array,
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
      // Only a Buffer is sent as raw bytes; a plain Uint8Array would be JSON-encoded.
      data: Buffer.from(bytes).subarray(
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

type CanvasProbe = {
  cssWidth: number;
  /** Colour class at a point given as fractions of the page box. */
  at: Record<string, "red" | "blue" | "white" | "dark" | "other">;
  /** Dark (text) pixels per horizontal band, as fractions of page height from the top. */
  darkInTextBand: number;
  darkInEmptyBand: number;
};

async function probeCanvas(page: Page): Promise<CanvasProbe> {
  return page.locator("[data-pdf-viewer] canvas").evaluate((node) => {
    const canvas = node as HTMLCanvasElement;
    const required2 = canvas.getContext("2d");
    if (required2 === null) {
      throw new Error('Missing fixture value: canvas.getContext("2d")');
    }
    const ctx = required2;
    const { width, height } = canvas;
    const data = ctx.getImageData(0, 0, width, height).data;
    const px = (x: number, y: number) => {
      const i = (Math.floor(y) * width + Math.floor(x)) * 4;
      const required3 = data[i];
      if (required3 === undefined) {
        throw new Error("Missing fixture value: data[i]");
      }
      const required4 = data[i + 1];
      if (required4 === undefined) {
        throw new Error("Missing fixture value: data[i + 1]");
      }
      const required5 = data[i + 2];
      if (required5 === undefined) {
        throw new Error("Missing fixture value: data[i + 2]");
      }
      return [required3, required4, required5] as const;
    };
    const classify = ([r, g, b]: readonly [number, number, number]) => {
      if (r > 200 && g < 60 && b < 60) {
        return "red" as const;
      }
      if (b > 200 && r < 60 && g < 60) {
        return "blue" as const;
      }
      if (r > 230 && g > 230 && b > 230) {
        return "white" as const;
      }
      if (r < 90 && g < 90 && b < 90) {
        return "dark" as const;
      }
      return "other" as const;
    };
    const dark = (top: number, bottom: number) => {
      let count = 0;
      for (let y = Math.floor(top * height); y < Math.floor(bottom * height); y += 1) {
        for (let x = 0; x < width; x += 1) {
          if (classify(px(x, y)) === "dark") {
            count += 1;
          }
        }
      }
      return count;
    };
    // PDF y grows upward: text baseline at 130pt, 36pt glyphs; empty strip 90–110pt.
    return {
      cssWidth: canvas.getBoundingClientRect().width,
      at: {
        topBand: classify(px(width / 2, height * (1 - 250 / 300))),
        bottomBand: classify(px(width / 2, height * (1 - 50 / 300))),
      },
      darkInTextBand: dark(1 - 166 / 300, 1 - 128 / 300),
      darkInEmptyBand: dark(1 - 110 / 300, 1 - 90 / 300),
    };
  });
}

/**
 * The Korean fixture line "한글 문서" at 36pt from x=40pt: each character
 * advances one 36pt em cell (DW 1000). Returns a coarse bitmap signature
 * per cell; fallback "tofu" boxes would make the Hangul cells identical.
 */
async function hangulCells(page: Page): Promise<
  {
    dark: number;
    signature: string;
  }[]
> {
  return page.locator("[data-pdf-viewer] canvas").evaluate((node) => {
    const canvas = node as HTMLCanvasElement;
    const required6 = canvas.getContext("2d");
    if (required6 === null) {
      throw new Error('Missing fixture value: canvas.getContext("2d")');
    }
    const ctx = required6;
    const unit = canvas.width / 400;
    const top = Math.floor(canvas.height * (1 - 168 / 300));
    const bottom = Math.floor(canvas.height * (1 - 122 / 300));
    const cells = [];
    for (let i = 0; i < 5; i += 1) {
      const left = Math.floor((40 + 36 * i) * unit);
      const right = Math.floor((40 + 36 * (i + 1)) * unit);
      const { data, width } = ctx.getImageData(left, top, right - left, bottom - top);
      let dark = 0;
      let signature = "";
      const grid = 8;
      for (let gy = 0; gy < grid; gy += 1) {
        for (let gx = 0; gx < grid; gx += 1) {
          let cellDark = 0;
          for (
            let y = Math.floor((gy * (bottom - top)) / grid);
            y < Math.floor(((gy + 1) * (bottom - top)) / grid);
            y += 1
          ) {
            for (
              let x = Math.floor((gx * width) / grid);
              x < Math.floor(((gx + 1) * width) / grid);
              x += 1
            ) {
              const at = (y * width + x) * 4;
              const required7 = data[at];
              if (required7 === undefined) {
                throw new Error("Missing fixture value: data[at]");
              }
              const required8 = data[at + 1];
              if (required8 === undefined) {
                throw new Error("Missing fixture value: data[at + 1]");
              }
              const required9 = data[at + 2];
              if (required9 === undefined) {
                throw new Error("Missing fixture value: data[at + 2]");
              }
              if (required7 < 90 && required8 < 90 && required9 < 90) {
                cellDark += 1;
              }
            }
          }
          dark += cellDark;
          signature += cellDark > 0 ? "1" : "0";
        }
      }
      cells.push({ dark, signature });
    }
    return cells;
  });
}

test("PDF attachment: page navigation, zoom, rendered content, doc switch, not found", async ({
  page,
  browser,
}) => {
  test.setTimeout(90000);
  const csp = watchCspViolations(page);
  // pdf.js reports missing CMap/standard-font/wasm data as console warnings.
  const pdfjsDataWarnings: string[] = [];
  page.on("console", (message) => {
    if (/cMapUrl|standardFontDataUrl|wasmUrl|iccUrl|Failed to fetch file/.test(message.text())) {
      pdfjsDataWarnings.push(message.text());
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
  const required10 = workspaces.items.find((item) => item.slug === owner.workspaceSlug);
  if (required10 === undefined) {
    throw new Error(
      "Missing fixture value: workspaces.items.find((item) => item.slug === owner.workspaceSlug)",
    );
  }
  const wsId = required10.id;
  const docRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: "PDF 첨부" },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const documentId = (
    (await readJson(docRes, flowSchemas.document)) as {
      id: string;
    }
  ).id;
  const pdfId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "report.pdf",
    buildFixturePdf([
      { text: "FVOCI PAGE ONE", script: "latin", band: "top-red" },
      { text: "FVOCI PAGE TWO", script: "latin", band: "bottom-blue" },
    ]),
  );
  const textName = "notes.txt";
  const textId = await uploadAttachment(
    page,
    wsId,
    documentId,
    textName,
    Buffer.from("plain attachment body\n", "utf8"),
  );

  await page.goto(`/w/acme/a/${pdfId}/view`);
  const viewer = page.locator("[data-pdf-viewer]");
  await expect(viewer).toBeVisible({ timeout: 20000 });
  await expectVueViewer(page);
  // The candidate survives a direct URL refresh with the authenticated cookie.
  await page.reload();
  await expect(viewer.getByText("1 / 2")).toBeVisible();
  await expectVueViewer(page);
  // A direct session viewer URL in an anonymous context returns to login,
  // retaining the full viewer target, including search and fragment.
  const anonymous = await browser.newContext();
  try {
    const signedOut = await anonymous.newPage();
    const target = `/w/acme/a/${pdfId}/view?chunk=0#document`;
    // Deliver the real setup response after /auth/me's real 401 has started
    // login navigation, while that first navigation is still uncommitted.
    // A repeated redirect used to cancel it and reject the URL predicate.
    let releaseSetup!: () => void;
    const setupRelease = new Promise<void>((resolve) => {
      releaseSetup = resolve;
    });
    let setupDelivered!: () => void;
    const setupDelivery = new Promise<void>((resolve) => {
      setupDelivered = resolve;
    });
    let releaseLogin!: () => void;
    const loginRelease = new Promise<void>((resolve) => {
      releaseLogin = resolve;
    });
    await signedOut.route("**/api/v1/setup", async (route) => {
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      await setupRelease;
      await route.fulfill({ response });
      setupDelivered();
    });
    const loginRequests: string[] = [];
    const firstLogin = signedOut.waitForRequest(
      (request) => new URL(request.url()).pathname === "/login",
    );
    await signedOut.route("**/login?returnTo=*", async (route) => {
      loginRequests.push(route.request().url());
      await loginRelease;
      await route.continue().catch(() => undefined);
    });
    // The initial commit is the navigation precondition; the exact login URL
    // and its loaded page are still required by the assertion below.
    await signedOut.goto(target, { waitUntil: "commit" });
    await firstLogin;
    const redirected = expect(signedOut).toHaveURL(
      (url) => url.pathname === "/login" && url.searchParams.get("returnTo") === target,
    );
    // Keep a rejected navigation assertion observed while the browser barrier
    // completes; it is still awaited below and never treated as a success.
    void redirected.catch(() => undefined);
    const setupResponse = signedOut.waitForResponse(
      (response) => new URL(response.url()).pathname === "/api/v1/setup",
    );
    releaseSetup();
    await setupDelivery;
    // Finish delivery of the real setup body before login can commit.
    expect(await (await setupResponse).finished()).toBeNull();
    releaseLogin();
    await redirected;
    expect(loginRequests).toEqual([
      new URL(`/login?returnTo=${encodeURIComponent(target)}`, signedOut.url()).href,
    ]);
    expect(
      (await signedOut.request.get(`/api/v1/workspaces/${wsId}/attachments/${pdfId}`)).status(),
    ).toBe(401);
  } finally {
    await anonymous.close();
  }
  await expect(viewer.getByText("1 / 2")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "이전 쪽" })).toBeDisabled();

  // Page 1: red band on top, no blue band, rendered text line, clean margin.
  await expect.poll(async () => (await probeCanvas(page)).at.topBand).toBe("red");
  const first = await probeCanvas(page);
  expect(first.at.bottomBand).toBe("white");
  expect(first.darkInTextBand).toBeGreaterThan(200);
  expect(first.darkInEmptyBand).toBe(0);
  expect(Math.round(first.cssWidth)).toBe(PAGE_W);

  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await expect(viewer.getByText("2 / 2")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "다음 쪽" })).toBeDisabled();
  await expect.poll(async () => (await probeCanvas(page)).at.bottomBand).toBe("blue");
  const second = await probeCanvas(page);
  expect(second.at.topBand).toBe("white");
  expect(second.darkInTextBand).toBeGreaterThan(200);

  await viewer.getByRole("button", { name: "확대" }).click();
  await expect(viewer.getByText("125%")).toBeVisible();
  await expect.poll(async () => Math.round((await probeCanvas(page)).cssWidth)).toBe(PAGE_W * 1.25);
  await viewer.getByRole("button", { name: "원래 크기" }).click();
  await expect(viewer.getByText("100%")).toBeVisible();
  await expect.poll(async () => Math.round((await probeCanvas(page)).cssWidth)).toBe(PAGE_W);
  await viewer.getByRole("button", { name: "이전 쪽" }).click();
  await expect(viewer.getByText("1 / 2")).toBeVisible();

  // Doc switch while the PDF bytes are still in flight: the late response must
  // never paint the previous file into the text attachment's viewer.
  let releasePdf!: () => void;
  const held = new Promise<void>((resolve) => {
    releasePdf = resolve;
  });
  let pdfRequested!: () => void;
  const requested = new Promise<void>((resolve) => {
    pdfRequested = resolve;
  });
  await page.route(`**/api/v1/workspaces/${wsId}/attachments/${pdfId}/download`, async (route) => {
    pdfRequested();
    await held;
    await route.continue().catch(() => undefined);
  });
  const pdfUrl = `/api/v1/workspaces/${wsId}/attachments/${pdfId}/download`;
  const pdfSettled = new Promise<void>((resolve) => {
    const done = (request: { url: () => string }) => {
      if (request.url().endsWith(pdfUrl)) {
        resolve();
      }
    };
    page.on("requestfinished", done);
    page.on("requestfailed", done);
  });
  await page.goto(`/w/acme/a/${pdfId}/view`);
  await requested;
  await page.evaluate((path) => {
    window.history.pushState({}, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${textId}/view`);
  await expect(page.getByText("plain attachment body")).toBeVisible();
  releasePdf();
  await pdfSettled;
  await expect(
    page.locator("[data-attachment-viewer]").getByText(textName, { exact: true }),
  ).toBeVisible();
  await expect(page.locator("[data-pdf-viewer]")).toHaveCount(0);
  await expect(page.locator("canvas")).toHaveCount(0);
  await page.unroute(`**/api/v1/workspaces/${wsId}/attachments/${pdfId}/download`);

  // Korean text in a non-embedded Adobe-Korea1 font renders through the
  // packed CMaps shipped as same-origin build assets.
  const koreanId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "korean.pdf",
    buildFixturePdf([{ text: "한글 문서", script: "korean", band: "top-red" }]),
  );
  const cmapResponse = page.waitForResponse((response) =>
    response.url().endsWith(`${PDFJS_ASSETS}cmaps/UniKS-UCS2-H.bcmap`),
  );
  await page.goto(`/w/acme/a/${koreanId}/view`);
  expect((await cmapResponse).status()).toBe(200);
  await expect(page.locator("[data-pdf-viewer]")).toBeVisible({ timeout: 20000 });
  await expect.poll(async () => (await probeCanvas(page)).at.topBand).toBe("red");
  const korean = await probeCanvas(page);
  expect(korean.darkInTextBand).toBeGreaterThan(200);
  expect(korean.darkInEmptyBand).toBe(0);
  // 한, 글, (space), 문, 서: four inked, mutually distinct glyphs and an empty space cell.
  const cells = await hangulCells(page);
  const required11 = cells[0];
  if (required11 === undefined) {
    throw new Error("Missing fixture value: cells[0]");
  }
  const required12 = cells[1];
  if (required12 === undefined) {
    throw new Error("Missing fixture value: cells[1]");
  }
  const required13 = cells[3];
  if (required13 === undefined) {
    throw new Error("Missing fixture value: cells[3]");
  }
  const required14 = cells[4];
  if (required14 === undefined) {
    throw new Error("Missing fixture value: cells[4]");
  }
  const glyphs = [required11, required12, required13, required14];
  for (const glyph of glyphs) expect(glyph.dark).toBeGreaterThan(40);
  const required15 = cells[2];
  if (required15 === undefined) {
    throw new Error("Missing fixture value: cells[2]");
  }
  expect(required15.dark).toBe(0);
  expect(new Set(glyphs.map((glyph) => glyph.signature)).size).toBe(4);
  const koreanPng = await page.locator("[data-pdf-viewer] canvas").screenshot();
  await test.info().attach("korean-pdf-canvas", { body: koreanPng, contentType: "image/png" });
  // Opt-in durable copy for review evidence; runner output is removed on success.
  const evidenceDir = process.env.FVOCI_PDF_EVIDENCE_DIR;
  if (evidenceDir) {
    fs.mkdirSync(evidenceDir, { recursive: true });
    fs.writeFileSync(path.join(evidenceDir, "korean-pdf-canvas.png"), koreanPng);
    fs.writeFileSync(
      path.join(evidenceDir, "korean-pdf-cells.json"),
      `${JSON.stringify(cells, null, 2)}\n`,
    );
  }

  // Production asset URLs, types and the scripting exclusion.
  for (const [rel, type] of [
    ["cmaps/UniKS-UCS2-H.bcmap", "application/octet-stream"],
    ["cmaps/LICENSE", null],
    ["standard_fonts/LiberationSans-Regular.ttf", null],
    ["standard_fonts/LICENSE_FOXIT", null],
    ["wasm/openjpeg.wasm", "application/wasm"],
    ["wasm/openjpeg_nowasm_fallback.js", "javascript"],
    ["iccs/CGATS001Compat-v2-micro.icc", null],
  ] as const) {
    const res = await page.request.get(`${PDFJS_ASSETS}${rel}`);
    expect(res.status(), rel).toBe(200);
    if (type) {
      expect(res.headers()["content-type"], rel).toContain(type);
    }
    expect(res.headers()["x-content-type-options"], rel).toBe("nosniff");
  }
  expect((await page.request.get(`${PDFJS_ASSETS}wasm/quickjs-eval.wasm`)).status()).toBe(404);

  // Unknown (or unauthorized) attachment: visible error, no viewer body.
  await page.goto(`/w/acme/a/00000000-0000-4000-8000-000000000000/view`);
  await expect(page.getByText("접근 권한이 없거나 존재하지 않는 항목입니다.")).toBeVisible();
  await expect(page.locator("[data-pdf-viewer]")).toHaveCount(0);

  expect(csp).toEqual([]);
  expect(pdfjsDataWarnings).toEqual([]);
});
