import fs from "node:fs";
import path from "node:path";
import { crc32 as zlibCrc32, deflateRawSync } from "node:zlib";
import { expect, test, type Page, type Route } from "@playwright/test";
import { writeZip } from "../src/features/attachments/docx-test-fixture";
import {
  buildFixturePptx,
  DEFAULT_PPTX_TEXT,
  FIXTURE_PPTX_COLORS,
  FIXTURE_PPTX_EXTERNAL_IMAGE,
  FIXTURE_PPTX_EXTERNAL_LINK,
  FIXTURE_PPTX_SLIDE_H,
  FIXTURE_PPTX_SLIDE_W,
} from "../src/features/attachments/pptx-test-fixture";
import { watchCspViolations } from "./helpers";

const owner = {
  email: "pptx-viewer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "발표",
  workspaceSlug: "acme",
  workspaceName: "PPTX Viewer",
};

const unavailable = "이 파일을 뷰어로 열 수 없습니다. 원본을 다운로드하세요.";
const loadFailed = "불러오지 못했습니다.";

async function uploadAttachment(
  page: Page,
  wsId: string,
  documentId: string,
  name: string,
  bytes: Uint8Array,
): Promise<string> {
  const uploadRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/uploads`,
    { data: { name, sizeBytes: bytes.length } },
  );
  expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
  const upload = (await uploadRes.json()) as {
    attachmentId: string;
    partSizeBytes: number;
    parts: Array<{ partNumber: number; url: string }>;
  };
  const parts: { partNumber: number; etag: string }[] = [];
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
    parts.push({ partNumber: part.partNumber, etag: etag! });
  }
  const completeRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`,
    { data: { parts } },
  );
  expect(completeRes.ok(), await completeRes.text()).toBeTruthy();
  return upload.attachmentId;
}

/** Holds matching requests until released; resolves `requested` on the first one. */
function holdRoute() {
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let markRequested!: () => void;
  const requested = new Promise<void>((resolve) => {
    markRequested = resolve;
  });
  return {
    requested,
    release,
    handler: async (route: Route) => {
      markRequested();
      await held;
      await route.continue().catch(() => undefined);
    },
  };
}

/**
 * Keeps every SVG Blob the page turns into an object URL, and every revoked
 * URL. The app CSP (`connect-src 'self'`) forbids `fetch(blob:)`, so the
 * served slide markup is read back from the Blob itself.
 */
async function recordSvgBlobs(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const created = new Map<string, Blob>();
    const revoked: string[] = [];
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (object: Blob | MediaSource) => {
      const url = create(object);
      if (object instanceof Blob && object.type === "image/svg+xml") created.set(url, object);
      return url;
    };
    URL.revokeObjectURL = (url: string) => {
      revoked.push(url);
      revoke(url);
    };
    (window as unknown as { __svgBlobs: unknown }).__svgBlobs = { created, revoked };
  });
}

type SvgProbe = {
  src: string;
  text: string;
  anchors: number;
  denied: number;
  handlers: number;
  refs: string[];
  cssUrls: string[];
  images: number;
  foreignObjects: number;
};

/**
 * What the served slide SVG (the Blob behind the visible `<img>`) contains.
 * The markup is analysed here in Node: an in-page DOM parse would itself
 * raise CSP `style-src` reports for the slide's style attributes.
 */
async function probeSvg(page: Page): Promise<SvgProbe> {
  const img = page.locator("[data-pptx-viewer] img.pptx-viewer__slide");
  const src = (await img.getAttribute("src"))!;
  const markup = await page.evaluate(async (url) => {
    const store = (window as unknown as { __svgBlobs: { created: Map<string, Blob> } }).__svgBlobs;
    return store.created.get(url)!.text();
  }, src);
  // The renderer escapes & < > " in text and values, so tags are exactly <[^<>]*>.
  const tags = markup.match(/<[^<>]*>/g) ?? [];
  const named = (name: string) => tags.filter((tag) => new RegExp(`^<${name}[\\s/>]`, "i").test(tag)).length;
  const refs: string[] = [];
  const cssUrls: string[] = [];
  for (const tag of tags) {
    for (const match of tag.matchAll(/\s((?:[a-z]+:)?href|src)="([^"]*)"/gi)) {
      if (!match[2]!.startsWith("#") && !match[2]!.startsWith("data:image/")) refs.push(`${match[1]}=${match[2]!.slice(0, 60)}`);
    }
    for (const match of tag.matchAll(/url\(\s*["']?([^"')]*)/gi)) {
      if (!match[1]!.startsWith("#") && !match[1]!.startsWith("data:")) cssUrls.push(match[1]!);
    }
  }
  const text = markup
    .replace(/<[^<>]*>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&amp;/g, "&");
  return {
    src,
    text,
    anchors: named("a"),
    denied: ["script", "iframe", "object", "embed", "form", "animate", "set", "style"].reduce((n, name) => n + named(name), 0),
    handlers: tags.filter((tag) => /\son[a-z]+\s*=/i.test(tag.replace(/"[^"]*"/g, '""'))).length,
    refs,
    cssUrls,
    images: named("image"),
    foreignObjects: named("foreignObject"),
  };
}

type Rgb = [number, number, number];

/**
 * Colours of the rendered slide at slide-space points, from a screenshot of
 * the `<img>` (a canvas would be tainted by the SVG's foreignObject text).
 */
async function sampleSlide(page: Page, points: Record<string, [number, number]>) {
  const img = page.locator("[data-pptx-viewer] img.pptx-viewer__slide");
  const png = await img.screenshot();
  return page.evaluate(
    async ({ data, points, slideWidth }) => {
      const image = new Image();
      image.src = `data:image/png;base64,${data}`;
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const ctx = canvas.getContext("2d")!;
      ctx.drawImage(image, 0, 0);
      // Screenshot pixels per slide-space px (device pixel ratio × zoom).
      const px = canvas.width / slideWidth;
      const out: Record<string, [number, number, number]> = {};
      for (const [name, [x, y]] of Object.entries(points)) {
        const [r, g, b] = ctx.getImageData(Math.round(x * px), Math.round(y * px), 1, 1).data;
        out[name] = [r!, g!, b!];
      }
      // Dark pixels in the title box: glyphs were drawn (whatever font the host has).
      const title = ctx.getImageData(Math.round(40 * px), Math.round(24 * px), Math.round(880 * px), Math.round(60 * px)).data;
      let ink = 0;
      for (let i = 0; i < title.length; i += 4) if (title[i]! + title[i + 1]! + title[i + 2]! < 240) ink += 1;
      return { colors: out, titleInk: ink };
    },
    { data: png.toString("base64"), points, slideWidth: FIXTURE_PPTX_SLIDE_W },
  );
}

function near(actual: Rgb, expected: readonly number[], tolerance = 24) {
  return actual.every((c, i) => Math.abs(c - expected[i]!) <= tolerance);
}

function evidence(name: string, body: Buffer | string): void {
  // Opt-in durable copy for review evidence; runner output is removed on success.
  const dir = process.env.FVOCI_PPTX_EVIDENCE_DIR;
  if (!dir) return;
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, name), body);
}

test("PPTX attachment: slide layout, inert SVG, slides, zoom, original bytes, chunk supplement, failures, limits, share", async ({
  page,
  browser,
}) => {
  test.setTimeout(180_000);
  await recordSvgBlobs(page);
  const csp = watchCspViolations(page);
  const baseOrigin = new URL(test.info().project.use.baseURL ?? "http://127.0.0.1:5173").origin;
  const foreign: string[] = [];
  page.on("request", (request) => {
    const url = request.url();
    if (url.startsWith("data:") || url.startsWith("blob:")) return;
    if (new URL(url).origin !== baseOrigin) foreign.push(url);
  });
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

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

  const workspaces = (await (await page.request.get("/api/v1/me/workspaces")).json()) as {
    items: { id: string; slug: string }[];
  };
  const wsId = workspaces.items.find((item) => item.slug === owner.workspaceSlug)!.id;
  const docRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: "PPTX 첨부" },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const documentId = ((await docRes.json()) as { id: string }).id;

  const token = `pptx${Date.now()}`;
  const text = { ...DEFAULT_PPTX_TEXT, title: `${DEFAULT_PPTX_TEXT.title} ${token}` };
  const pptxBytes = buildFixturePptx(text);
  const pptxName = `${token}-deck.pptx`;
  const pptxId = await uploadAttachment(page, wsId, documentId, pptxName, pptxBytes);
  const meta = (await (await page.request.get(`/api/v1/workspaces/${wsId}/attachments/${pptxId}`)).json()) as {
    mime?: string;
  };
  const textName = "notes.txt";
  const textId = await uploadAttachment(page, wsId, documentId, textName, Buffer.from("plain attachment body\n", "utf8"));
  const downloadPath = `/api/v1/workspaces/${wsId}/attachments/${pptxId}/download`;

  // --- Slide 1 layout ---------------------------------------------------------
  await page.goto(`/w/acme/a/${pptxId}/view`);
  const viewer = page.locator('[data-pptx-viewer][data-pptx-slide-state="ready"]');
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  await expect(page.locator("[data-chunk-supplement]")).toHaveCount(0);
  await expect(viewer.getByText("슬라이드 1 / 2")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "이전 슬라이드" })).toBeDisabled();
  const img = viewer.locator("img.pptx-viewer__slide");
  await expect(img).toHaveAttribute("src", /^blob:/);
  await expect(img).toHaveAttribute("alt", "슬라이드 1 / 2");
  await expect.poll(() => img.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0)).toBe(true);
  const box1 = (await img.boundingBox())!;
  expect(Math.round(box1.width)).toBe(FIXTURE_PPTX_SLIDE_W);
  expect(Math.round(box1.height)).toBe(FIXTURE_PPTX_SLIDE_H);

  const first = await probeSvg(page);
  evidence("pptx-slide-1-probe.json", `${JSON.stringify({ ...first, meta }, null, 2)}\n`);
  for (const part of [text.title, text.body.trim(), text.bold, text.link, text.scriptLink, ...text.list, ...text.table]) {
    expect(first.text).toContain(part);
  }
  expect(first.text).not.toContain(text.secondSlide);
  expect(first.anchors).toBe(0);
  expect(first.denied).toBe(0);
  expect(first.handlers).toBe(0);
  expect(first.refs).toEqual([]);
  expect(first.cssUrls).toEqual([]);
  expect(first.images).toBe(1);
  expect(first.foreignObjects).toBeGreaterThan(0);

  const samples = await sampleSlide(page, {
    red: [620, 292],
    green: [120, 420],
    orange: [320, 420],
    blue: [528, 424],
    white: [900, 500],
  });
  const slide1Png = await img.screenshot();
  await test.info().attach("pptx-slide-1", { body: slide1Png, contentType: "image/png" });
  evidence("pptx-slide-1.png", slide1Png);
  evidence("pptx-slide-1-samples.json", `${JSON.stringify(samples, null, 2)}\n`);
  expect(near(samples.colors.red!, FIXTURE_PPTX_COLORS.red)).toBe(true);
  expect(near(samples.colors.green!, FIXTURE_PPTX_COLORS.green)).toBe(true);
  expect(near(samples.colors.orange!, FIXTURE_PPTX_COLORS.orange)).toBe(true);
  expect(near(samples.colors.blue!, FIXTURE_PPTX_COLORS.blue)).toBe(true);
  expect(near(samples.colors.white!, [255, 255, 255])).toBe(true);
  expect(samples.titleInk).toBeGreaterThan(200);

  // Clicking where the links are laid out navigates nowhere and opens nothing.
  const before = page.url();
  await img.click({ position: { x: 470, y: 115 } });
  await img.click({ position: { x: 640, y: 115 } });
  await page.waitForTimeout(300);
  expect(page.url()).toBe(before);
  expect(page.context().pages()).toHaveLength(1);

  // --- Slide 2, zoom, blob URL revocation ------------------------------------
  await viewer.getByRole("button", { name: "다음 슬라이드" }).click();
  await expect(viewer.getByText("슬라이드 2 / 2")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "다음 슬라이드" })).toBeDisabled();
  await expect(img).not.toHaveAttribute("src", first.src);
  const second = await probeSvg(page);
  expect(second.text).toContain(text.secondSlide);
  expect(second.text).not.toContain(text.title);
  const revoked = await page.evaluate(
    () => (window as unknown as { __svgBlobs: { revoked: string[] } }).__svgBlobs.revoked,
  );
  expect(revoked).toContain(first.src);
  evidence("pptx-slide-2.png", await img.screenshot());

  await viewer.getByRole("button", { name: "확대" }).click();
  await expect(viewer.getByText("125%")).toBeVisible();
  await expect.poll(async () => Math.round((await img.boundingBox())!.width)).toBe(FIXTURE_PPTX_SLIDE_W * 1.25);
  for (let i = 0; i < 3; i += 1) await viewer.getByRole("button", { name: "축소" }).click();
  await expect(viewer.getByText("50%")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "축소" })).toBeDisabled();
  await expect.poll(async () => Math.round((await img.boundingBox())!.width)).toBe(FIXTURE_PPTX_SLIDE_W / 2);
  await viewer.getByRole("button", { name: "원래 크기" }).click();
  await expect(viewer.getByText("100%")).toBeVisible();
  await viewer.getByRole("button", { name: "이전 슬라이드" }).click();
  await expect(viewer.getByText("슬라이드 1 / 2")).toBeVisible();

  // --- Original bytes download ----------------------------------------------
  const download = page.locator("[data-attachment-viewer] header a[download]");
  await expect(download).toHaveAttribute("href", downloadPath);
  const original = await page.request.get(downloadPath);
  expect(original.status()).toBe(200);
  expect(Buffer.compare(await original.body(), Buffer.from(pptxBytes))).toBe(0);

  // --- Search hit → layout + chunk supplement (session preview-html) ---------
  await expect
    .poll(
      async () => {
        const res = await page.request.get(
          `/api/v1/workspaces/${wsId}/search?q=${encodeURIComponent(token)}&type=attachment`,
        );
        const body = (await res.json()) as { items?: { type: string; title: string }[] };
        return (body.items ?? []).some((item) => item.type === "attachment" && item.title === pptxName);
      },
      { timeout: 30_000 },
    )
    .toBe(true);
  await page.goto(`/w/acme/search?q=${encodeURIComponent(token)}`);
  await page.getByRole("region", { name: "검색" }).getByRole("link", { name: new RegExp(pptxName) }).click();
  await expect(page).toHaveURL(new RegExp(`/w/acme/a/${pptxId}/view(?:\\?chunk=\\d+)?$`));
  await expect(viewer).toBeVisible({ timeout: 20_000 });

  await page.goto(`/w/acme/a/${pptxId}/view?chunk=0`);
  const supplement = page.locator("[data-chunk-supplement]");
  await expect(supplement.getByText("레이아웃 없음")).toBeVisible();
  await expect(supplement.locator("mark")).toContainText(token, { timeout: 20_000 });
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  evidence("pptx-chunk-supplement.png", await page.locator("[data-attachment-viewer]").screenshot());

  // An unavailable or failing supplement never hides the layout.
  const previewPath = `**/api/v1/workspaces/${wsId}/attachments/${pptxId}/preview-html`;
  for (const [status, message] of [
    [413, unavailable],
    [500, loadFailed],
  ] as const) {
    await page.route(previewPath, (route) =>
      route.fulfill({ status, contentType: "application/problem+json", body: "{}" }),
    );
    await page.goto(`/w/acme/a/${pptxId}/view?chunk=0`);
    await expect(supplement.getByText(message)).toBeVisible();
    await expect(viewer).toBeVisible({ timeout: 20_000 });
    await page.unroute(previewPath);
  }

  // --- 403 on the download: visible error, retry reaches the current URL -----
  const downloadRoute = `**${downloadPath}`;
  await page.route(downloadRoute, (route) => route.fulfill({ status: 403, body: "" }));
  await page.goto(`/w/acme/a/${pptxId}/view`);
  await expect(page.locator("[data-attachment-viewer] [role=alert]")).toHaveText(loadFailed);
  await expect(page.locator("[data-pptx-viewer]")).toHaveCount(0);
  await page.unroute(downloadRoute);
  await page.getByRole("button", { name: "다시 시도" }).click();
  await expect(viewer).toBeVisible({ timeout: 20_000 });

  // --- Switch files while the PPTX bytes are in flight ------------------------
  const hold = holdRoute();
  await page.route(downloadRoute, hold.handler);
  const settled = new Promise<void>((resolve) => {
    const done = (request: { url: () => string }) => {
      if (request.url().endsWith(downloadPath)) resolve();
    };
    page.on("requestfinished", done);
    page.on("requestfailed", done);
  });
  await page.goto(`/w/acme/a/${pptxId}/view`);
  await hold.requested;
  await page.evaluate((to) => {
    window.history.pushState({}, "", to);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${textId}/view`);
  await expect(page.getByText("plain attachment body")).toBeVisible();
  hold.release();
  await settled;
  await expect(page.locator("[data-attachment-viewer]").getByText(textName, { exact: true })).toBeVisible();
  await expect(page.locator("[data-pptx-viewer]")).toHaveCount(0);
  await expect(page.locator("img.pptx-viewer__slide")).toHaveCount(0);
  await page.unroute(downloadRoute);

  // --- Inflation limit: a small package that inflates past 128 MiB -----------
  const inflated = Buffer.alloc(160 * 1024 * 1024);
  const bombId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "bomb.pptx",
    writeZip([
      { name: "[Content_Types].xml", bytes: new TextEncoder().encode("<Types/>") },
      { name: "ppt/presentation.xml", deflated: deflateRawSync(inflated), crc: zlibCrc32(inflated), size: 16 },
    ]),
  );
  await page.goto(`/w/acme/a/${bombId}/view`);
  await expect(page.locator("[data-attachment-viewer] [role=alert]")).toHaveText(unavailable, { timeout: 30_000 });
  await expect(page.locator("[data-pptx-viewer]")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "다시 시도" })).toHaveCount(0);

  // --- Not a deck; and legacy PPT stays download-only -------------------------
  const brokenId = await uploadAttachment(page, wsId, documentId, "broken.pptx", Buffer.from("not a zip", "utf8"));
  await page.goto(`/w/acme/a/${brokenId}/view`);
  await expect(page.locator("[data-attachment-viewer] [role=alert]")).toHaveText(unavailable, { timeout: 20_000 });
  // The server sniffs MIME from content, so the legacy file carries an OLE compound-file header.
  const ole = Buffer.alloc(4096);
  Buffer.from([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]).copy(ole);
  const pptId = await uploadAttachment(page, wsId, documentId, "old.ppt", ole);
  await page.goto(`/w/acme/a/${pptId}/view`);
  await expect(page.getByText(unavailable)).toBeVisible();
  await expect(page.locator("[data-pptx-viewer]")).toHaveCount(0);

  expect(csp).toEqual([]);
  expect(foreign).toEqual([]);
  expect(foreign.filter((url) => url.startsWith(new URL(FIXTURE_PPTX_EXTERNAL_LINK).origin))).toEqual([]);
  expect(foreign.filter((url) => url.startsWith(new URL(FIXTURE_PPTX_EXTERNAL_IMAGE).origin))).toEqual([]);
  expect(pageErrors).toEqual([]);

  // --- Share: same viewer over share bytes; no session preview/edit calls -----
  const shareRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${documentId}/share-links`, {
    data: { expiresInDays: 7 },
  });
  expect(shareRes.status(), await shareRes.text()).toBe(201);
  const share = (await shareRes.json()) as { id: string; url: string };
  const sharePath = new URL(share.url).pathname;
  const shareToken = sharePath.split("/")[2]!;
  const shareDownload = `/api/v1/share/${shareToken}/attachments/${pptxId}/download`;
  const anon = await browser.newContext();
  const reader = await anon.newPage();
  await recordSvgBlobs(reader);
  const readerCsp = watchCspViolations(reader);
  const readerApi: string[] = [];
  reader.on("request", (request) => {
    const pathname = new URL(request.url()).pathname;
    if (pathname.startsWith("/api/")) readerApi.push(pathname);
  });
  await reader.goto(`${sharePath}/attachments/${pptxId}/view?chunk=0`);
  const shared = reader.locator('[data-pptx-viewer][data-pptx-slide-state="ready"]');
  await expect(shared).toBeVisible({ timeout: 20_000 });
  await expect(shared.getByText("슬라이드 1 / 2")).toBeVisible();
  await expect(reader.locator("[data-chunk-supplement]")).toHaveCount(0);
  await expect(reader.locator("[data-attachment-viewer] header a[download]")).toHaveAttribute("href", shareDownload);
  const sharedSvg = await probeSvg(reader);
  expect(sharedSvg.text).toContain(text.title);
  expect(sharedSvg.anchors).toBe(0);
  expect(sharedSvg.refs).toEqual([]);

  const shareHold = holdRoute();
  await reader.route(`**${shareDownload}`, shareHold.handler);
  await reader.reload();
  await shareHold.requested;
  const revoke = await page.request.delete(`/api/v1/workspaces/${wsId}/share-links/${share.id}`);
  expect(revoke.ok(), await revoke.text()).toBeTruthy();
  shareHold.release();
  await expect(reader.locator("[data-attachment-viewer] [role=alert]")).toHaveText(loadFailed, { timeout: 20_000 });
  await expect(reader.locator("img.pptx-viewer__slide")).toHaveCount(0);
  await reader.unroute(`**${shareDownload}`);
  expect((await reader.request.get(shareDownload)).status()).toBe(404);
  for (const pathname of readerApi) {
    expect(pathname.startsWith("/api/v1/share/")).toBe(true);
  }
  expect(readerCsp).toEqual([]);
  await anon.close();
});
