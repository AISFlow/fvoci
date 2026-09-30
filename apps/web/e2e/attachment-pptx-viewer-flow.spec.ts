import { expectVueViewer } from "./viewer-app";
import fs from "node:fs";
import path from "node:path";
import { crc32 as zlibCrc32, deflateRawSync } from "node:zlib";
import { expect, test, type Page, type Route } from "@playwright/test";
import { unzipSync } from "fflate";
import { writeZip } from "../src/features/attachments/docx-test-fixture";
import {
  buildChartPptx,
  HOSTILE_PPTX_MARKUP,
} from "../src/features/attachments/pptx-hostile-fixture";
import { PPTX_MAX_MARKUP_BYTES } from "../src/features/attachments/pptx-limits";
import { innerSlideSvg } from "../src/features/attachments/pptx-svg";
import {
  buildFixturePptx,
  DEFAULT_PPTX_TEXT,
  FIXTURE_PPTX_COLORS,
  FIXTURE_PPTX_FILLER,
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
/** Thrown inside an idle PPTX worker by the spec; the page may report it as an uncaught worker error. */
const IDLE_WORKER_DEATH = "pptx idle worker death (spec)";

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
  /** The served blob, verbatim. */
  outer: string;
  /** The renderer SVG inside it, or null when the blob is not exactly the fixed template. */
  inner: string | null;
  text: string;
  images: number;
  foreignObjects: number;
};

/**
 * What the served slide (the Blob behind the visible `<img>`) contains. It
 * must be exactly the fixed outer template (`innerSlideSvg`); the renderer SVG
 * inside is decoded and read here in Node: an in-page DOM parse would itself
 * raise CSP `style-src` reports for the slide's style attributes.
 */
async function probeSvg(page: Page): Promise<SvgProbe> {
  const img = page.locator("[data-pptx-viewer] img.pptx-viewer__slide");
  const src = (await img.getAttribute("src"))!;
  const outer = await page.evaluate(async (url) => {
    const store = (window as unknown as { __svgBlobs: { created: Map<string, Blob> } }).__svgBlobs;
    return store.created.get(url)!.text();
  }, src);
  const inner = innerSlideSvg(outer);
  const markup = inner ?? "";
  // Text for assertions only: tags dropped and the renderer's entities undone.
  const text = markup
    .replace(/<[^<>]*>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&amp;/g, "&");
  return {
    src,
    outer,
    inner,
    text,
    images: (markup.match(/<image[\s/>]/g) ?? []).length,
    foreignObjects: (markup.match(/<foreignObject[\s/>]/g) ?? []).length,
  };
}

/** The dedicated PPTX workers (Vite names the built chunk after `pptx-worker.ts`). */
function pptxWorkers(page: Page) {
  return page
    .workers()
    .filter((worker) => /\/assets\/pptx-worker-[^/]+\.js$/.test(new URL(worker.url()).pathname));
}

type Rgb = [number, number, number];

/**
 * The fixture's embedded picture (480, 400, 96 × 48), inset 2 px from the
 * edges its upscale blends with the background.
 */
const BLUE_PICTURE_INTERIOR = { x: 482, y: 402, width: 92, height: 44 } as const;

/**
 * The linked picture's placeholder box (640, 400, 48 × 48) inset 2 px, and
 * the bands beside it on the rows of its label, up to the blue picture and to
 * x 900. The renderer centres a ~450 px label on the box; bounded, it paints
 * inside the box only.
 */
const PLACEHOLDER = {
  inside: { x: 642, y: 402, width: 44, height: 44 },
  beside: [
    { x: 578, y: 402, width: 60, height: 44 },
    { x: 690, y: 402, width: 210, height: 44 },
  ],
} as const;

/**
 * Colours of the rendered slide at slide-space points, from a screenshot of
 * the `<img>` (a canvas would be tainted by the SVG's foreignObject text),
 * and how many slide px of the blue picture's interior are not its colour:
 * one point can fall between the glyphs of text drawn over the picture; and
 * the linked placeholder's label ink inside its box and paint beside it.
 */
async function sampleSlide(page: Page, points: Record<string, [number, number]>) {
  const img = page.locator("[data-pptx-viewer] img.pptx-viewer__slide");
  const png = await img.screenshot();
  return page.evaluate(
    async ({ data, points, slideWidth, box, blue, placeholderBox }) => {
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
      let offBlue = 0;
      for (let y = box.y; y < box.y + box.height; y += 1) {
        for (let x = box.x; x < box.x + box.width; x += 1) {
          const rgb = ctx.getImageData(
            Math.round((x + 0.5) * px),
            Math.round((y + 0.5) * px),
            1,
            1,
          ).data;
          if (!blue.every((c, k) => Math.abs(rgb[k]! - c) <= 24)) offBlue += 1;
        }
      }
      const count = (
        r: { x: number; y: number; width: number; height: number },
        hit: (rgb: Uint8ClampedArray) => boolean,
      ) => {
        let n = 0;
        for (let y = r.y; y < r.y + r.height; y += 1) {
          for (let x = r.x; x < r.x + r.width; x += 1) {
            if (
              hit(
                ctx.getImageData(Math.round((x + 0.5) * px), Math.round((y + 0.5) * px), 1, 1).data,
              )
            )
              n += 1;
          }
        }
        return n;
      };
      const dark = (rgb: Uint8ClampedArray) => rgb[0]! + rgb[1]! + rgb[2]! < 384;
      const notWhite = (rgb: Uint8ClampedArray) => rgb[0]! < 250 || rgb[1]! < 250 || rgb[2]! < 250;
      const placeholder = {
        ink: count(placeholderBox.inside, dark),
        beside: placeholderBox.beside.reduce((n, r) => n + count(r, notWhite), 0),
      };
      // Dark pixels in the title box: glyphs were drawn (whatever font the host has).
      const title = ctx.getImageData(
        Math.round(40 * px),
        Math.round(24 * px),
        Math.round(880 * px),
        Math.round(60 * px),
      ).data;
      let ink = 0;
      for (let i = 0; i < title.length; i += 4)
        if (title[i]! + title[i + 1]! + title[i + 2]! < 240) ink += 1;
      return { colors: out, titleInk: ink, offBlue, placeholder };
    },
    {
      data: png.toString("base64"),
      points,
      slideWidth: FIXTURE_PPTX_SLIDE_W,
      box: BLUE_PICTURE_INTERIOR,
      blue: FIXTURE_PPTX_COLORS.blue,
      placeholderBox: PLACEHOLDER,
    },
  );
}

/**
 * The hostile chart deck (1280 × 720 slide px) paints two columns in the
 * default series fill (Office accent 1): x 112–206 and 350–444, bottoms at
 * y 288. Measured in the 8f4b82aa screenshots: 31,933 pixels within 24 of it.
 */
const HOSTILE_CHART = {
  width: 1280,
  bar: [68, 114, 196],
  inBars: [
    [159, 240],
    [397, 120],
  ],
  background: [
    [280, 120],
    [800, 500],
  ],
} as const;

/**
 * Proof that the inner chart SVG was parsed and painted, from a screenshot of
 * exactly the slide: bar and background samples, and the bar-coloured area in
 * slide px². Decoded in `page` (the app document), not the page shown.
 */
async function chartPaint(page: Page, png: Buffer) {
  return page.evaluate(
    async ({ data, chart }) => {
      const image = new Image();
      image.src = `data:image/png;base64,${data}`;
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const ctx = canvas.getContext("2d")!;
      ctx.drawImage(image, 0, 0);
      const px = canvas.width / chart.width;
      const at = ([x, y]: readonly [number, number]) => {
        const [r, g, b] = ctx.getImageData(Math.round(x * px), Math.round(y * px), 1, 1).data;
        return [r!, g!, b!] as [number, number, number];
      };
      const all = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
      let bar = 0;
      for (let i = 0; i < all.length; i += 4) {
        if (chart.bar.every((c, k) => Math.abs(all[i + k]! - c) <= 24)) bar += 1;
      }
      return {
        inBars: chart.inBars.map(at),
        background: chart.background.map(at),
        barArea: bar / (px * px),
      };
    },
    { data: png.toString("base64"), chart: HOSTILE_CHART },
  );
}

async function expectChartPainted(page: Page, png: Buffer) {
  const paint = await chartPaint(page, png);
  for (const rgb of paint.inBars) expect(near(rgb, HOSTILE_CHART.bar), `bar ${rgb}`).toBe(true);
  for (const rgb of paint.background)
    expect(near(rgb, [255, 255, 255]), `background ${rgb}`).toBe(true);
  expect(paint.barArea).toBeGreaterThan(25_000);
  expect(paint.barArea).toBeLessThan(40_000);
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

/** The same package with every part deflated, so a large synthetic deck uploads small. */
function deflateAll(bytes: Uint8Array): Buffer {
  const parts = unzipSync(bytes);
  return Buffer.from(
    writeZip(
      Object.entries(parts).map(([name, data]) => ({
        name,
        deflated: deflateRawSync(data),
        crc: zlibCrc32(data),
        size: data.byteLength,
      })),
    ),
  );
}

test("PPTX attachment: slide layout, image-wrapped SVG, slides, zoom, original bytes, chunk supplement, failures, limits, hostile markup, worker bounds, share", async ({
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
  const meta = (await (
    await page.request.get(`/api/v1/workspaces/${wsId}/attachments/${pptxId}`)
  ).json()) as {
    mime?: string;
  };
  const textName = "notes.txt";
  const textId = await uploadAttachment(
    page,
    wsId,
    documentId,
    textName,
    Buffer.from("plain attachment body\n", "utf8"),
  );
  const downloadPath = `/api/v1/workspaces/${wsId}/attachments/${pptxId}/download`;

  // --- Slide 1 layout ---------------------------------------------------------
  await page.goto(`/w/acme/a/${pptxId}/view`);
  await expectVueViewer(page);
  const viewer = page.locator('[data-pptx-viewer][data-pptx-slide-state="ready"]');
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  await expect(page.locator("[data-chunk-supplement]")).toHaveCount(0);
  await expect(viewer.getByText("슬라이드 1 / 2")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "이전 슬라이드" })).toBeDisabled();
  const img = viewer.locator("img.pptx-viewer__slide");
  await expect(img).toHaveAttribute("src", /^blob:/);
  await expect(img).toHaveAttribute("alt", "슬라이드 1 / 2");
  await expect
    .poll(() => img.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0))
    .toBe(true);
  const box1 = (await img.boundingBox())!;
  expect(Math.round(box1.width)).toBe(FIXTURE_PPTX_SLIDE_W);
  expect(Math.round(box1.height)).toBe(FIXTURE_PPTX_SLIDE_H);

  const first = await probeSvg(page);
  evidence("pptx-slide-1-probe.json", `${JSON.stringify({ ...first, meta }, null, 2)}\n`);
  for (const part of [
    text.title,
    text.body.trim(),
    text.bold,
    text.link,
    text.scriptLink,
    ...text.list,
    ...text.table,
  ]) {
    expect(first.text).toContain(part);
  }
  expect(first.text).not.toContain(text.secondSlide);
  // The blob is only the fixed outer template; the renderer SVG (links included) is an image inside it.
  expect(first.inner).not.toBeNull();
  expect(first.outer).toMatch(
    /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg" width="960" height="540" viewBox="0 0 960 540"><image /,
  );
  expect(first.inner).toContain(`href="${FIXTURE_PPTX_EXTERNAL_LINK}"`);
  expect(first.images).toBe(1);
  expect(first.foreignObjects).toBeGreaterThan(0);

  const samples = await sampleSlide(page, {
    red: [620, 292],
    green: [120, 420],
    orange: [320, 420],
    blue: [528, 424],
    white: [900, 500],
    placeholder: [646, 406],
  });
  const slide1Png = await img.screenshot();
  await test.info().attach("pptx-slide-1", { body: slide1Png, contentType: "image/png" });
  evidence("pptx-slide-1.png", slide1Png);
  evidence("pptx-slide-1-samples.json", `${JSON.stringify(samples, null, 2)}\n`);
  expect(near(samples.colors.red!, FIXTURE_PPTX_COLORS.red)).toBe(true);
  expect(near(samples.colors.green!, FIXTURE_PPTX_COLORS.green)).toBe(true);
  expect(near(samples.colors.orange!, FIXTURE_PPTX_COLORS.orange)).toBe(true);
  expect(near(samples.colors.blue!, FIXTURE_PPTX_COLORS.blue)).toBe(true);
  expect(samples.offBlue).toBe(0);
  // The linked picture stays a visible placeholder (its fill, its label cut to the box), and its
  // label paints nothing beside the box.
  expect(near(samples.colors.placeholder!, [0xf3, 0xf4, 0xf6], 4)).toBe(true);
  expect(samples.placeholder.ink).toBeGreaterThan(20);
  expect(samples.placeholder.beside).toBe(0);
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
  expect(second.inner).not.toBeNull();
  expect(second.text).toContain(text.secondSlide);
  expect(second.text).not.toContain(text.title);
  const revoked = await page.evaluate(
    () => (window as unknown as { __svgBlobs: { revoked: string[] } }).__svgBlobs.revoked,
  );
  expect(revoked).toContain(first.src);
  evidence("pptx-slide-2.png", await img.screenshot());

  await viewer.getByRole("button", { name: "확대" }).click();
  await expect(viewer.getByText("125%")).toBeVisible();
  await expect
    .poll(async () => Math.round((await img.boundingBox())!.width))
    .toBe(FIXTURE_PPTX_SLIDE_W * 1.25);
  for (let i = 0; i < 3; i += 1) await viewer.getByRole("button", { name: "축소" }).click();
  await expect(viewer.getByText("50%")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "축소" })).toBeDisabled();
  await expect
    .poll(async () => Math.round((await img.boundingBox())!.width))
    .toBe(FIXTURE_PPTX_SLIDE_W / 2);
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
        return (body.items ?? []).some(
          (item) => item.type === "attachment" && item.title === pptxName,
        );
      },
      { timeout: 30_000 },
    )
    .toBe(true);
  await page.goto(`/w/acme/search?q=${encodeURIComponent(token)}`);
  await page
    .getByRole("region", { name: "검색" })
    .getByRole("link", { name: new RegExp(pptxName) })
    .click();
  await expect(page).toHaveURL(new RegExp(`/w/acme/a/${pptxId}/view(?:\\?chunk=\\d+)?$`));
  await expect(viewer).toBeVisible({ timeout: 20_000 });

  await page.goto(`/w/acme/a/${pptxId}/view?chunk=0`);
  const supplement = page.locator("[data-chunk-supplement]");
  await expect(supplement.getByText("레이아웃 없음")).toBeVisible();
  await expect(supplement.locator("mark")).toContainText(token, { timeout: 20_000 });
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  evidence(
    "pptx-chunk-supplement.png",
    await page.locator("[data-attachment-viewer]").screenshot(),
  );

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

  // --- The worker dies while idle: the next slide change fails visibly -------
  // No render is waiting to hear of it, and reopening on its own could repeat without end.
  await expect.poll(() => pptxWorkers(page).length).toBe(1);
  const idleWorker = pptxWorkers(page)[0]!;
  await idleWorker.evaluate((message) => {
    setTimeout(() => {
      throw new Error(message);
    }, 0);
  }, IDLE_WORKER_DEATH);
  await expect.poll(() => pptxWorkers(page).includes(idleWorker)).toBe(false);
  await viewer.getByRole("button", { name: "다음 슬라이드" }).click();
  await expect(page.locator("[data-attachment-viewer] [role=alert]")).toHaveText(loadFailed, {
    timeout: 2_000,
  });
  await expect(page.locator("[data-pptx-viewer]")).toHaveCount(0);
  expect(pptxWorkers(page)).toHaveLength(0);
  // Retry downloads and opens again, in exactly one new worker.
  await page.getByRole("button", { name: "다시 시도" }).click();
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  await expect(viewer.getByText("슬라이드 1 / 2")).toBeVisible();
  await expect.poll(() => pptxWorkers(page).length).toBe(1);

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
  await expect(
    page.locator("[data-attachment-viewer]").getByText(textName, { exact: true }),
  ).toBeVisible();
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
      {
        name: "ppt/presentation.xml",
        deflated: deflateRawSync(inflated),
        crc: zlibCrc32(inflated),
        size: 16,
      },
    ]),
  );
  await page.goto(`/w/acme/a/${bombId}/view`);
  await expect(page.locator("[data-attachment-viewer] [role=alert]")).toHaveText(unavailable, {
    timeout: 30_000,
  });
  await expect(page.locator("[data-pptx-viewer]")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "다시 시도" })).toHaveCount(0);

  // --- Not a deck; and legacy PPT stays download-only -------------------------
  const brokenId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "broken.pptx",
    Buffer.from("not a zip", "utf8"),
  );
  await page.goto(`/w/acme/a/${brokenId}/view`);
  await expect(page.locator("[data-attachment-viewer] [role=alert]")).toHaveText(unavailable, {
    timeout: 20_000,
  });
  // The server sniffs MIME from content, so the legacy file carries an OLE compound-file header.
  const ole = Buffer.alloc(4096);
  Buffer.from([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]).copy(ole);
  const pptId = await uploadAttachment(page, wsId, documentId, "old.ppt", ole);
  await page.goto(`/w/acme/a/${pptId}/view`);
  await expect(page.getByText(unavailable)).toBeVisible();
  await expect(page.locator("[data-pptx-viewer]")).toHaveCount(0);

  // --- Markup budget: a small deck whose slide XML inflates past 16 MiB -------
  const fillerBytes = new TextEncoder().encode(FIXTURE_PPTX_FILLER).byteLength;
  const bigSlide = buildFixturePptx(DEFAULT_PPTX_TEXT, {
    slide2Paragraphs: Math.ceil(PPTX_MAX_MARKUP_BYTES / fillerBytes),
  });
  const markupId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "markup.pptx",
    deflateAll(bigSlide),
  );
  await page.goto(`/w/acme/a/${markupId}/view`);
  await expect(page.locator("[data-attachment-viewer] [role=alert]")).toHaveText(unavailable, {
    timeout: 30_000,
  });
  await expect(page.getByRole("button", { name: "다시 시도" })).toHaveCount(0);
  await expect.poll(() => pptxWorkers(page).length).toBe(0);

  // --- Hostile chart markup: inert in the page and in the standalone blob ------
  const pwned: string[] = [];
  page.context().on("request", (request) => {
    if (/pwned/.test(request.url())) pwned.push(request.url());
  });
  const hostileId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "hostile.pptx",
    await buildChartPptx(Object.values(HOSTILE_PPTX_MARKUP).join("")),
  );
  await page.goto(`/w/acme/a/${hostileId}/view`);
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  const hostile = await probeSvg(page);
  // The renderer really emitted the deck's markup (negative control), but only inside the image.
  expect(hostile.inner).not.toBeNull();
  for (const markup of Object.values(HOSTILE_PPTX_MARKUP)) expect(hostile.inner).toContain(markup);
  expect(hostile.outer).not.toMatch(/script|iframe|meta|javascript/i);
  await expect
    .poll(() => img.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0))
    .toBe(true);
  const hostileShot = await img.screenshot();
  evidence("pptx-hostile-chart.png", hostileShot);
  // Positive control: the inner chart really painted, so the negatives below are not vacuous.
  await expectChartPainted(page, hostileShot);
  await img.click({ position: { x: 40, y: 40 } });
  // "Open image in new tab": the blob as its own document, on the app origin.
  const standalone = await page.context().newPage();
  const standaloneCsp = watchCspViolations(standalone);
  const standaloneErrors: string[] = [];
  standalone.on("pageerror", (error) => standaloneErrors.push(error.message));
  await standalone.goto(hostile.src);
  await standalone.waitForTimeout(1_000);
  expect(standalone.url()).toBe(hostile.src);
  expect(standalone.frames()).toHaveLength(1);
  expect(
    await standalone.evaluate(() => (window as unknown as { __pptxPwned?: number }).__pptxPwned),
  ).toBeUndefined();
  expect(
    await standalone.evaluate(() => document.documentElement.outerHTML.length),
  ).toBeGreaterThan(0);
  expect(
    await standalone.evaluate(() => document.querySelectorAll("script, iframe, a, meta").length),
  ).toBe(0);
  const standaloneShot = await standalone.screenshot({
    clip: { x: 0, y: 0, width: HOSTILE_CHART.width, height: 720 },
  });
  evidence("pptx-hostile-standalone.png", standaloneShot);
  await expectChartPainted(page, standaloneShot);
  await standalone.mouse.click(40, 40);
  await standalone.waitForTimeout(300);
  expect(standalone.url()).toBe(hostile.src);
  expect(standaloneErrors).toEqual([]);
  expect(standaloneCsp).toEqual([]);
  await standalone.close();
  expect(
    await page.evaluate(() => (window as unknown as { __pptxPwned?: number }).__pptxPwned),
  ).toBeUndefined();
  expect(page.url()).toContain(`/a/${hostileId}/view`);
  expect(pwned).toEqual([]);

  // --- Slow layout: leaving a slide mid-layout, the render bound, unmount -----
  // Slide 2's text box has ~1 MiB of paragraphs: its layout runs far past the 10 s bound.
  const slowId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "slow.pptx",
    deflateAll(buildFixturePptx(DEFAULT_PPTX_TEXT, { slide2Paragraphs: 11_000 })),
  );
  await page.goto(`/w/acme/a/${slowId}/view`);
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  const pane = page.locator("[data-pptx-viewer]");
  await expect.poll(() => pptxWorkers(page).length).toBe(1);
  const slowFirstWorker = pptxWorkers(page)[0]!;
  await pane.getByRole("button", { name: "다음 슬라이드" }).click();
  await expect(pane).toHaveAttribute("data-pptx-slide-state", "loading");
  // The page stays responsive while the worker lays out slide 2.
  const tick = await page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const started = performance.now();
        setTimeout(() => resolve(performance.now() - started), 0);
      }),
  );
  expect(tick).toBeLessThan(500);
  // Back to slide 1 mid-layout: that worker is terminated and a new one shows slide 1.
  await pane.getByRole("button", { name: "이전 슬라이드" }).click();
  await expect(
    page.locator('[data-pptx-viewer][data-pptx-slide="0"][data-pptx-slide-state="ready"]'),
  ).toBeVisible({
    timeout: 20_000,
  });
  await expect.poll(() => pptxWorkers(page).includes(slowFirstWorker)).toBe(false);
  await expect.poll(() => pptxWorkers(page).length).toBe(1);
  // Left alone, slide 2 hits the render bound: unavailable, and slide 1 still works.
  const boundStarted = Date.now();
  await pane.getByRole("button", { name: "다음 슬라이드" }).click();
  await expect(
    page.locator('[data-pptx-viewer][data-pptx-slide="1"][data-pptx-slide-state="unavailable"]'),
  ).toBeVisible({
    timeout: 30_000,
  });
  expect(Date.now() - boundStarted).toBeGreaterThanOrEqual(9_000);
  await expect(pane.getByRole("alert")).toHaveText(unavailable);
  await pane.getByRole("button", { name: "이전 슬라이드" }).click();
  await expect(
    page.locator('[data-pptx-viewer][data-pptx-slide="0"][data-pptx-slide-state="ready"]'),
  ).toBeVisible({
    timeout: 20_000,
  });
  // Not laid out again: slide 2 is unavailable at once.
  await pane.getByRole("button", { name: "다음 슬라이드" }).click();
  await expect(
    page.locator('[data-pptx-viewer][data-pptx-slide="1"][data-pptx-slide-state="unavailable"]'),
  ).toBeVisible({
    timeout: 2_000,
  });
  // Unmount mid-layout (switch attachments): no PPTX worker is left running.
  await page.goto(`/w/acme/a/${slowId}/view`);
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  await pane.getByRole("button", { name: "다음 슬라이드" }).click();
  await expect(pane).toHaveAttribute("data-pptx-slide-state", "loading");
  await page.evaluate((to) => {
    window.history.pushState({}, "", to);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${textId}/view`);
  await expect(page.getByText("plain attachment body")).toBeVisible();
  await expect.poll(() => pptxWorkers(page).length, { timeout: 3_000 }).toBe(0);

  // --- The worker bundle's packages are in the public notice -----------------
  const notice = await (await page.request.get("/open-source-licenses.txt")).text();
  for (const name of ["@office-kit/pptx", "@office-kit/pptx-preview", "fflate"]) {
    expect(notice).toContain(name);
  }

  expect(csp).toEqual([]);
  expect(foreign).toEqual([]);
  expect(
    foreign.filter((url) => url.startsWith(new URL(FIXTURE_PPTX_EXTERNAL_LINK).origin)),
  ).toEqual([]);
  expect(
    foreign.filter((url) => url.startsWith(new URL(FIXTURE_PPTX_EXTERNAL_IMAGE).origin)),
  ).toEqual([]);
  expect(pageErrors.filter((message) => !message.includes(IDLE_WORKER_DEATH))).toEqual([]);

  // --- Share: same viewer over share bytes; no session preview/edit calls -----
  const shareRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/share-links`,
    {
      data: { expiresInDays: 7 },
    },
  );
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
  await expect(reader.locator("[data-attachment-viewer] header a[download]")).toHaveAttribute(
    "href",
    shareDownload,
  );
  const sharedSvg = await probeSvg(reader);
  expect(sharedSvg.inner).not.toBeNull();
  expect(sharedSvg.text).toContain(text.title);

  const shareHold = holdRoute();
  await reader.route(`**${shareDownload}`, shareHold.handler);
  await reader.reload();
  await shareHold.requested;
  const revoke = await page.request.delete(`/api/v1/workspaces/${wsId}/share-links/${share.id}`);
  expect(revoke.ok(), await revoke.text()).toBeTruthy();
  shareHold.release();
  await expect(reader.locator("[data-attachment-viewer] [role=alert]")).toHaveText(loadFailed, {
    timeout: 20_000,
  });
  await expect(reader.locator("img.pptx-viewer__slide")).toHaveCount(0);
  await reader.unroute(`**${shareDownload}`);
  expect((await reader.request.get(shareDownload)).status()).toBe(404);
  for (const pathname of readerApi) {
    expect(pathname.startsWith("/api/v1/share/")).toBe(true);
  }
  expect(readerCsp).toEqual([]);
  await anon.close();
});
