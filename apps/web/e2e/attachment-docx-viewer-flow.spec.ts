import { expectVueViewer } from "./viewer-app";
import fs from "node:fs";
import path from "node:path";
import { crc32 as zlibCrc32, deflateRawSync } from "node:zlib";
import { expect, test, type Page } from "@playwright/test";
import {
  buildFixtureDocx,
  DEFAULT_DOCX_TEXT,
  FIXTURE_DOCX_PAGE_W,
  writeZip,
} from "../src/features/attachments/docx-test-fixture";
import { watchCspViolations } from "./helpers";

const owner = {
  email: "docx-viewer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "문서",
  workspaceSlug: "acme",
  workspaceName: "DOCX Viewer",
};

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

type LayoutProbe = {
  sandbox: string | null;
  csp: string | null;
  scripts: number;
  hrefs: number;
  handlers: number;
  nonDataUrls: string[];
  pages: number;
  visible: number[];
  visibleText: string;
  sectionWidth: number;
  boldWeight: number;
  link: { href: string | null; color: string; decoration: string } | null;
  listIndent: number[];
  listMarkers: string[];
  cells: number;
  redCell: string;
  cellBorder: string;
  images: { src: string; width: number; height: number; natural: number }[];
  styleElements: number;
  boxSizing: string;
};

async function probeLayout(page: Page, text: typeof DEFAULT_DOCX_TEXT): Promise<LayoutProbe> {
  return page.locator("[data-docx-viewer] iframe").evaluate((node, text) => {
    const frame = node as HTMLIFrameElement;
    const doc = frame.contentDocument!;
    const win = frame.contentWindow!;
    const sections = [...doc.querySelectorAll<HTMLElement>(".docx-wrapper > section.docx")];
    const visible = sections.flatMap((s, i) =>
      win.getComputedStyle(s).display === "none" ? [] : [i],
    );
    const shown = sections[visible[0] ?? 0]!;
    const byText = (value: string) =>
      [...doc.querySelectorAll<HTMLElement>("span, a, p, td")].find(
        (el) => el.textContent?.trim() === value.trim(),
      );
    const link = [...doc.querySelectorAll("a")].find((a) => a.textContent?.includes(text.link));
    // Paragraph content-box start: OOXML w:ind/@w:left as laid out (marker width excluded).
    const textLeft = (value: string) => {
      const p = [...doc.querySelectorAll("p")].find((el) => el.textContent?.includes(value))!;
      const css = win.getComputedStyle(p);
      return (
        p.getBoundingClientRect().left +
        parseFloat(css.borderLeftWidth) +
        parseFloat(css.paddingLeft)
      );
    };
    const listParagraph = (value: string) =>
      [...doc.querySelectorAll("p")].find((el) => el.textContent?.includes(value))!;
    const firstCell = doc.querySelector("td")!;
    const nonDataUrls: string[] = [];
    for (const el of doc.querySelectorAll("*")) {
      for (const attr of ["src", "href", "xlink:href"]) {
        const value = el.getAttribute(attr);
        if (value !== null && !value.startsWith("data:image/"))
          nonDataUrls.push(`${el.localName}[${attr}]=${value}`);
      }
    }
    return {
      sandbox: frame.getAttribute("sandbox"),
      csp:
        doc.querySelector('meta[http-equiv="Content-Security-Policy"]')?.getAttribute("content") ??
        null,
      scripts: doc.querySelectorAll("script, iframe, object, embed, form, base").length,
      hrefs: doc.querySelectorAll("a[href]").length,
      handlers: [...doc.querySelectorAll("*")].filter((el) =>
        [...el.attributes].some((a) => a.name.startsWith("on")),
      ).length,
      nonDataUrls,
      pages: sections.length,
      visible,
      visibleText: shown.textContent ?? "",
      sectionWidth: shown.getBoundingClientRect().width,
      boldWeight: Number(win.getComputedStyle(byText(text.bold)!).fontWeight),
      link: link
        ? {
            href: link.getAttribute("href"),
            color: win.getComputedStyle(link.querySelector("span") ?? link).color,
            decoration: win.getComputedStyle(link.querySelector("span") ?? link).textDecorationLine,
          }
        : null,
      listIndent: text.list.map(textLeft),
      listMarkers: text.list.map(
        (value) => win.getComputedStyle(listParagraph(value), "::before").content,
      ),
      cells: doc.querySelectorAll("table td").length,
      redCell: win.getComputedStyle(firstCell).backgroundColor,
      cellBorder: `${win.getComputedStyle(firstCell).borderTopStyle} ${win.getComputedStyle(firstCell).borderTopWidth}`,
      images: [...doc.querySelectorAll("img")].map((img) => ({
        src: img.getAttribute("src")?.slice(0, 22) ?? "",
        width: img.getBoundingClientRect().width,
        height: img.getBoundingClientRect().height,
        natural: img.naturalWidth,
      })),
      styleElements: doc.querySelectorAll("style").length,
      boxSizing: win.getComputedStyle(shown).boxSizing,
    };
  }, text);
}

function evidence(name: string, body: Buffer | string): void {
  // Opt-in durable copy for review evidence; runner output is removed on success.
  const dir = process.env.FVOCI_DOCX_EVIDENCE_DIR;
  if (!dir) return;
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, name), body);
}

test("DOCX attachment: layout, isolation, pages, zoom, original bytes, chunk supplement, limits", async ({
  page,
}) => {
  test.setTimeout(150_000);
  const csp = watchCspViolations(page);
  const baseOrigin = new URL(test.info().project.use.baseURL ?? "http://127.0.0.1:5173").origin;
  const foreign: string[] = [];
  const nullRequests: string[] = [];
  page.on("request", (request) => {
    const url = request.url();
    if (url.startsWith("data:") || url.startsWith("blob:") || url === "about:srcdoc") return;
    if (new URL(url).origin !== baseOrigin) foreign.push(url);
    if (/\/null(?:[?#]|$)/.test(url)) nullRequests.push(url);
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

  const workspaces = (await (await page.request.get("/api/v1/me/workspaces")).json()) as {
    items: { id: string; slug: string }[];
  };
  const wsId = workspaces.items.find((item) => item.slug === owner.workspaceSlug)!.id;
  const docRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: "DOCX 첨부" },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const documentId = ((await docRes.json()) as { id: string }).id;

  const token = `docx${Date.now()}`;
  const text = { ...DEFAULT_DOCX_TEXT, heading: `${DEFAULT_DOCX_TEXT.heading} ${token}` };
  const docxBytes = buildFixtureDocx(text);
  const docxName = `${token}-layout.docx`;
  const docxId = await uploadAttachment(page, wsId, documentId, docxName, docxBytes);
  const textName = "notes.txt";
  const textId = await uploadAttachment(
    page,
    wsId,
    documentId,
    textName,
    Buffer.from("plain attachment body\n", "utf8"),
  );
  const downloadPath = `/api/v1/workspaces/${wsId}/attachments/${docxId}/download`;

  // --- Layout of page 1 -------------------------------------------------------
  await page.goto(`/w/acme/a/${docxId}/view`);
  await expectVueViewer(page);
  const viewer = page.locator('[data-docx-viewer][data-docx-state="ready"]');
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  await expect(page.locator("[data-chunk-supplement]")).toHaveCount(0);
  await expect(viewer.getByText("1 / 2")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "이전 쪽" })).toBeDisabled();

  const first = await probeLayout(page, text);
  const frame = page.locator("[data-docx-viewer] iframe");
  const page1Png = await frame.screenshot();
  await test.info().attach("docx-page-1", { body: page1Png, contentType: "image/png" });
  evidence("docx-page-1.png", page1Png);
  evidence("docx-page-1-probe.json", `${JSON.stringify(first, null, 2)}\n`);
  evidence("csp-at-page-1.json", `${JSON.stringify(csp, null, 2)}\n`);
  expect(first.sandbox).toBe("allow-same-origin");
  expect(first.csp).toContain("default-src 'none'");
  expect(first.csp).toContain("script-src 'none'");
  expect(first.scripts).toBe(0);
  expect(first.hrefs).toBe(0);
  expect(first.handlers).toBe(0);
  expect(first.nonDataUrls).toEqual([]);
  expect(first.pages).toBe(2);
  expect(first.visible).toEqual([0]);
  for (const part of [
    text.heading,
    text.body.trim(),
    text.bold,
    text.link,
    ...text.list,
    ...text.table,
  ]) {
    expect(first.visibleText).toContain(part);
  }
  expect(first.visibleText).not.toContain(text.secondPage);
  expect(Math.round(first.sectionWidth)).toBe(FIXTURE_DOCX_PAGE_W);
  expect(first.boldWeight).toBeGreaterThanOrEqual(700);
  expect(first.link).toEqual({ href: null, color: "rgb(5, 99, 193)", decoration: "underline" });
  // 720 twips per list level = 0.5 in = 48 CSS px; the second top-level item is back at level 0.
  expect(Math.round(first.listIndent[1]! - first.listIndent[0]!)).toBe(48);
  expect(Math.round(first.listIndent[2]! - first.listIndent[0]!)).toBe(0);
  for (const marker of first.listMarkers) expect(marker).toContain("counter(");
  expect(first.cells).toBe(4);
  expect(first.redCell).toBe("rgb(255, 0, 0)");
  expect(first.cellBorder).toMatch(/^solid [1-9]/);
  // The embedded PNG loads at its 96×48 px extent; the linked external image never loads.
  expect(first.images.filter((img) => img.src.startsWith("data:image/png"))).toEqual([
    { src: "data:image/png;base64,", width: 96, height: 48, natural: 4 },
  ]);

  // Clicking a (stripped) link navigates nowhere.
  const before = page.url();
  await page.frameLocator("[data-docx-viewer] iframe").getByText(text.link).click();
  await page.frameLocator("[data-docx-viewer] iframe").getByText(text.scriptLink).click();
  expect(page.url()).toBe(before);
  expect(page.context().pages()).toHaveLength(1);

  // --- Page 2 and zoom -------------------------------------------------------
  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await expect(viewer.getByText("2 / 2")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "다음 쪽" })).toBeDisabled();
  const second = await probeLayout(page, text);
  expect(second.visible).toEqual([1]);
  expect(second.visibleText).toContain(text.secondPage);
  expect(second.visibleText).not.toContain(text.heading);
  const page2Png = await frame.screenshot();
  evidence("docx-page-2.png", page2Png);

  const frameBox = async () => (await frame.boundingBox())!;
  const height100 = (await frameBox()).height;
  await viewer.getByRole("button", { name: "확대" }).click();
  await expect(viewer.getByText("125%")).toBeVisible();
  await expect
    .poll(async () => Math.round((await frameBox()).height))
    .toBeGreaterThan(Math.round(height100 * 1.2));
  await viewer.getByRole("button", { name: "원래 크기" }).click();
  await expect(viewer.getByText("100%")).toBeVisible();
  await expect.poll(async () => Math.round((await frameBox()).height)).toBe(Math.round(height100));
  await viewer.getByRole("button", { name: "이전 쪽" }).click();
  await expect(viewer.getByText("1 / 2")).toBeVisible();

  // --- Original bytes download ---------------------------------------------
  const download = page.locator("[data-attachment-viewer] header a[download]");
  await expect(download).toHaveAttribute("href", downloadPath);
  const original = await page.request.get(downloadPath);
  expect(original.status()).toBe(200);
  expect(Buffer.compare(await original.body(), Buffer.from(docxBytes))).toBe(0);

  // --- Search hit → layout + chunk supplement (session preview-html) --------
  await expect
    .poll(
      async () => {
        const res = await page.request.get(
          `/api/v1/workspaces/${wsId}/search?q=${encodeURIComponent(token)}&type=attachment`,
        );
        const body = (await res.json()) as { items?: { type: string; title: string }[] };
        return (body.items ?? []).some(
          (item) => item.type === "attachment" && item.title === docxName,
        );
      },
      { timeout: 30_000 },
    )
    .toBe(true);
  await page.goto(`/w/acme/search?q=${encodeURIComponent(token)}`);
  const results = page.getByRole("region", { name: "검색" });
  await results.getByRole("link", { name: new RegExp(docxName) }).click();
  await expect(page).toHaveURL(new RegExp(`/w/acme/a/${docxId}/view(?:\\?chunk=\\d+)?$`));
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  await expectVueViewer(page);

  await page.goto(`/w/acme/a/${docxId}/view?chunk=0`);
  const supplement = page.locator("[data-chunk-supplement]");
  await expect(supplement.getByText("레이아웃 없음")).toBeVisible();
  await expect(supplement.locator("mark")).toContainText(token, { timeout: 20_000 });
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  const chunkPng = await page.locator("[data-attachment-viewer]").screenshot();
  evidence("docx-chunk-supplement.png", chunkPng);

  // An unavailable or failing supplement never hides the layout.
  const previewPath = `**/api/v1/workspaces/${wsId}/attachments/${docxId}/preview-html`;
  for (const [status, message] of [
    [413, "이 파일을 뷰어로 열 수 없습니다. 원본을 다운로드하세요."],
    [500, "불러오지 못했습니다."],
  ] as const) {
    await page.route(previewPath, (route) =>
      route.fulfill({ status, contentType: "application/problem+json", body: "{}" }),
    );
    await page.goto(`/w/acme/a/${docxId}/view?chunk=0`);
    await expect(supplement.getByText(message)).toBeVisible();
    await expect(viewer).toBeVisible({ timeout: 20_000 });
    await page.unroute(previewPath);
  }

  // --- Download failure: visible error with retry, no stale frame ----------
  const downloadRoute = `**${downloadPath}`;
  await page.route(downloadRoute, (route) => route.fulfill({ status: 403, body: "" }));
  await page.goto(`/w/acme/a/${docxId}/view`);
  await expect(
    page.locator('[data-docx-viewer][data-docx-state="error"]').getByRole("alert"),
  ).toHaveText("불러오지 못했습니다.");
  await expect(page.locator("[data-docx-viewer] iframe")).toHaveCount(0);
  await page.unroute(downloadRoute);
  await page.getByRole("button", { name: "다시 시도" }).click();
  await expect(viewer).toBeVisible({ timeout: 20_000 });

  // --- Switch files while the DOCX bytes are in flight ---------------------
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let requested!: () => void;
  const inFlight = new Promise<void>((resolve) => {
    requested = resolve;
  });
  await page.route(downloadRoute, async (route) => {
    requested();
    await held;
    await route.continue().catch(() => undefined);
  });
  const settled = new Promise<void>((resolve) => {
    const done = (request: { url: () => string }) => {
      if (request.url().endsWith(downloadPath)) resolve();
    };
    page.on("requestfinished", done);
    page.on("requestfailed", done);
  });
  await page.goto(`/w/acme/a/${docxId}/view`);
  await inFlight;
  await page.evaluate((to) => {
    window.history.pushState({}, "", to);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${textId}/view`);
  await expect(page.getByText("plain attachment body")).toBeVisible();
  release();
  await settled;
  await expect(
    page.locator("[data-attachment-viewer]").getByText(textName, { exact: true }),
  ).toBeVisible();
  await expect(page.locator("[data-docx-viewer]")).toHaveCount(0);
  await expect(page.locator("iframe")).toHaveCount(0);
  await page.unroute(downloadRoute);

  // --- Inflation limit: a ~260 KB package that inflates to 260 MiB ---------
  const inflated = Buffer.alloc(260 * 1024 * 1024);
  const bombId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "bomb.docx",
    writeZip([
      { name: "[Content_Types].xml", bytes: new TextEncoder().encode("<Types/>") },
      {
        name: "word/document.xml",
        deflated: deflateRawSync(inflated),
        crc: zlibCrc32(inflated),
        size: inflated.byteLength,
      },
    ]),
  );
  await page.goto(`/w/acme/a/${bombId}/view`);
  await expect(
    page.locator('[data-docx-viewer][data-docx-state="error"]').getByRole("alert"),
  ).toHaveText("이 파일을 뷰어로 열 수 없습니다. 원본을 다운로드하세요.", { timeout: 30_000 });
  await expect(page.locator("[data-docx-viewer] iframe")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "다시 시도" })).toHaveCount(0);

  // --- Other Office kinds still have no layout viewer here -----------------
  const pptxId = await uploadAttachment(page, wsId, documentId, "deck.pptx", docxBytes);
  await page.goto(`/w/acme/a/${pptxId}/view`);
  await expect(
    page.getByText("이 파일을 뷰어로 열 수 없습니다. 원본을 다운로드하세요."),
  ).toBeVisible();
  await expect(page.locator("[data-docx-viewer]")).toHaveCount(0);

  // --- Unknown (or unauthorized) attachment --------------------------------
  await page.goto(`/w/acme/a/00000000-0000-4000-8000-000000000000/view`);
  await expect(page.getByText("접근 권한이 없거나 존재하지 않는 항목입니다.")).toBeVisible();
  await expect(page.locator("[data-docx-viewer]")).toHaveCount(0);

  expect(csp).toEqual([]);
  expect(foreign).toEqual([]);
  expect(nullRequests).toEqual([]);
});
