import { expect, test, type Page } from "@playwright/test";
import { watchCspViolations } from "./helpers";

const owner = {
  email: "pdf-viewer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "뷰어",
  workspaceSlug: "acme",
  workspaceName: "PDF Viewer",
};

/** Page geometry in PDF points; the canvas at 100% is this many CSS px. */
const PAGE_W = 400;
const PAGE_H = 300;

/**
 * Minimal synthetic PDF: one Helvetica (standard 14, not embedded) text line
 * per page plus a colour band that marks which page is on screen — red across
 * the top of page 1, blue across the bottom of page 2.
 */
function buildPdf(pages: { text: string; band: "top-red" | "bottom-blue" }[]): Buffer {
  const objects: string[] = [];
  const add = (body: string) => {
    objects.push(body);
    return objects.length;
  };
  const catalog = add("");
  const pagesId = add("");
  const font = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  const kids: number[] = [];
  for (const page of pages) {
    const band =
      page.band === "top-red"
        ? `1 0 0 rg 20 220 ${PAGE_W - 40} 60 re f`
        : `0 0 1 rg 20 20 ${PAGE_W - 40} 60 re f`;
    const stream = `${band}\n0 0 0 rg BT /F1 36 Tf 40 130 Td (${page.text}) Tj ET\n`;
    const content = add(`<< /Length ${Buffer.byteLength(stream, "latin1")} >>\nstream\n${stream}endstream`);
    kids.push(
      add(
        `<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [0 0 ${PAGE_W} ${PAGE_H}] ` +
          `/Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${content} 0 R >>`,
      ),
    );
  }
  objects[catalog - 1] = `<< /Type /Catalog /Pages ${pagesId} 0 R >>`;
  objects[pagesId - 1] = `<< /Type /Pages /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] /Count ${kids.length} >>`;

  let out = "%PDF-1.4\n";
  const offsets: number[] = [];
  objects.forEach((body, index) => {
    offsets.push(Buffer.byteLength(out, "latin1"));
    out += `${index + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = Buffer.byteLength(out, "latin1");
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) out += `${String(offset).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root ${catalog} 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(out, "latin1");
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
  const upload = (await uploadRes.json()) as {
    attachmentId: string;
    partSizeBytes: number;
    parts: Array<{ partNumber: number; url: string }>;
  };
  const parts: { partNumber: number; etag: string }[] = [];
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
    parts.push({ partNumber: part.partNumber, etag: etag! });
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
    const ctx = canvas.getContext("2d")!;
    const { width, height } = canvas;
    const data = ctx.getImageData(0, 0, width, height).data;
    const px = (x: number, y: number) => {
      const i = (Math.floor(y) * width + Math.floor(x)) * 4;
      return [data[i]!, data[i + 1]!, data[i + 2]!] as const;
    };
    const classify = ([r, g, b]: readonly [number, number, number]) => {
      if (r > 200 && g < 60 && b < 60) return "red" as const;
      if (b > 200 && r < 60 && g < 60) return "blue" as const;
      if (r > 230 && g > 230 && b > 230) return "white" as const;
      if (r < 90 && g < 90 && b < 90) return "dark" as const;
      return "other" as const;
    };
    const dark = (top: number, bottom: number) => {
      let count = 0;
      for (let y = Math.floor(top * height); y < Math.floor(bottom * height); y += 1) {
        for (let x = 0; x < width; x += 1) {
          if (classify(px(x, y)) === "dark") count += 1;
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

test("PDF attachment: page navigation, zoom, rendered content, doc switch, not found", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const csp = watchCspViolations(page);

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
    data: { parentId: null, title: "PDF 첨부" },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const documentId = ((await docRes.json()) as { id: string }).id;

  const pdfId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "report.pdf",
    buildPdf([
      { text: "FVOCI PAGE ONE", band: "top-red" },
      { text: "FVOCI PAGE TWO", band: "bottom-blue" },
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
  await expect(viewer).toBeVisible({ timeout: 20_000 });
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
      if (request.url().endsWith(pdfUrl)) resolve();
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
  await expect(page.locator("[data-attachment-viewer]").getByText(textName, { exact: true })).toBeVisible();
  await expect(page.locator("[data-pdf-viewer]")).toHaveCount(0);
  await expect(page.locator("canvas")).toHaveCount(0);
  await page.unroute(`**/api/v1/workspaces/${wsId}/attachments/${pdfId}/download`);

  // Unknown (or unauthorized) attachment: visible error, no viewer body.
  await page.goto(`/w/acme/a/00000000-0000-4000-8000-000000000000/view`);
  await expect(page.getByText("접근 권한이 없거나 존재하지 않는 항목입니다.")).toBeVisible();
  await expect(page.locator("[data-pdf-viewer]")).toHaveCount(0);

  expect(csp).toEqual([]);
});
