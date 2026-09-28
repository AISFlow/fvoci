// User-perceived performance baseline (opt-in; run through scripts/perf/run-perf-baseline.sh).
// Not a correctness suite: every sample is recorded, failures and timeouts included.
import fs from "node:fs";
import path from "node:path";
import { expect, test, type Browser, type BrowserContext, type Page, type WebSocket } from "@playwright/test";
import { buildFixtureDocx } from "../../src/features/attachments/docx-test-fixture";
import { buildFixturePdf, type FixturePage } from "../../src/features/attachments/pdf-test-fixture";
import { buildFixturePptx } from "../../src/features/attachments/pptx-test-fixture";
import { buildFixtureXlsx, gridSheet } from "../../src/features/attachments/xlsx-test-fixture";
import { createE2eUser } from "../helpers";
import {
  attachProbe,
  calibrate,
  elementPaint,
  eventsSince,
  firstContentfulPaint,
  inputsSince,
  interactions,
  longTasks,
  nodeNow,
  pageNow,
  paints,
  quietWindow,
  resourcesSince,
  stats,
  waitHit,
  watch,
  waterfall,
  writeJson,
  type Calibration,
  type LoadWindow,
  type ResourceEntry,
} from "./probe";

test.describe.configure({ mode: "serial" });

const DATASET = process.env.FVOCI_PERF_DATASET === "scaled" ? "scaled" : "minimal";
const N = Number(process.env.FVOCI_PERF_SAMPLES ?? 30);
// Diagnostic: keep sanitized per-sample resource waterfalls (paths only).
const WATERFALL = process.env.FVOCI_PERF_WATERFALL === "1";
// Suffix for partial re-runs (FVOCI_PERF_GREP) so they never overwrite a full run.
const TAG = (process.env.FVOCI_PERF_TAG ?? "").replace(/[^a-z0-9-]/gi, "");
const HIT_TIMEOUT = 20_000;
const REPO = path.resolve(import.meta.dirname, "../../../..");

const owner = {
  email: "perf-owner@example.com",
  password: "perfpass-owner-1",
  familyName: "성능",
  givenName: "소유자",
  workspaceSlug: "perf",
  workspaceName: "Perf 워크스페이스",
};
const member = {
  email: "perf-member@example.com",
  password: "perfpass-member-1",
  familyName: "성능",
  givenName: "멤버",
};

const loginUser = (i: number) => ({ email: `perf-login-${i}@example.com`, password: `perfpass-login-${i}` });

// Server budget: 30 logins per IP per 5 minutes. Keep a margin for the setup login.
const LOGIN_BUDGET = 25;
const LOGIN_WINDOW_MS = 5 * 60_000 + 2_000;
const loginTimes: number[] = [];
async function paceLogin(): Promise<number> {
  const started = Date.now();
  for (;;) {
    const now = Date.now();
    while (loginTimes.length && now - loginTimes[0]! > LOGIN_WINDOW_MS) loginTimes.shift();
    if (loginTimes.length < LOGIN_BUDGET) break;
    await new Promise((resolve) => setTimeout(resolve, LOGIN_WINDOW_MS - (now - loginTimes[0]!) + 50));
  }
  loginTimes.push(Date.now());
  return Date.now() - started;
}

// Body writes share a server budget (30 per user per 60 s); stay under it.
const WRITE_BUDGET = 25;
const WRITE_WINDOW_MS = 61_000;
const writeTimes: number[] = [];
async function paceWrite(): Promise<void> {
  for (;;) {
    const now = Date.now();
    while (writeTimes.length && now - writeTimes[0]! > WRITE_WINDOW_MS) writeTimes.shift();
    if (writeTimes.length < WRITE_BUDGET) break;
    await new Promise((resolve) => setTimeout(resolve, WRITE_WINDOW_MS - (now - writeTimes[0]!) + 50));
  }
  writeTimes.push(Date.now());
}

type Ctx = {
  wsId: string;
  projectId: string;
  projectKey: string;
  targets: { id: string; number: number; title: string; token: string; month: number }[];
  smallDoc: { displayId: string; marker: string };
  freshDocs: { displayId: string; marker: string }[];
  bigDoc: { displayId: string; marker: string; bytes: number } | null;
  attachments: Record<string, { id: string; bytes: number; source: string }>;
  sizes: Record<string, number>;
  ownerState: string;
  memberState: string;
};

let ctx: Ctx;
const loadLog: LoadWindow[] = [];
const results: Record<string, unknown> = {};
const summary: Record<string, ReturnType<typeof stats> & { unit: string; boundary: string }> = {};

function record(flow: string, metric: string, boundary: string, values: (number | null | undefined)[]): void {
  summary[`${flow}.${metric}`] = { ...stats(values), unit: "ms", boundary };
}

function flush(): void {
  writeJson(`results-${DATASET}${TAG}.json`, { dataset: DATASET, samplesTarget: N, loadWindows: loadLog, results });
  writeJson(`summary-${DATASET}${TAG}.json`, summary);
  const rows = ["key,boundary,n,failures,median,p95,max"];
  for (const [key, s] of Object.entries(summary)) {
    rows.push([key, s.boundary, s.n, s.failures, s.median ?? "", s.p95 ?? "", s.max ?? ""].join(","));
  }
  fs.writeFileSync(path.join(process.env.FVOCI_PERF_OUT!, `summary-${DATASET}${TAG}.csv`), `${rows.join("\n")}\n`);
}

const round = (v: number | null | undefined) => (typeof v === "number" ? Math.round(v * 10) / 10 : null);

async function newProbedContext(browser: Browser, storageState?: string): Promise<BrowserContext> {
  const context = await browser.newContext(storageState ? { storageState } : {});
  await attachProbe(context);
  return context;
}

/** Runs one sample; a thrown failure is kept as a failed sample, never dropped. */
async function guarded(samples: Record<string, unknown>[], base: Record<string, unknown>, fn: () => Promise<void>): Promise<void> {
  const before = samples.length;
  try {
    await fn();
  } catch (error) {
    samples.length = before;
    samples.push({ ...base, error: String(error instanceof Error ? error.message : error).split("\n")[0]!.slice(0, 160) });
  }
}

async function uploadAttachment(page: Page, wsId: string, documentId: string, name: string, bytes: Uint8Array): Promise<string> {
  const uploadRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${documentId}/uploads`, {
    data: { name, sizeBytes: bytes.length },
  });
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
      data: Buffer.from(bytes).subarray((part.partNumber - 1) * upload.partSizeBytes, part.partNumber * upload.partSizeBytes),
    });
    expect(put.ok(), await put.text()).toBeTruthy();
    parts.push({ partNumber: part.partNumber, etag: put.headers()["etag"]! });
  }
  const complete = await page.request.post(`/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`, {
    data: { parts },
  });
  expect(complete.ok(), await complete.text()).toBeTruthy();
  return upload.attachmentId;
}

async function createTasks(
  page: Page,
  wsId: string,
  projectId: string,
  specs: { title: string; startDate: string; dueDate: string }[],
): Promise<{ id: string; number: number }[]> {
  const out: { id: string; number: number }[] = [];
  for (let offset = 0; offset < specs.length; offset += 10) {
    const chunk = specs.slice(offset, offset + 10);
    const responses = await Promise.all(
      chunk.map((data) => page.request.post(`/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`, { data })),
    );
    for (const res of responses) {
      expect(res.status(), await res.text()).toBe(201);
      const body = (await res.json()) as { id: string; number: number };
      out.push({ id: body.id, number: body.number });
    }
  }
  return out;
}

function dateIn(month: number, day: number): string {
  return `2026-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
}

const FILLER_WORDS = ["기획", "검토", "배포", "회의", "문서", "디자인", "테스트", "정리", "보고", "분석"];

test("setup dataset", async ({ browser }) => {
  test.setTimeout(900_000);
  const context = await browser.newContext();
  const page = await context.newPage();
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
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });
  // One synthetic member per login sample: the server limits logins per
  // IP+email (10 / 5 min), and the run respects that instead of bypassing it.
  for (let i = 0; i < 2 * N + 2; i += 1) {
    createE2eUser(loginUser(i).email, loginUser(i).password, `로그인${i}`, {
      familyName: "성능",
      workspaceSlug: owner.workspaceSlug,
      membershipRole: "member",
    });
  }

  const wsRes = await page.request.get("/api/v1/me/workspaces");
  const wsId = ((await wsRes.json()) as { items: { id: string; slug: string }[] }).items.find(
    (w) => w.slug === owner.workspaceSlug,
  )!.id;
  const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: "PRF", name: "성능 측정", visibility: "workspace" },
  });
  expect(projectRes.status()).toBe(201);
  const project = (await projectRes.json()) as { id: string; key: string };

  // Measurement targets exist in both datasets: one per sample, spread over
  // the 12 months of 2026 so the gantt has a known row per month.
  const targetCount = Math.max(N + 6, 36);
  const targetSpecs = Array.from({ length: targetCount }, (_, k) => {
    const month = (k % 12) + 1;
    return {
      title: `측정 대상 #${k}# pt${k + 100}q`,
      startDate: dateIn(month, 3 + (k % 5)),
      dueDate: dateIn(month, 10 + (k % 5)),
    };
  });
  const created = await createTasks(page, wsId, project.id, targetSpecs);
  const targets = created.map((t, k) => ({
    ...t,
    title: targetSpecs[k]!.title,
    token: `pt${k + 100}q`,
    month: (k % 12) + 1,
  }));

  const docsRes = async (title: string) => {
    const res = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, { data: { parentId: null, title } });
    expect(res.ok(), await res.text()).toBeTruthy();
    return (await res.json()) as { id: string; displayId: string };
  };
  const smallMarker = "작은 문서 첫 문단 marker";
  const small = await docsRes("작은 측정 문서");
  await paceWrite();
  const put = await page.request.put(`/api/v1/workspaces/${wsId}/documents/${small.id}/body`, {
    data: { contentMd: `${smallMarker}\n\n두 번째 문단입니다.\n` },
  });
  expect(put.ok(), await put.text()).toBeTruthy();

  // Seeded now, while no collab room is open: a body PUT needs a free collab slot.
  const freshDocs: Ctx["freshDocs"] = [];
  for (let i = 0; i < N; i += 1) {
    const marker = `새 문서 ${i} 첫 문단 marker`;
    const doc = await docsRes(`첫 열기 측정 ${i}`);
    await paceWrite();
    const res = await page.request.put(`/api/v1/workspaces/${wsId}/documents/${doc.id}/body`, {
      data: { contentMd: `${marker}\n\n두 번째 문단입니다.\n` },
    });
    expect(res.ok(), await res.text()).toBeTruthy();
    freshDocs.push({ displayId: doc.displayId, marker });
  }
  const sizes: Record<string, number> = { targetTasks: targetCount, fillerTasks: 0, fillerDocuments: 0, firstOpenDocuments: N };
  let bigDoc: Ctx["bigDoc"] = null;
  if (DATASET === "scaled") {
    const fillerCount = Number(process.env.FVOCI_PERF_FILLER_TASKS ?? 2000);
    const filler = Array.from({ length: fillerCount }, (_, i) => {
      const month = (i % 12) + 1;
      return {
        title: `합성 태스크 ${i} ${FILLER_WORDS[i % FILLER_WORDS.length]} ${FILLER_WORDS[(i * 7) % FILLER_WORDS.length]}`,
        startDate: dateIn(month, 1 + (i % 20)),
        dueDate: dateIn(month, 5 + (i % 20)),
      };
    });
    await createTasks(page, wsId, project.id, filler);
    sizes.fillerTasks = fillerCount;
    const docCount = Number(process.env.FVOCI_PERF_FILLER_DOCS ?? 300);
    for (let i = 0; i < docCount; i += 10) {
      await Promise.all(
        Array.from({ length: Math.min(10, docCount - i) }, (_, j) => docsRes(`합성 문서 ${i + j} ${FILLER_WORDS[(i + j) % 10]}`)),
      );
    }
    sizes.fillerDocuments = docCount;
    const bigMarker = "큰 문서 첫 문단 marker";
    const paras = Number(process.env.FVOCI_PERF_BIG_DOC_PARAGRAPHS ?? 2000);
    const lines = [bigMarker, ""];
    for (let i = 0; i < paras; i += 1) {
      if (i % 50 === 0) lines.push(`## 절 ${i / 50}`, "");
      lines.push(`문단 ${i}: 합성 본문 텍스트 ${FILLER_WORDS[i % 10]} ${FILLER_WORDS[(i * 3) % 10]} lorem ipsum dolor sit amet.`, "");
    }
    const md = lines.join("\n");
    const big = await docsRes("큰 측정 문서");
    await paceWrite();
    const bigPut = await page.request.put(`/api/v1/workspaces/${wsId}/documents/${big.id}/body`, { data: { contentMd: md } });
    expect(bigPut.ok(), await bigPut.text()).toBeTruthy();
    bigDoc = { displayId: big.displayId, marker: bigMarker, bytes: Buffer.byteLength(md) };
    sizes.bigDocMarkdownBytes = bigDoc.bytes;
    sizes.bigDocParagraphs = paras;
  }

  const holder = await docsRes("첨부 보관 문서");
  // compat/fixtures/sample.pdf is a blank page, so the PDF is the e2e text fixture.
  const pdfPageCount = DATASET === "scaled" ? 100 : 5;
  const pdfPages: FixturePage[] = Array.from({ length: pdfPageCount }, (_, i) =>
    i % 2 === 0
      ? { text: `FVOCI PERF PAGE ${i + 1}`, script: "latin", band: "top-red" }
      : { text: "한글 문서", script: "korean", band: "bottom-blue" },
  );
  const xlsxRows = DATASET === "scaled" ? 2000 : 200;
  const files: Record<string, { name: string; bytes: Uint8Array; source: string }> = {
    pdf: { name: "pages.pdf", bytes: buildFixturePdf(pdfPages), source: `pdf-test-fixture buildFixturePdf(${pdfPages.length} pages)` },
    docx: { name: "layout.docx", bytes: buildFixtureDocx(), source: "docx-test-fixture buildFixtureDocx()" },
    hwp: { name: "sample.hwp", bytes: fs.readFileSync(path.join(REPO, "compat/fixtures/sample.hwp")), source: "compat/fixtures/sample.hwp" },
    xlsx: {
      name: "grid.xlsx",
      bytes: await buildFixtureXlsx([gridSheet("시트1", xlsxRows, 20)]),
      source: `xlsx-test-fixture gridSheet(${xlsxRows}x20)`,
    },
    pptx: { name: "deck.pptx", bytes: buildFixturePptx(), source: "pptx-test-fixture buildFixturePptx()" },
  };
  const attachments: Ctx["attachments"] = {};
  for (const [kind, f] of Object.entries(files)) {
    attachments[kind] = { id: await uploadAttachment(page, wsId, holder.id, f.name, f.bytes), bytes: f.bytes.length, source: f.source };
    sizes[`attachment_${kind}_bytes`] = f.bytes.length;
  }

  // Search corpus must be indexed before the search window (bounded readiness wait).
  const lastTarget = targets[targets.length - 1]!;
  await expect
    .poll(
      async () => {
        const res = await page.request.get(`/api/v1/workspaces/${wsId}/search?q=${lastTarget.token}&type=task`);
        if (!res.ok()) return false;
        return ((await res.json()) as { items: { title: string }[] }).items.some((i) => i.title === lastTarget.title);
      },
      { timeout: 300_000, intervals: [1000] },
    )
    .toBe(true);

  const ownerState = path.join(process.env.FVOCI_PERF_RUN_DIR!, "owner-state.json");
  await context.storageState({ path: ownerState });
  const memberContext = await browser.newContext();
  const memberPage = await memberContext.newPage();
  await memberPage.goto("/login");
  await memberPage.getByLabel("이메일").fill(member.email);
  await memberPage.getByLabel("비밀번호").fill(member.password);
  loginTimes.push(Date.now());
  await memberPage.getByRole("button", { name: "로그인", exact: true }).click();
  await memberPage.waitForURL(/\/$/);
  const memberState = path.join(process.env.FVOCI_PERF_RUN_DIR!, "member-state.json");
  await memberContext.storageState({ path: memberState });
  await memberContext.close();
  await context.close();

  ctx = {
    wsId,
    projectId: project.id,
    projectKey: project.key,
    targets,
    smallDoc: { displayId: small.displayId, marker: smallMarker },
    freshDocs,
    bigDoc,
    attachments,
    sizes,
    ownerState,
    memberState,
  };
  results.dataset = { name: DATASET, sizes };
  results.browserVersion = browser.version();
  flush();
});

// (a) first entry + login -> workspace shown
test("a: login to workspace shown", async ({ browser }) => {
  test.setTimeout(3_600_000);
  const samples: Record<string, unknown>[] = [];
  let userIndex = 0;
  const once = async (page: Page, mode: string) => {
    const user = loginUser(userIndex++);
    const t0 = await pageNow(page).catch(() => 0);
    const nav = mode !== "warm-spa";
    if (nav) await page.goto("/login");
    const loginButton = page.getByRole("button", { name: "로그인", exact: true });
    await expect(loginButton).toBeEnabled({ timeout: HIT_TIMEOUT });
    const navTiming = nav
      ? await page.evaluate(() => {
          const n = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
          return n ? { ttfb: n.responseStart, domContentLoaded: n.domContentLoadedEventEnd, load: n.loadEventEnd } : null;
        })
      : null;
    const fcp = nav ? await firstContentfulPaint(page) : null;
    await page.getByLabel("이메일").fill(user.email);
    await page.getByLabel("비밀번호").fill(user.password);
    const id = `ws-${samples.length}`;
    const pacedMs = await paceLogin();
    await watch(page, id, { selector: ".workspace-list__item strong", text: owner.workspaceName });
    const before = await pageNow(page);
    await loginButton.click();
    const hit = await waitHit(page, id, HIT_TIMEOUT);
    const paint = hit ? await elementPaint(page, id) : null;
    const click = (await inputsSince(page, before)).find((i) => i.t === "pointerdown")?.ts ?? null;
    const res = await resourcesSince(page, before);
    const loginApi = res.find((r) => r.name.endsWith("/auth/login"));
    samples.push({
      mode,
      fcp: round(fcp),
      nav: navTiming,
      clickToLoginResponse: loginApi && click !== null ? round(loginApi.end - click) : null,
      loginServerTtfb: loginApi ? round(loginApi.respStart - loginApi.reqStart) : null,
      clickToShownDom: hit && click !== null ? round(hit.dom - click) : null,
      clickToShownPaint: paint && click !== null ? round(paint - click) : null,
      clickToShownFrame: hit?.raf && click !== null ? round(hit.raf - click) : null,
      requestsAfterClick: res.length,
      loginStatusOk: loginApi ? hit !== null : null,
      pacedMs,
      pre: hit?.pre ?? false,
      t0,
    });
  };

  await quietWindow("a-cold", loadLog);
  for (let i = 0; i < N; i += 1) {
    const context = await newProbedContext(browser);
    const page = await context.newPage();
    await guarded(samples, { mode: "cold-context" }, () => once(page, "cold-context"));
    await context.close();
  }
  await quietWindow("a-warm", loadLog);
  const context = await newProbedContext(browser);
  const page = await context.newPage();
  await once(page, "cold-context");
  samples.pop(); // warm-up for the warm series, not a sample
  for (let i = 0; i < N; i += 1) {
    await page.request.post("/api/v1/auth/logout", { data: {} }).catch(() => null);
    await guarded(samples, { mode: "warm-reload" }, () => once(page, "warm-reload"));
  }
  await context.close();
  results.a = samples;
  for (const mode of ["cold-context", "warm-reload"]) {
    const s = samples.filter((x) => x.mode === mode);
    record(`a.${mode}`, "fcp", "paint (Paint Timing)", s.map((x) => x.fcp as number | null));
    record(`a.${mode}`, "clickToLoginResponse", "HTTP response end (Resource Timing)", s.map((x) => x.clickToLoginResponse as number | null));
    record(`a.${mode}`, "clickToShownDom", "DOM-observed", s.map((x) => x.clickToShownDom as number | null));
    record(`a.${mode}`, "clickToShownPaint", "paint (Element Timing)", s.map((x) => x.clickToShownPaint as number | null));
    record(`a.${mode}`, "clickToShownFrame", "next animation frame after DOM (not paint)", s.map((x) => x.clickToShownFrame as number | null));
  }
  flush();
});

// (b) task detail open + document open/edit-ready
test("b: task detail and document open", async ({ browser }) => {
  test.setTimeout(1_200_000);
  const samples: Record<string, unknown>[] = [];
  const openTask = async (page: Page, mode: string, target: Ctx["targets"][number]) => {
    const idTitle = `tt-${samples.length}`;
    const idReady = `tr-${samples.length}`;
    let start: number;
    if (mode === "warm-spa") {
      await watch(page, idTitle, { selector: "h1.task-detail__title", text: target.title });
      await watch(page, idReady, { selector: '[data-testid="task-body"] [data-collab-status="connected"]' });
      start = await pageNow(page);
      await page.locator(`a.task-row[href$="/${ctx.projectKey}-${target.number}"]`).first().click();
      start = (await inputsSince(page, start)).find((i) => i.t === "pointerdown")?.ts ?? start;
    } else {
      await page.goto("about:blank");
      await page.goto(`/w/${owner.workspaceSlug}/${ctx.projectKey}-${target.number}`, { waitUntil: "commit" });
      await watch(page, idTitle, { selector: "h1.task-detail__title", text: target.title });
      await watch(page, idReady, { selector: '[data-testid="task-body"] [data-collab-status="connected"]' });
      start = 0; // navigation start of this document
    }
    const title = await waitHit(page, idTitle, HIT_TIMEOUT);
    const ready = await waitHit(page, idReady, HIT_TIMEOUT);
    const editable = ready
      ? await page
          .locator('[data-testid="task-body"] .ProseMirror[contenteditable="true"]')
          .waitFor({ timeout: HIT_TIMEOUT })
          .then(() => pageNow(page))
          .catch(() => null)
      : null;
    const titlePaint = title ? await elementPaint(page, idTitle) : null;
    const res = await resourcesSince(page, mode === "warm-spa" ? start : 0);
    const api = res.filter((r) => r.name.includes("/api/v1/"));
    samples.push({
      kind: "task",
      mode,
      titleDom: title && !title.pre ? round(title.dom - start) : null,
      titlePaint: titlePaint && !title?.pre ? round(titlePaint - start) : null,
      collabConnectedDom: ready && !ready.pre ? round(ready.dom - start) : null,
      editableObserved: editable !== null ? round(editable - start) : null,
      apiCount: api.length,
      apiMaxTtfb: round(Math.max(0, ...api.map((r) => r.respStart - r.reqStart))),
      apiLastEnd: api.length ? round(Math.max(...api.map((r) => r.end)) - start) : null,
      longTasksBeforeTitle: title ? await longTasks(page, start, title.dom) : null,
      ...(WATERFALL ? { waterfall: waterfall(res) } : {}),
    });
    if (mode === "warm-spa") {
      await page.goBack();
      await expect(page.locator(".task-status-list")).toBeVisible({ timeout: HIT_TIMEOUT });
    }
  };

  await quietWindow("b-task-cold", loadLog);
  for (let i = 0; i < N; i += 1) {
    const context = await newProbedContext(browser, ctx.ownerState);
    const page = await context.newPage();
    await guarded(samples, { kind: "task", mode: "cold-context" }, () => openTask(page, "cold-context", ctx.targets[i % ctx.targets.length]!));
    await context.close();
  }
  await quietWindow("b-task-warm", loadLog);
  {
    const context = await newProbedContext(browser, ctx.ownerState);
    const page = await context.newPage();
    await page.goto(`/w/${owner.workspaceSlug}/${ctx.projectKey}/tasks`);
    await expect(page.locator(".task-status-list")).toBeVisible({ timeout: HIT_TIMEOUT });
    // Warm SPA re-open of tasks already listed on the first page.
    const listed = await page.locator("a.task-row").evaluateAll((els) => els.map((e) => e.getAttribute("href") ?? ""));
    const visible = ctx.targets.filter((t) => listed.some((h) => h.endsWith(`/${ctx.projectKey}-${t.number}`)));
    const pool = visible.length ? visible : ctx.targets;
    await openTask(page, "warm-spa", pool[0]!);
    samples.pop(); // first SPA open loads route chunks: warm-up
    for (let i = 0; i < N; i += 1) {
      await guarded(samples, { kind: "task", mode: "warm-spa" }, () => openTask(page, "warm-spa", pool[i % Math.min(pool.length, 3)]!));
    }
    await context.close();
  }

  const docs: { label: string; displayId: string; marker: string }[] = [{ label: "small", ...ctx.smallDoc }];
  if (ctx.bigDoc) docs.push({ label: "big", displayId: ctx.bigDoc.displayId, marker: ctx.bigDoc.marker });
  for (const doc of docs) {
    const openDoc = async (page: Page, mode: string) => {
      const idText = `dt-${samples.length}`;
      const idReady = `dr-${samples.length}`;
      await page.goto("about:blank");
      await page.goto(`/w/${owner.workspaceSlug}/${doc.displayId}`, { waitUntil: "commit" });
      await watch(page, idText, { selector: ".fvoci-editor .ProseMirror p", text: doc.marker });
      await watch(page, idReady, { selector: '[data-collab-status="connected"]' });
      const text = await waitHit(page, idText, 60_000);
      const ready = await waitHit(page, idReady, 60_000);
      const editable = await page
        .locator('.fvoci-editor .ProseMirror[contenteditable="true"]')
        .waitFor({ timeout: 60_000 })
        .then(() => pageNow(page))
        .catch(() => null);
      const textPaint = text ? await elementPaint(page, idText) : null;
      const res = await resourcesSince(page, 0);
      const api = res.filter((r) => r.name.includes("/api/v1/"));
      const lcp = await page.evaluate(
        () =>
          new Promise<number | null>((resolve) => {
            try {
              new PerformanceObserver((l) => {
                const e = l.getEntries();
                resolve(e.length ? e[e.length - 1]!.startTime : null);
              }).observe({ type: "largest-contentful-paint", buffered: true });
              setTimeout(() => resolve(null), 200);
            } catch {
              resolve(null);
            }
          }),
      );
      samples.push({
        kind: `doc-${doc.label}`,
        mode,
        textDom: text && !text.pre ? round(text.dom) : null,
        textPaint: round(textPaint),
        textFrame: text && !text.pre && text.raf ? round(text.raf) : null,
        lcpFullLoad: round(lcp),
        collabConnectedDom: ready && !ready.pre ? round(ready.dom) : null,
        editableObserved: round(editable),
        apiCount: api.length,
        apiBytes: api.reduce((a, r) => a + r.bytes, 0),
        apiMaxTtfb: round(Math.max(0, ...api.map((r) => r.respStart - r.reqStart))),
        longTasksBeforeText: text ? await longTasks(page, 0, text.dom) : null,
        ...(WATERFALL ? { waterfall: waterfall(res) } : {}),
      });
    };
    await quietWindow(`b-doc-${doc.label}-cold`, loadLog);
    for (let i = 0; i < N; i += 1) {
      const context = await newProbedContext(browser, ctx.ownerState);
      const page = await context.newPage();
      await guarded(samples, { kind: `doc-${doc.label}`, mode: "cold-context" }, () => openDoc(page, "cold-context"));
      await context.close();
    }
    await quietWindow(`b-doc-${doc.label}-warm`, loadLog);
    const context = await newProbedContext(browser, ctx.ownerState);
    const page = await context.newPage();
    await openDoc(page, "warm-reload");
    samples.pop();
    for (let i = 0; i < N; i += 1) {
      await guarded(samples, { kind: `doc-${doc.label}`, mode: "warm-reload" }, () => openDoc(page, "warm-reload"));
    }
    await context.close();
  }
  results.b = samples;
  const kinds = [...new Set(samples.map((s) => `${s.kind}|${s.mode}`))];
  for (const key of kinds) {
    const [kind, mode] = key.split("|");
    const s = samples.filter((x) => x.kind === kind && x.mode === mode);
    const flow = `b.${kind}.${mode}`;
    const pick = (m: string) => s.map((x) => x[m] as number | null);
    if (kind === "task") {
      record(flow, "titleDom", "DOM-observed", pick("titleDom"));
      record(flow, "titlePaint", "paint (Element Timing)", pick("titlePaint"));
      record(flow, "collabConnectedDom", "DOM-observed (collab ack state)", pick("collabConnectedDom"));
      record(flow, "editableObserved", "DOM-observed (runner poll)", pick("editableObserved"));
    } else {
      record(flow, "textDom", "DOM-observed", pick("textDom"));
      record(flow, "textPaint", "paint (Element Timing)", pick("textPaint"));
      record(flow, "textFrame", "next animation frame after DOM (not paint)", pick("textFrame"));
      record(flow, "collabConnectedDom", "DOM-observed (collab ack state)", pick("collabConnectedDom"));
      record(flow, "editableObserved", "DOM-observed (runner poll)", pick("editableObserved"));
    }
  }
  flush();
});

// (c) body typing, menu, gantt interaction
test("c: typing, menu and gantt interactions", async ({ browser }) => {
  test.setTimeout(1_200_000);
  const out: Record<string, unknown> = {};
  const context = await newProbedContext(browser, ctx.ownerState);
  const page = await context.newPage();

  const typeInto = async (label: string, url: string, editorSel: string) => {
    await page.goto(url);
    await expect(page.locator('[data-collab-status="connected"]').first()).toBeVisible({ timeout: 60_000 });
    const editor = page.locator(`${editorSel} .ProseMirror[contenteditable="true"]`).first();
    await editor.click();
    await page.keyboard.press("Control+End");
    await page.keyboard.press("Enter");
    await quietWindow(`c-typing-${label}`, loadLog);
    const since = await pageNow(page);
    const keys = "thequickbrownfoxjumpsoverthelazydog0123456789abcdefghijklmnopq".slice(0, Math.max(N, 60));
    for (const k of keys) {
      await page.keyboard.press(k);
      await page.waitForTimeout(80); // human-like spacing so each key is its own interaction (pacing, not a result)
    }
    await page.waitForTimeout(500);
    // The keys must have reached this editor; otherwise the series is invalid.
    const typedVisible = await editor.evaluate((el, k) => (el.textContent ?? "").includes(k), keys.slice(0, 20));
    const focusInEditor = await editor.evaluate((el) => el.contains(document.activeElement) || el === document.activeElement);
    const inputs = (await inputsSince(page, since)).filter((i) => i.t === "keydown");
    const ints = interactions(await eventsSince(page, since));
    // Interactions below the 16 ms Event Timing threshold have no entry; they are
    // counted as "<16" and enter the stats at the 16 ms upper bound.
    const durations = inputs.map((_, idx) => ints[idx]?.duration ?? null);
    const below = inputs.length - ints.length;
    const values = [...ints.map((x) => x.duration), ...Array.from({ length: Math.max(0, below) }, () => 16)];
    out[`typing-${label}`] = { typedVisible, focusInEditor, keydowns: inputs.length, entries: ints.length, below16: below, interactions: ints, durations };
    record(
      `c.typing.${label}`,
      "interactionDuration",
      "scripted-scenario INP-style (Event Timing, <16ms counted as 16)",
      typedVisible ? values : inputs.map(() => null),
    );
    record(`c.typing.${label}`, "inputDelay", "Event Timing processingStart-startTime (entries >=16ms only)", ints.map((x) => x.inputDelay));
  };

  const target = ctx.targets[0]!;
  await typeInto("task-body", `/w/${owner.workspaceSlug}/${ctx.projectKey}-${target.number}`, '[data-testid="task-body"]');
  if (ctx.bigDoc) await typeInto("big-doc", `/w/${owner.workspaceSlug}/${ctx.bigDoc.displayId}`, "");

  // Menu: quick-search palette open (Ctrl+K) -> dialog painted, then Escape.
  await page.goto(`/w/${owner.workspaceSlug}/${ctx.projectKey}/tasks`);
  await expect(page.locator(".task-status-list")).toBeVisible({ timeout: HIT_TIMEOUT });
  await quietWindow("c-menu", loadLog);
  const menu: Record<string, unknown>[] = [];
  const palette = page.getByRole("dialog", { name: "빠른 검색" });
  for (let i = 0; i < N + 1; i += 1) {
    const id = `menu-${i}`;
    await watch(page, id, { selector: ".search-command__title" });
    const since = await pageNow(page);
    await page.keyboard.press("Control+k");
    const hit = await waitHit(page, id, HIT_TIMEOUT);
    const paint = hit ? await elementPaint(page, id) : null;
    await page.waitForTimeout(100);
    const key = (await inputsSince(page, since)).find((x) => x.t === "keydown");
    const ints = interactions(await eventsSince(page, since));
    menu.push({
      keyToDom: hit && key ? round(hit.dom - key.ts) : null,
      keyToPaint: paint && key ? round(paint - key.ts) : null,
      interaction: ints[0]?.duration ?? "<16",
    });
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden({ timeout: HIT_TIMEOUT });
  }
  menu.shift(); // first open loads nothing extra but is kept out as warm-up
  out.menu = menu;
  record("c.menu", "keyToPaint", "paint (Element Timing)", menu.map((m) => m.keyToPaint as number | null));
  record("c.menu", "keyToDom", "DOM-observed", menu.map((m) => m.keyToDom as number | null));
  record(
    "c.menu",
    "interactionDuration",
    "scripted-scenario INP-style (Event Timing, <16ms counted as 16)",
    menu.map((m) => (typeof m.interaction === "number" ? m.interaction : 16)),
  );

  // Gantt: month navigation; result = a known row of the new month painted.
  await page.goto(`/w/${owner.workspaceSlug}/${ctx.projectKey}/gantt?y=2026&m=1`);
  const chart = page.locator('[data-slot="gantt"]');
  await expect(chart.locator(".fvoci-gantt__row-label", { hasText: "#0#" })).toBeVisible({ timeout: 60_000 });
  await quietWindow("c-gantt", loadLog);
  const gantt: Record<string, unknown>[] = [];
  let month = 1;
  let dir = 1;
  const seen = new Set<number>([1]);
  for (let i = 0; i < N + 3; i += 1) {
    if (month + dir < 1 || month + dir > 12) dir = -dir;
    const next = month + dir;
    const id = `gantt-${i}`;
    await watch(page, id, { selector: ".fvoci-gantt__row-label", text: `#${next - 1}#` });
    const since = await pageNow(page);
    await page.getByRole("button", { name: dir > 0 ? "다음 달" : "이전 달" }).click();
    const hit = await waitHit(page, id, 60_000);
    const paint = hit ? await elementPaint(page, id) : null;
    await page.waitForTimeout(50);
    const click = (await inputsSince(page, since)).find((x) => x.t === "pointerdown");
    const ints = interactions(await eventsSince(page, since));
    const res = (await resourcesSince(page, since)).filter((r) => r.name.includes("/api/v1/"));
    gantt.push({
      month: next,
      firstVisit: !seen.has(next),
      clickToDom: hit && click ? round(hit.dom - click.ts) : null,
      clickToPaint: paint && click ? round(paint - click.ts) : null,
      clickToFrame: hit?.raf && click ? round(hit.raf - click.ts) : null,
      interaction: ints.length ? Math.max(...ints.map((x) => x.duration)) : "<16",
      apiCount: res.length,
      apiMs: res.length ? round(Math.max(...res.map((r) => r.end)) - Math.min(...res.map((r) => r.start))) : 0,
      apiBytes: res.reduce((a, r) => a + r.bytes, 0),
      rows: await chart.locator(".fvoci-gantt__row-label").count(),
    });
    seen.add(next);
    month = next;
  }
  out.gantt = gantt;
  record("c.gantt.all", "clickToPaint", "paint (Element Timing)", gantt.map((g) => g.clickToPaint as number | null));
  record("c.gantt.all", "clickToDom", "DOM-observed", gantt.map((g) => g.clickToDom as number | null));
  record("c.gantt.all", "clickToFrame", "next animation frame after DOM (not paint)", gantt.map((g) => g.clickToFrame as number | null));
  record(
    "c.gantt.all",
    "interactionDuration",
    "scripted-scenario INP-style (Event Timing, <16ms counted as 16)",
    gantt.map((g) => (typeof g.interaction === "number" ? g.interaction : 16)),
  );
  record("c.gantt.firstVisit", "clickToPaint", "paint (Element Timing)", gantt.filter((g) => g.firstVisit).map((g) => g.clickToPaint as number | null));
  record("c.gantt.revisit", "clickToPaint", "paint (Element Timing)", gantt.filter((g) => !g.firstVisit).map((g) => g.clickToPaint as number | null));
  await context.close();
  results.c = out;
  flush();
});

// (d) search input -> matching result shown
test("d: quick search", async ({ browser }) => {
  test.setTimeout(900_000);
  const context = await newProbedContext(browser, ctx.ownerState);
  const page = await context.newPage();
  await page.goto(`/w/${owner.workspaceSlug}/${ctx.projectKey}/tasks`);
  await expect(page.locator(".task-status-list")).toBeVisible({ timeout: HIT_TIMEOUT });
  const palette = page.getByRole("dialog", { name: "빠른 검색" });
  await quietWindow("d-search", loadLog);
  const samples: Record<string, unknown>[] = [];
  for (let i = 0; i < N; i += 1) {
    await guarded(samples, { i }, async () => {
    const target = ctx.targets[i]!;
    await page.keyboard.press("Control+k");
    const input = palette.getByLabel("검색어");
    await expect(input).toBeFocused();
    // The palette keeps its draft across open/close: clear it and let the
    // cleared (debounced) query settle before the sample starts.
    await page.keyboard.press("Control+a");
    await page.keyboard.press("Backspace");
    await expect(palette.locator(".search-command__hint")).toBeVisible({ timeout: HIT_TIMEOUT });
    const id = `search-${i}`;
    await watch(page, id, { selector: ".search-command__dialog a", text: target.title });
    const since = await pageNow(page);
    await page.keyboard.type(target.token, { delay: 60 });
    const hit = await waitHit(page, id, HIT_TIMEOUT);
    const paint = hit ? await elementPaint(page, id) : null;
    const keys = (await inputsSince(page, since)).filter((x) => x.t === "keydown");
    const last = keys[keys.length - 1];
    const res = (await resourcesSince(page, since)).filter((r) => r.name.endsWith("/search"));
    const final = res[res.length - 1];
    const ints = interactions(await eventsSince(page, since));
    samples.push({
      chars: target.token.length,
      searchRequests: res.length,
      lastKeyToRequest: final && last ? round(final.start - last.ts) : null,
      searchRequestMs: final ? round(final.end - final.start) : null,
      searchServerTtfb: final ? round(final.respStart - final.reqStart) : null,
      responseToDom: final && hit ? round(hit.dom - final.end) : null,
      lastKeyToDom: hit && last ? round(hit.dom - last.ts) : null,
      lastKeyToPaint: paint && last ? round(paint - last.ts) : null,
      lastKeyToFrame: hit?.raf && last ? round(hit.raf - last.ts) : null,
      pre: hit?.pre ?? false,
      keyInteractionMax: ints.length ? Math.max(...ints.map((x) => x.duration)) : "<16",
    });
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden({ timeout: HIT_TIMEOUT });
    });
    if (await palette.isVisible()) await page.keyboard.press("Escape");
  }
  await context.close();
  results.d = samples;
  const pick = (m: string) => samples.map((x) => x[m] as number | null);
  record("d.search", "lastKeyToPaint", "paint (Element Timing)", pick("lastKeyToPaint"));
  record("d.search", "lastKeyToDom", "DOM-observed", pick("lastKeyToDom"));
  record("d.search", "lastKeyToFrame", "next animation frame after DOM (not paint)", pick("lastKeyToFrame"));
  record("d.search", "lastKeyToRequest", "client debounce (Resource Timing start)", pick("lastKeyToRequest"));
  record("d.search", "searchRequestMs", "HTTP (Resource Timing)", pick("searchRequestMs"));
  record("d.search", "searchServerTtfb", "HTTP TTFB on loopback", pick("searchServerTtfb"));
  record("d.search", "responseToDom", "browser JS/render to DOM", pick("responseToDom"));
  flush();
});

// `text` stays in memory only (to find the frame carrying a synthetic token); it is never written out.
type FrameLog = { t: number; dir: "sent" | "recv"; persist: "request" | "done" | null; bytes: number; text: string }[];

function logFrames(page: Page, frames: FrameLog): void {
  page.on("websocket", (ws: WebSocket) => {
    const onFrame = (dir: "sent" | "recv") => (data: { payload: string | Buffer }) => {
      const t = nodeNow();
      const buf = typeof data.payload === "string" ? Buffer.from(data.payload) : data.payload;
      const text = buf.toString("latin1");
      const persist = text.includes("persisted:") ? "done" : text.includes("persist:") ? "request" : null;
      frames.push({ t, dir, persist, bytes: buf.length, text });
    };
    ws.on("framesent", onFrame("sent"));
    ws.on("framereceived", onFrame("recv"));
  });
}

const toNode = (abs: number, cal: Calibration) => abs - cal.offset;

// (e) save confirmation + cross-browser change visible
test("e: save ack and remote reflection", async ({ browser }) => {
  test.setTimeout(1_200_000);
  const target = ctx.targets[1]!;
  const aCtx = await newProbedContext(browser, ctx.ownerState);
  const bCtx = await newProbedContext(browser, ctx.memberState);
  const a = await aCtx.newPage();
  const b = await bCtx.newPage();
  const aFrames: FrameLog = [];
  const bFrames: FrameLog = [];
  logFrames(a, aFrames);
  logFrames(b, bFrames);
  const url = `/w/${owner.workspaceSlug}/${ctx.projectKey}-${target.number}`;
  await a.goto(url);
  await b.goto(url);
  for (const p of [a, b]) {
    await expect(p.locator('[data-testid="task-body"] [data-collab-status="connected"]')).toBeVisible({ timeout: 60_000 });
  }
  const aEditor = a.locator('[data-testid="task-body"] .ProseMirror[contenteditable="true"]');
  await aEditor.click();
  await a.keyboard.press("Control+End");

  await quietWindow("e-body", loadLog);
  const body: Record<string, unknown>[] = [];
  const calBefore = [await calibrate(a), await calibrate(b)];
  for (let i = 0; i < N; i += 1) {
    await guarded(body, { i }, async () => {
    const token = `remote${i}x${Date.now() % 100000}`;
    await aEditor.click();
    await a.keyboard.press("Control+End");
    await a.keyboard.press("Enter");
    // Let the provider's 200 ms flushDelay window (started by Enter/cursor
    // awareness) drain so the measured insert starts from an idle provider.
    await a.waitForTimeout(400);
    const idB = `rb-${i}`;
    await watch(b, idB, { selector: '[data-testid="task-body"] .ProseMirror p', text: token });
    const calA = await calibrate(a, 5);
    const calB = await calibrate(b, 5);
    const sinceA = await pageNow(a);
    const frameMark = aFrames.length;
    const bFrameMark = bFrames.length;
    await a.keyboard.insertText(token);
    const hitB = await waitHit(b, idB, HIT_TIMEOUT);
    const paintB = hitB ? await elementPaint(b, idB) : null;
    const aInput = (await inputsSince(a, sinceA)).find((x) => x.t === "beforeinput" || x.t === "input");
    const aInputNode = aInput ? toNode(await a.evaluate((ts) => performance.timeOrigin + ts, aInput.ts), calA) : null;
    const bTimeOrigin = await b.evaluate(() => performance.timeOrigin);
    const bDomNode = hitB ? toNode(hitB.abs, calB) : null;
    const bPaintNode = paintB ? toNode(bTimeOrigin + paintB, calB) : null;
    const bFrameNode = hitB?.raf ? toNode(bTimeOrigin + hitB.raf, calB) : null;
    const aSent = aFrames.slice(frameMark).find((f) => f.dir === "sent" && f.text.includes(token));
    const bRecv = bFrames.slice(bFrameMark).find((f) => f.dir === "recv" && f.text.includes(token));

    // Save: click -> persisted ack reflected in A's state (DOM-observed).
    await expect(a.locator('[data-testid="task-body"] [data-collab-persisted="false"]')).toBeVisible({ timeout: HIT_TIMEOUT }).catch(() => null);
    const idSave = `save-${i}`;
    await watch(a, idSave, { selector: '[data-testid="task-body"] [data-collab-persisted]', attr: ["data-collab-persisted", "true"] });
    const sinceSave = await pageNow(a);
    const persistMark = aFrames.length;
    await a.locator('[data-testid="task-body"]').getByRole("button", { name: "저장", exact: true }).click();
    const saved = await waitHit(a, idSave, HIT_TIMEOUT);
    const click = (await inputsSince(a, sinceSave)).find((x) => x.t === "pointerdown");
    const req = aFrames.slice(persistMark).find((f) => f.persist === "request");
    const done = aFrames.slice(persistMark).find((f) => f.persist === "done");
    const clickNode = click ? toNode(await a.evaluate((ts) => performance.timeOrigin + ts, click.ts), calA) : null;
    body.push({
      calRttA: round(calA.rtt),
      calRttB: round(calB.rtt),
      inputToASendNodeClock: aSent && aInputNode ? round(aSent.t - aInputNode) : null,
      aSendToBRecvNodeClock: aSent && bRecv ? round(bRecv.t - aSent.t) : null,
      inputToRemoteDom: bDomNode && aInputNode ? round(bDomNode - aInputNode) : null,
      inputToRemotePaint: bPaintNode && aInputNode ? round(bPaintNode - aInputNode) : null,
      inputToRemoteFrame: bFrameNode && aInputNode ? round(bFrameNode - aInputNode) : null,
      saveClickToAckDom: saved && !saved.pre && click ? round(saved.dom - click.ts) : null,
      saveClickToAckFrameNodeClock: done && clickNode ? round(done.t - clickNode) : null,
      persistRequestToAckFrame: done && req ? round(done.t - req.t) : null,
      savePre: saved?.pre ?? false,
    });
    });
  }
  const calAfter = [await calibrate(a), await calibrate(b)];
  results.eCalibration = {
    before: calBefore,
    after: calAfter,
    driftA: round(calAfter[0]!.offset - calBefore[0]!.offset),
    driftB: round(calAfter[1]!.offset - calBefore[1]!.offset),
    pageOffsetDelta: round(calBefore[0]!.offset - calBefore[1]!.offset),
  };
  results.eBody = body;
  const pick = (m: string) => body.map((x) => x[m] as number | null);
  record("e.body", "inputToRemotePaint", "remote paint (Element Timing, Node-clock aligned)", pick("inputToRemotePaint"));
  record("e.body", "inputToRemoteDom", "remote DOM-observed (Node-clock aligned)", pick("inputToRemoteDom"));
  record("e.body", "inputToRemoteFrame", "remote next animation frame after DOM, not paint (Node-clock aligned)", pick("inputToRemoteFrame"));
  record("e.body", "inputToASendNodeClock", "client batching until WS send (Node receipt of CDP frame event)", pick("inputToASendNodeClock"));
  record("e.body", "aSendToBRecvNodeClock", "server relay WS->WS (Node receipt of CDP frame events)", pick("aSendToBRecvNodeClock"));
  record("e.body", "saveClickToAckDom", "server persistence ack reflected in client state (DOM-observed)", pick("saveClickToAckDom"));
  record("e.body", "persistRequestToAckFrame", "server persist request->ack WS frames (Node clock)", pick("persistRequestToAckFrame"));

  // Task meta edit in A (title blur -> PATCH) -> B's task list via task SSE.
  await b.goto(`/w/${owner.workspaceSlug}/${ctx.projectKey}/tasks`);
  await expect(b.locator(".task-status-list")).toBeVisible({ timeout: HIT_TIMEOUT });
  const firstRow = await b.locator("a.task-row").first().getAttribute("href");
  const number = Number(firstRow?.split("-").pop());
  const metaTask = ctx.targets.find((t) => t.number === number) ?? ctx.targets[2]!;
  await a.goto(`/w/${owner.workspaceSlug}/${ctx.projectKey}-${metaTask.number}`);
  const titleInput = a.getByLabel("태스크 제목");
  await expect(titleInput).toBeEditable({ timeout: HIT_TIMEOUT });
  await b.waitForResponse((r) => r.url().includes("/stream"), { timeout: 5_000 }).catch(() => null);
  await quietWindow("e-meta", loadLog);
  const meta: Record<string, unknown>[] = [];
  for (let i = 0; i < N; i += 1) {
    await guarded(meta, { i }, async () => {
    const title = `원격 제목 ${i} ${Date.now() % 100000}`;
    const idB = `meta-${i}`;
    await watch(b, idB, { selector: ".task-row__title", text: title });
    await titleInput.fill(title);
    // The task stream polls on a free-running 750 ms ticker; a fixed sample
    // cadence would phase-lock to it. Low-discrepancy start offsets spread the
    // samples over the whole poll period (pacing, recorded, not a result).
    const jitterMs = Math.round(((i * 0.6180339887) % 1) * 750);
    await a.waitForTimeout(jitterMs);
    const calA = await calibrate(a, 5);
    const calB = await calibrate(b, 5);
    const sinceA = await pageNow(a);
    const sinceB = await pageNow(b);
    await titleInput.press("Tab");
    const hitB = await waitHit(b, idB, HIT_TIMEOUT);
    const paintB = hitB ? await elementPaint(b, idB) : null;
    const tab = (await inputsSince(a, sinceA)).find((x) => x.t === "keydown");
    const patch = (await resourcesSince(a, sinceA)).find((r) => /\/tasks\/:id$/.test(r.name));
    const aOrigin = await a.evaluate(() => performance.timeOrigin);
    const bOrigin = await b.evaluate(() => performance.timeOrigin);
    const tabNode = tab ? toNode(aOrigin + tab.ts, calA) : null;
    const patchEndNode = patch ? toNode(aOrigin + patch.end, calA) : null;
    const refetch = (await resourcesSince(b, sinceB)).filter((r: ResourceEntry) => r.name.endsWith("/tasks") || r.name.includes("/tasks?"));
    const firstRefetch = refetch[0];
    const refetchStartNode = firstRefetch ? toNode(bOrigin + firstRefetch.start, calB) : null;
    meta.push({
      tabToPatchResponse: patch && tab ? round(patch.end - tab.ts) : null,
      patchServerTtfb: patch ? round(patch.respStart - patch.reqStart) : null,
      patchEndToRemoteRefetchStart: refetchStartNode && patchEndNode ? round(refetchStartNode - patchEndNode) : null,
      remoteRefetchMs: firstRefetch ? round(firstRefetch.end - firstRefetch.start) : null,
      remoteRefetchBytes: firstRefetch?.bytes ?? null,
      remoteRefetchToDom: firstRefetch && hitB ? round(hitB.dom - firstRefetch.end) : null,
      tabToRemoteDom: hitB && tabNode ? round(toNode(hitB.abs, calB) - tabNode) : null,
      tabToRemotePaint: paintB && tabNode ? round(toNode(bOrigin + paintB, calB) - tabNode) : null,
      tabToRemoteFrame: hitB?.raf && tabNode ? round(toNode(bOrigin + hitB.raf, calB) - tabNode) : null,
      refetchCount: refetch.length,
      jitterMs,
    });
    // Next sample starts after this change settled on both sides.
    await expect(titleInput).toHaveValue(title);
    });
  }
  results.eMeta = meta;
  const pm = (m: string) => meta.map((x) => x[m] as number | null);
  record("e.meta", "tabToPatchResponse", "HTTP ack (Resource Timing)", pm("tabToPatchResponse"));
  record("e.meta", "patchEndToRemoteRefetchStart", "SSE hint delivery incl. 750ms poll (Node-clock aligned)", pm("patchEndToRemoteRefetchStart"));
  record("e.meta", "remoteRefetchMs", "remote list refetch HTTP", pm("remoteRefetchMs"));
  record("e.meta", "remoteRefetchToDom", "remote JS/render to DOM", pm("remoteRefetchToDom"));
  record("e.meta", "tabToRemoteDom", "remote DOM-observed (Node-clock aligned)", pm("tabToRemoteDom"));
  record("e.meta", "tabToRemoteFrame", "remote next animation frame after DOM, not paint (Node-clock aligned)", pm("tabToRemoteFrame"));
  record("e.meta", "tabToRemotePaint", "remote paint (Element Timing, Node-clock aligned)", pm("tabToRemotePaint"));
  await aCtx.close();
  await bCtx.close();
  flush();
});

// (f) attachment first page display + interaction ready
const VIEWERS: Record<string, { display: { selector: string; canvas?: boolean; attr?: [string, string] }; paintable: boolean; ready: string }> = {
  pdf: { display: { selector: "[data-pdf-viewer] canvas", canvas: true }, paintable: false, ready: "[data-pdf-viewer] .attachment-viewer__page-label" },
  docx: {
    display: { selector: "[data-docx-viewer]", attr: ["data-docx-state", "ready"] },
    paintable: false,
    ready: '[data-docx-viewer][data-docx-state="ready"] iframe',
  },
  xlsx: { display: { selector: '[data-testid="xlsx-viewer"] td' }, paintable: true, ready: '[data-testid="xlsx-viewer"] table' },
  pptx: { display: { selector: "[data-pptx-viewer] img.pptx-viewer__slide" }, paintable: true, ready: '[data-pptx-viewer][data-pptx-slide-state="ready"]' },
  hwp: { display: { selector: "[data-hwp-viewer] img.hwp-viewer__page" }, paintable: true, ready: "[data-hwp-viewer] .attachment-viewer__page-label" },
};

test("f: attachment viewers", async ({ browser }) => {
  test.setTimeout(1_800_000);
  const samples: Record<string, unknown>[] = [];
  const open = async (page: Page, kind: string, mode: string) => {
    const v = VIEWERS[kind]!;
    const id = `f-${kind}-${samples.length}`;
    await page.goto("about:blank");
    await page.goto(`/w/${owner.workspaceSlug}/a/${ctx.attachments[kind]!.id}/view`, { waitUntil: "commit" });
    await watch(page, id, v.display);
    const hit = await waitHit(page, id, 60_000);
    const paint = hit && v.paintable ? await elementPaint(page, id, 2000) : null;
    const ready = await page
      .locator(v.ready)
      .first()
      .waitFor({ timeout: 60_000 })
      .then(() => pageNow(page))
      .catch(() => null);
    const res = await resourcesSince(page, 0);
    const file = res.filter((r) => /attachments\/:id\/(content|download|raw|file|view)|\/storage\/|\/uploads?\//.test(r.name));
    const api = res.filter((r) => r.name.includes("/api/v1/"));
    const lastApiEnd = api.length ? Math.max(...api.map((r) => r.end)) : null;
    const fcp = (await paints(page)).find((p) => p.n === "first-contentful-paint")?.s ?? null;
    samples.push({
      kind,
      mode,
      fcp: round(fcp),
      displayDom: hit && !hit.pre ? round(hit.dom) : null,
      displayNextFrame: hit && !hit.pre && hit.raf ? round(hit.raf) : null,
      displayPaint: round(paint),
      readyObserved: round(ready),
      apiLastEnd: round(lastApiEnd),
      fileRequests: file.map((r) => ({ name: r.name, ms: round(r.end - r.start), bytes: r.bytes })),
      apiBytes: api.reduce((a, r) => a + r.bytes, 0),
      jsBytes: res.filter((r) => r.name.endsWith(".js") || r.name.endsWith(".mjs") || r.name.endsWith(":file")).reduce((a, r) => a + r.bytes, 0),
      wasmOrWorker: res.filter((r) => /\.wasm$|worker/i.test(r.name)).map((r) => ({ name: r.name, ms: round(r.end - r.start), bytes: r.bytes })),
      longTasksBeforeDisplay: hit ? await longTasks(page, 0, hit.dom) : null,
      ...(WATERFALL ? { waterfall: waterfall(res) } : {}),
    });
  };
  for (const kind of Object.keys(VIEWERS)) {
    await quietWindow(`f-${kind}-cold`, loadLog);
    for (let i = 0; i < N; i += 1) {
      const context = await newProbedContext(browser, ctx.ownerState);
      const page = await context.newPage();
      await guarded(samples, { kind, mode: "cold-context" }, () => open(page, kind, "cold-context"));
      await context.close();
    }
    await quietWindow(`f-${kind}-warm`, loadLog);
    const context = await newProbedContext(browser, ctx.ownerState);
    const page = await context.newPage();
    await open(page, kind, "warm-reload");
    samples.pop();
    for (let i = 0; i < N; i += 1) await guarded(samples, { kind, mode: "warm-reload" }, () => open(page, kind, "warm-reload"));
    await context.close();
    flush();
  }
  results.f = samples;
  for (const kind of Object.keys(VIEWERS)) {
    for (const mode of ["cold-context", "warm-reload"]) {
      const s = samples.filter((x) => x.kind === kind && x.mode === mode);
      const pick = (m: string) => s.map((x) => x[m] as number | null);
      const flow = `f.${kind}.${mode}`;
      record(flow, "displayDom", VIEWERS[kind]!.display.canvas ? "canvas pixels observed (not paint)" : "DOM-observed", pick("displayDom"));
      record(flow, "displayNextFrame", "next animation frame after display (not paint)", pick("displayNextFrame"));
      if (VIEWERS[kind]!.paintable) record(flow, "displayPaint", "paint (Element Timing, image/text)", pick("displayPaint"));
      record(flow, "readyObserved", "controls present (runner poll, DOM)", pick("readyObserved"));
      record(flow, "fcp", "paint (Paint Timing, app shell)", pick("fcp"));
    }
  }
  flush();
});

// (g) first-ever open of freshly API-seeded documents (one cold open per document).
// Added after an intermittent multi-second delay between collab `connected` and
// the first text on the first open of the seeded small document in (b).
test("g: first open of freshly seeded documents", async ({ browser }) => {
  test.setTimeout(1_800_000);
  const docs = ctx.freshDocs;
  await quietWindow("g-first-open", loadLog);
  const samples: Record<string, unknown>[] = [];
  for (const [i, doc] of docs.entries()) {
    const context = await newProbedContext(browser, ctx.ownerState);
    const page = await context.newPage();
    const sockets: { opened: number; closed: number | null; sent: number; recv: number }[] = [];
    const consoleCounts: Record<string, number> = {};
    page.on("console", (m) => {
      consoleCounts[m.type()] = (consoleCounts[m.type()] ?? 0) + 1;
    });
    page.on("websocket", (ws) => {
      const rec = { opened: nodeNow(), closed: null as number | null, sent: 0, recv: 0 };
      sockets.push(rec);
      ws.on("framesent", () => (rec.sent += 1));
      ws.on("framereceived", () => (rec.recv += 1));
      ws.on("close", () => (rec.closed = nodeNow()));
    });
    await guarded(samples, { i }, async () => {
      const t0 = nodeNow();
      await page.goto(`/w/${owner.workspaceSlug}/${doc.displayId}`, { waitUntil: "commit" });
      await watch(page, `g-text-${i}`, { selector: ".fvoci-editor .ProseMirror p", text: doc.marker });
      await watch(page, `g-conn-${i}`, { selector: '[data-collab-status="connected"]' });
      const text = await waitHit(page, `g-text-${i}`, 60_000);
      const conn = await waitHit(page, `g-conn-${i}`, 60_000);
      samples.push({
        i,
        textDom: text && !text.pre ? round(text.dom) : null,
        collabConnectedDom: conn && !conn.pre ? round(conn.dom) : null,
        connectedToText: text && conn ? round(text.dom - conn.dom) : null,
        sockets: sockets.map((s) => ({
          openedAfterMs: round(s.opened - t0),
          closedAfterMs: s.closed === null ? null : round(s.closed - t0),
          sent: s.sent,
          recv: s.recv,
        })),
        console: consoleCounts,
      });
    });
    await context.close();
  }
  results.g = samples;
  const pick = (m: string) => samples.map((x) => x[m] as number | null);
  record("g.firstOpen", "textDom", "DOM-observed", pick("textDom"));
  record("g.firstOpen", "collabConnectedDom", "DOM-observed (collab ack state)", pick("collabConnectedDom"));
  record("g.firstOpen", "connectedToText", "DOM-observed interval", pick("connectedToText"));
  flush();
});
