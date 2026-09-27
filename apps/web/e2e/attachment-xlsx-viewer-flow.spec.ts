import { expect, test, type Page, type Route } from "@playwright/test";
import {
  buildFixtureXlsx,
  FIXTURE_EXTERNAL_WORKBOOK,
  gridSheet,
} from "../src/features/attachments/xlsx-test-fixture";
import { watchCspViolations } from "./helpers";

const owner = {
  email: "xlsx-viewer@example.com",
  password: "supersecret1",
  familyName: "김",
  givenName: "시트",
  workspaceSlug: "acme",
  workspaceName: "XLSX Viewer",
};

const unavailable = "이 파일을 뷰어로 열 수 없습니다. 원본을 다운로드하세요.";
const loadFailed = "불러오지 못했습니다.";

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

function cellTexts(page: Page) {
  return page.getByTestId("xlsx-viewer").locator("table tr").evaluateAll((rows) =>
    rows.map((row) => [...row.querySelectorAll("td")].map((cell) => cell.textContent ?? "")),
  );
}

test("XLSX attachment: sheets, paging, zoom, cached values, bounds, failures, stale URL and share revocation", async ({
  page,
  browser,
}) => {
  test.setTimeout(120_000);
  const csp = watchCspViolations(page);
  const requestUrls: string[] = [];
  page.on("request", (request) => requestUrls.push(request.url()));

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
    data: { parentId: null, title: "XLSX 첨부" },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const documentId = ((await docRes.json()) as { id: string }).id;
  const downloadUrl = (id: string) => `/api/v1/workspaces/${wsId}/attachments/${id}/download`;

  const bookBytes = await buildFixtureXlsx(
    [
      {
        name: "요약 📊",
        cells: [
          { ref: "A1", shared: "한글 셀 😀" },
          { ref: "B1", inline: "여러 줄\n둘째 줄" },
          { ref: "C1", number: 3.5 },
          { ref: "A2", formula: "SUM(C1,1)", cached: 4.5 },
          { ref: "B2", formula: "[1]Remote!A1", cached: "외부 캐시값" },
          { ref: "C2", formula: "NOW()", cached: "저장된 값" },
          { ref: "A3", inline: "병합" },
        ],
        merges: ["A3:C3"],
      },
      { name: "Chart", chart: true },
      gridSheet("Grid", 201, 65),
    ],
    { vba: true, externalLink: true, deflate: true },
  );
  const bookId = await uploadAttachment(page, wsId, documentId, "재무 보고.xlsx", bookBytes);
  const twoSheetsId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "two-sheets.xlsx",
    await buildFixtureXlsx([
      { name: "첫 시트", cells: [{ ref: "A1", inline: "FIRST SHEET" }] },
      { name: "Second", cells: [{ ref: "A1", inline: "SECOND SHEET" }] },
    ]),
  );

  // Sheet 1: text cells, cached formula values (never recalculated), merge anchor.
  await page.goto(`/w/acme/a/${bookId}/view`);
  const viewer = page.getByTestId("xlsx-viewer");
  await expect(viewer).toBeVisible({ timeout: 20_000 });
  await expect(viewer.getByText("시트 선택: 요약 📊 (1/3)")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "이전 시트" })).toBeDisabled();
  expect(await cellTexts(page)).toEqual([
    ["한글 셀 😀", "여러 줄\n둘째 줄", "3.5"],
    ["4.5", "외부 캐시값", "저장된 값"],
    ["병합", "", ""],
  ]);
  await expect(viewer.locator("[data-xlsx-row-page]")).toHaveCount(0);

  // Zoom: 50–300% in 25% steps, reset to 100%.
  await viewer.getByRole("button", { name: "확대" }).click();
  await expect(viewer.getByText("125%")).toBeVisible();
  expect(await viewer.locator("table").evaluate((table) => getComputedStyle(table).zoom)).toBe("1.25");
  await viewer.getByRole("button", { name: "원래 크기" }).click();
  await expect(viewer.getByText("100%")).toBeVisible();

  // Sheet 2: chartsheet is unavailable with the original download; navigation stays.
  await viewer.getByRole("button", { name: "다음 시트" }).click();
  await expect(viewer.getByText("시트 선택: Chart (2/3)")).toBeVisible();
  const chartPane = viewer.locator("[data-xlsx-unsupported]");
  await expect(chartPane.getByText(unavailable)).toBeVisible();
  await expect(chartPane.getByRole("link", { name: "다운로드" })).toHaveAttribute("href", downloadUrl(bookId));
  await expect(viewer.locator("table")).toHaveCount(0);

  // Sheet 3: 201 rows × 65 columns → two row pages and two column pages.
  await viewer.getByRole("button", { name: "다음 시트" }).click();
  await expect(viewer.getByText("시트 선택: Grid (3/3)")).toBeVisible();
  await expect(viewer.getByRole("button", { name: "다음 시트" })).toBeDisabled();
  await expect(viewer.locator("[data-xlsx-row-page]")).toHaveText("1 / 2");
  await expect(viewer.locator("[data-xlsx-col-page]")).toHaveText("열 1/2");
  let grid = await cellTexts(page);
  expect(grid).toHaveLength(200);
  expect(grid[0]).toHaveLength(64);
  expect(grid[0]![0]).toBe("R1C1");
  expect(grid[199]![63]).toBe("R200C64");
  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await expect(viewer.locator("[data-xlsx-row-page]")).toHaveText("2 / 2");
  await expect(viewer.getByRole("button", { name: "다음 쪽" })).toBeDisabled();
  expect(await cellTexts(page)).toEqual([Array.from({ length: 64 }, (_, i) => `R201C${i + 1}`)]);
  await viewer.getByRole("button", { name: "다음 열" }).click();
  await expect(viewer.locator("[data-xlsx-col-page]")).toHaveText("열 2/2");
  expect(await cellTexts(page)).toEqual([["R201C65"]]);
  await viewer.getByRole("button", { name: "이전 열" }).click();
  await viewer.getByRole("button", { name: "이전 쪽" }).click();
  await expect(viewer.locator("[data-xlsx-row-page]")).toHaveText("1 / 2");
  grid = await cellTexts(page);
  expect(grid[0]![0]).toBe("R1C1");
  // Switching sheets resets the pages.
  await viewer.getByRole("button", { name: "다음 쪽" }).click();
  await viewer.getByRole("button", { name: "이전 시트" }).click();
  await viewer.getByRole("button", { name: "다음 시트" }).click();
  await expect(viewer.locator("[data-xlsx-row-page]")).toHaveText("1 / 2");

  // The original bytes stay downloadable unchanged.
  const original = await page.request.get(downloadUrl(bookId));
  expect(original.ok()).toBe(true);
  expect(Buffer.from(await original.body()).equals(Buffer.from(bookBytes))).toBe(true);

  // Two worksheets (source parity test).
  await page.goto(`/w/acme/a/${twoSheetsId}/view`);
  await expect(viewer.getByText("시트 선택: 첫 시트 (1/2)")).toBeVisible({ timeout: 20_000 });
  await expect(viewer.locator("td")).toHaveText(["FIRST SHEET"]);
  await viewer.getByRole("button", { name: "다음 시트" }).click();
  await expect(viewer.getByText("시트 선택: Second (2/2)")).toBeVisible();
  await expect(viewer.locator("td")).toHaveText(["SECOND SHEET"]);

  // Search hit: the extract-text supplement sits above the grid and never replaces it.
  await page.goto(`/w/acme/a/${twoSheetsId}/view?chunk=0`);
  await expect(page.locator("[data-chunk-supplement]")).toBeVisible({ timeout: 20_000 });
  await expect(viewer.getByText("시트 선택: 첫 시트 (1/2)")).toBeVisible();
  await expect(viewer.locator("td")).toHaveText(["FIRST SHEET"]);

  // Bounds: over the row cap and a small highly compressed part → download only.
  const overRowsId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "rows.xlsx",
    await buildFixtureXlsx([gridSheet("Rows", 20_001, 1)], { deflate: true }),
  );
  const bombBytes = await buildFixtureXlsx(
    [{ name: "Bomb", cells: [{ ref: "A1", inline: "x".repeat(40 * 1024 * 1024) }] }],
    { deflate: true },
  );
  expect(bombBytes.byteLength).toBeLessThan(1024 * 1024);
  const bombId = await uploadAttachment(page, wsId, documentId, "bomb.xlsx", bombBytes);
  for (const id of [overRowsId, bombId]) {
    await page.goto(`/w/acme/a/${id}/view`);
    await expect(page.getByRole("alert")).toHaveText(unavailable, { timeout: 20_000 });
    await expect(page.getByRole("button", { name: "다시 시도" })).toHaveCount(0);
    await expect(
      page.locator(".attachment-viewer__pane--center").getByRole("link", { name: "다운로드" }),
    ).toHaveAttribute("href", downloadUrl(id));
    await expect(page.getByTestId("xlsx-viewer")).toHaveCount(0);
  }

  // Not a workbook: load failure with retry and the original download.
  const brokenId = await uploadAttachment(
    page,
    wsId,
    documentId,
    "broken.xlsx",
    Buffer.from("이것은 xlsx가 아닙니다", "utf8"),
  );
  await page.goto(`/w/acme/a/${brokenId}/view`);
  await expect(page.getByRole("alert")).toHaveText(loadFailed, { timeout: 20_000 });
  await expect(page.getByRole("button", { name: "다시 시도" })).toBeVisible();

  // Download failure, then retry succeeds.
  let failures = 0;
  await page.route(`**${downloadUrl(twoSheetsId)}`, async (route) => {
    if (failures === 0) {
      failures += 1;
      await route.fulfill({ status: 500, body: "boom" });
      return;
    }
    await route.continue();
  });
  await page.goto(`/w/acme/a/${twoSheetsId}/view`);
  await expect(page.getByRole("alert")).toHaveText(loadFailed, { timeout: 20_000 });
  await page.getByRole("button", { name: "다시 시도" }).click();
  await expect(viewer.getByText("시트 선택: 첫 시트 (1/2)")).toBeVisible();
  expect(failures).toBe(1);
  await page.unroute(`**${downloadUrl(twoSheetsId)}`);

  // Stale URL: a late response for the previous workbook never paints over the next one.
  const hold = holdRoute();
  await page.route(`**${downloadUrl(bookId)}`, hold.handler);
  const bookSettled = new Promise<void>((resolve) => {
    const done = (request: { url: () => string }) => {
      if (request.url().endsWith(downloadUrl(bookId))) resolve();
    };
    page.on("requestfinished", done);
    page.on("requestfailed", done);
  });
  await page.goto(`/w/acme/a/${bookId}/view`);
  await hold.requested;
  await page.evaluate((path) => {
    window.history.pushState({}, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/w/acme/a/${twoSheetsId}/view`);
  await expect(viewer.getByText("시트 선택: 첫 시트 (1/2)")).toBeVisible({ timeout: 20_000 });
  hold.release();
  await bookSettled;
  await expect(viewer.getByText("시트 선택: 첫 시트 (1/2)")).toBeVisible();
  await expect(page.getByText("한글 셀 😀")).toHaveCount(0);
  await page.unroute(`**${downloadUrl(bookId)}`);

  // Workbook content never triggers a fetch of its external link (or anything off-origin).
  const origin = new URL(page.url()).origin;
  expect(requestUrls.filter((url) => url.startsWith(new URL(FIXTURE_EXTERNAL_WORKBOOK).origin))).toEqual([]);
  expect(requestUrls.filter((url) => !/^(data|blob):/.test(url) && new URL(url).origin !== origin)).toEqual([]);
  expect(csp).toEqual([]);

  // Share: the same viewer over share bytes; revocation during the download fails closed.
  const shareRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${documentId}/share-links`, {
    data: { expiresInDays: 7 },
  });
  expect(shareRes.status(), await shareRes.text()).toBe(201);
  const share = (await shareRes.json()) as { id: string; url: string };
  const sharePath = new URL(share.url).pathname;
  const token = sharePath.split("/")[2]!;
  const shareDownload = `/api/v1/share/${token}/attachments/${bookId}/download`;
  const anon = await browser.newContext();
  const reader = await anon.newPage();
  const readerCsp = watchCspViolations(reader);
  const readerApi: string[] = [];
  reader.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.startsWith("/api/")) readerApi.push(path);
  });
  await reader.goto(`${sharePath}/attachments/${bookId}/view?chunk=0`);
  const shared = reader.getByTestId("xlsx-viewer");
  await expect(shared.getByText("시트 선택: 요약 📊 (1/3)")).toBeVisible({ timeout: 20_000 });
  // Share views have no preview-html supplement.
  await expect(reader.locator("[data-chunk-supplement]")).toHaveCount(0);
  await expect(shared.locator("td").first()).toHaveText("한글 셀 😀");
  await shared.getByRole("button", { name: "다음 시트" }).click();
  await expect(shared.locator("[data-xlsx-unsupported]").getByRole("link", { name: "다운로드" })).toHaveAttribute(
    "href",
    shareDownload,
  );

  const shareHold = holdRoute();
  await reader.route(`**${shareDownload}`, shareHold.handler);
  await reader.reload();
  await shareHold.requested;
  const revoke = await page.request.delete(`/api/v1/workspaces/${wsId}/share-links/${share.id}`);
  expect(revoke.ok(), await revoke.text()).toBeTruthy();
  shareHold.release();
  await expect(reader.getByRole("alert")).toHaveText(loadFailed, { timeout: 20_000 });
  await expect(reader.getByText("한글 셀 😀")).toHaveCount(0);
  await reader.getByRole("button", { name: "다시 시도" }).click();
  await expect(reader.getByRole("alert")).toHaveText(loadFailed);
  await reader.unroute(`**${shareDownload}`);
  expect((await reader.request.get(shareDownload)).status()).toBe(404);
  for (const path of readerApi) {
    expect(path.startsWith("/api/v1/share/")).toBe(true);
  }
  expect(readerCsp).toEqual([]);
  await anon.close();
});
