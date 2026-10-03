import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type CDPSession, type Page, type TestInfo } from "@playwright/test";
import { z } from "zod";
import { login } from "./helpers";

// C6. The workspace shell used to hold one persistent task stream per project
// plus the access stream; on plain HTTP/1.1 six of them exhausted the host's
// socket pool and an ordinary GET stalled without reaching the server
// (characterized at fbc2dd5: 6 sockets, 8 s stall, arrival after reclaiming
// one). The shell now holds one workspace task stream plus the access stream
// at any project count. The RED condition stays as a control: the fixture
// itself opens per-project streams (a route kept for existing clients) up to
// six sockets and the same probe must stall again until one closes. Fixture
// instrumentation only: an init-script EventSource registry and CDP capture.
// No retry or timeout change; workers 1, retries 0.
const owner = { email: "Admin@Example.COM", password: "supersecret1" };
const workspacesSchema = z.object({
  items: z.array(z.object({ id: z.string().uuid(), kind: z.string(), slug: z.string() })),
});
const projectsSchema = z.object({ items: z.array(z.object({ id: z.string().uuid() })) });
const PROBE = "/api/v1/me/workspaces";
// Observation window for a stall: longer than ordinary local GET latency,
// shorter than the pool reopen backoff ceiling. Not a retry or a fix.
const STALL_WINDOW_MS = 8_000;

type NetEvent = { at: number; kind: string; url: string; requestId: string; detail?: unknown };

async function setup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if (!new URL(page.url()).pathname.endsWith("/setup")) {
    await login(page, owner.email, owner.password);
    return;
  }
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("용량");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Tracer team");
  await page.getByLabel("주소(영문)").fill("tracer");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}
/** Make the team hold exactly `count` live projects (creates only; never deletes). */
async function projects(page: Page, count: number): Promise<string> {
  const team = workspacesSchema
    .parse(await (await page.request.get("/api/v1/me/workspaces")).json())
    .items.find((workspace) => workspace.kind === "team" && workspace.slug === "tracer");
  if (!team) throw new Error("tracer team workspace missing");
  const existing = projectsSchema.parse(
    await (await page.request.get(`/api/v1/workspaces/${team.id}/projects`)).json(),
  ).items.length;
  if (existing > count) throw new Error(`fixture order: ${String(existing)} > ${String(count)}`);
  for (let index = existing; index < count; index++) {
    const response = await page.request.post(`/api/v1/workspaces/${team.id}/projects`, {
      data: { key: `CAP${String(index)}`, name: `용량 ${String(index)}`, visibility: "workspace" },
    });
    expect(response.status()).toBe(201);
  }
  return team.id;
}
/** Fixture-only registry so the test can close exactly one pooled stream. */
async function instrument(page: Page): Promise<{ cdp: CDPSession; events: NetEvent[] }> {
  await page.addInitScript(() => {
    const Original = window.EventSource;
    const registry: EventSource[] = [];
    (window as unknown as { __fvociSources: EventSource[] }).__fvociSources = registry;
    window.EventSource = class extends Original {
      constructor(url: string | URL, init?: EventSourceInit) {
        super(url, init);
        registry.push(this);
      }
    };
  });
  const cdp = await page.context().newCDPSession(page);
  const events: NetEvent[] = [];
  const urls = new Map<string, string>();
  await cdp.send("Network.enable");
  cdp.on("Network.requestWillBeSent", (event) => {
    urls.set(event.requestId, event.request.url);
    events.push({
      at: Date.now(),
      kind: "request",
      url: event.request.url,
      requestId: event.requestId,
    });
  });
  cdp.on("Network.responseReceived", (event) => {
    events.push({
      at: Date.now(),
      kind: "response",
      url: event.response.url,
      requestId: event.requestId,
      detail: {
        status: event.response.status,
        connectionId: event.response.connectionId,
        protocol: event.response.protocol,
        timing: event.response.timing,
        date: event.response.headers.date ?? event.response.headers.Date,
      },
    });
  });
  cdp.on("Network.loadingFinished", (event) => {
    events.push({
      at: Date.now(),
      kind: "finished",
      url: urls.get(event.requestId) ?? "",
      requestId: event.requestId,
    });
  });
  return { cdp, events };
}
async function startProbe(page: Page): Promise<void> {
  await page.evaluate((probe) => {
    (window as unknown as { __probe: Promise<number> }).__probe = fetch(probe, {
      cache: "no-store",
      credentials: "same-origin",
    }).then(
      (response) => response.status,
      () => -1,
    );
  }, PROBE);
}
async function probeWithin(page: Page, ms: number): Promise<number | "pending"> {
  return page.evaluate(
    async (window_ms) =>
      Promise.race([
        (window as unknown as { __probe: Promise<number> }).__probe,
        new Promise<"pending">((resolve) =>
          setTimeout(() => {
            resolve("pending");
          }, window_ms),
        ),
      ]),
    ms,
  );
}
// Terminal colour codes in the retained server log (ESC [ ... m).
const ANSI = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g");
/** Server request-span starts for the probe route, as epoch ms (route template only). */
function probeArrivals(): number[] {
  const serverLog = process.env.SERVER_LOG ? readFileSync(process.env.SERVER_LOG, "utf8") : "";
  return serverLog
    .split("\n")
    .map((line) => line.replace(ANSI, ""))
    .filter((line) => line.includes(`route=${PROBE}`) && line.includes("on_request"))
    .map((line) => Date.parse(line.slice(0, line.indexOf(" "))))
    .filter((at) => !Number.isNaN(at));
}
async function attach(
  testInfo: TestInfo,
  name: string,
  events: NetEvent[],
  extra: Record<string, unknown>,
) {
  const serverLog = process.env.SERVER_LOG ? readFileSync(process.env.SERVER_LOG, "utf8") : "";
  const arrivals = serverLog
    .split("\n")
    .filter((line) => line.includes("/api/v1/me/workspaces") || line.includes("/stream"));
  const body = JSON.stringify({ test: testInfo.title, events, arrivals, ...extra }, null, 1);
  // Playwright attachments survive only failed groups; a wrapper-provided
  // directory keeps passing evidence too (never inside the source tree).
  const evidence = process.env.W2_C6_EVIDENCE_DIR;
  if (evidence) {
    mkdirSync(evidence, { recursive: true });
    writeFileSync(join(evidence, `${name}.json`), body);
  }
  await testInfo.attach(name, {
    body,
    contentType: "application/json",
  });
}
/** Every observed stream and probe response used HTTP/1.1 (the hypothesis' transport). */
function expectHttp1(events: NetEvent[], probeId?: string) {
  const responses = events.filter(
    (event) =>
      event.kind === "response" &&
      (event.url.endsWith("/stream") ||
        event.url.endsWith("/task-stream") ||
        event.url.endsWith("/access-stream") ||
        event.requestId === probeId),
  );
  expect(responses.length).toBeGreaterThan(0);
  for (const response of responses)
    expect((response.detail as { protocol?: string } | undefined)?.protocol).toBe("http/1.1");
}
async function shell(page: Page): Promise<void> {
  await page.goto("/w/tracer");
  await expect(page.getByRole("navigation").first()).toBeVisible();
}
async function sourceUrls(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    (window as unknown as { __fvociSources: EventSource[] }).__fvociSources
      .filter((source) => source.readyState === EventSource.OPEN)
      .map((source) => new URL(source.url).pathname),
  );
}
/** Starts the probe and returns its CDP requestId and issue time. */
async function correlatedProbe(
  page: Page,
  events: NetEvent[],
): Promise<{ probeId: string | undefined; probeIssuedAt: number }> {
  // The shell itself also GETs the probe URL while loading: correlate the
  // probe by its own CDP requestId, first seen after the probe starts.
  const beforeProbe = events.length;
  await startProbe(page);
  await expect
    .poll(() =>
      events
        .slice(beforeProbe)
        .some((event) => event.kind === "request" && event.url.endsWith(PROBE)),
    )
    .toBe(true);
  const probeId = events
    .slice(beforeProbe)
    .find((event) => event.kind === "request" && event.url.endsWith(PROBE))?.requestId;
  const probeIssuedAt =
    events.slice(beforeProbe).find((event) => event.requestId === probeId)?.at ?? 0;
  return { probeId, probeIssuedAt };
}
const PROJECTS = 8;

test.describe.configure({ mode: "serial" });

test("N=8 projects: the shell holds one workspace task stream plus the access stream and an ordinary GET reaches the server", async ({
  page,
}, testInfo) => {
  test.setTimeout(60_000);
  await setup(page);
  const workspaceId = await projects(page, PROJECTS);
  const { events } = await instrument(page);
  await shell(page);
  await expect
    .poll(async () => (await sourceUrls(page)).sort())
    .toEqual(
      [
        `/api/v1/workspaces/${workspaceId}/access-stream`,
        `/api/v1/workspaces/${workspaceId}/task-stream`,
      ].sort(),
    );
  const { probeId, probeIssuedAt } = await correlatedProbe(page, events);
  const result = await probeWithin(page, STALL_WINDOW_MS);
  const arrivals = probeArrivals().filter((at) => at >= probeIssuedAt);
  const urls = await sourceUrls(page);
  const created = events.filter(
    (event) => event.kind === "request" && /\/(task-stream|access-stream|stream)$/.test(event.url),
  ).length;
  await attach(testInfo, "fixed-n8", events, { result, probeId, arrivals, urls, created });
  expect(probeId).toBeTruthy();
  expect(result).toBe(200);
  expect(arrivals).toHaveLength(1);
  expect(urls.filter((url) => url.includes("/projects/"))).toEqual([]);
  expectHttp1(events, probeId);
});

test("control: six persistent streams opened by the fixture still stall an ordinary GET until one closes", async ({
  page,
}, testInfo) => {
  test.setTimeout(90_000);
  await setup(page);
  const workspaceId = await projects(page, PROJECTS);
  const projectIds = projectsSchema
    .parse(await (await page.request.get(`/api/v1/workspaces/${workspaceId}/projects`)).json())
    .items.map((project) => project.id);
  const { events } = await instrument(page);
  await shell(page);
  await expect.poll(async () => (await sourceUrls(page)).length).toBe(2);
  // Fixture: four per-project streams on top of the shell's two = six sockets.
  await page.evaluate(
    ({ workspace, ids }) => {
      for (const id of ids)
        new EventSource(`/api/v1/workspaces/${workspace}/projects/${id}/stream`, {
          withCredentials: true,
        });
    },
    { workspace: workspaceId, ids: projectIds.slice(0, 4) },
  );
  await expect.poll(async () => (await sourceUrls(page)).length).toBe(6);
  const { probeId, probeIssuedAt } = await correlatedProbe(page, events);
  const stalled = await probeWithin(page, STALL_WINDOW_MS);
  const arrivalsDuringStall = probeArrivals().filter((at) => at >= probeIssuedAt);
  const responsesBefore = events.filter(
    (event) => event.kind === "response" && event.requestId === probeId,
  ).length;
  const reclaimedAt = Date.now();
  const closed = await page.evaluate(() => {
    const source = (window as unknown as { __fvociSources: EventSource[] }).__fvociSources.find(
      (candidate) =>
        candidate.readyState === EventSource.OPEN && candidate.url.includes("/projects/"),
    );
    source?.close();
    return source?.url ?? null;
  });
  const recovered = await probeWithin(page, STALL_WINDOW_MS);
  const probeResponse = events.find(
    (event) => event.kind === "response" && event.requestId === probeId,
  );
  const arrivalsAfterReclaim = probeArrivals().filter((at) => at >= reclaimedAt);
  await attach(testInfo, "red-control", events, {
    probeId,
    probeIssuedAt,
    arrivalsDuringStall,
    arrivalsAfterReclaim,
    stalled,
    responsesBefore,
    reclaimedAt,
    closed,
    recovered,
    probeResponse,
  });
  expect(probeId).toBeTruthy();
  expectHttp1(events, probeId);
  expect(stalled).toBe("pending");
  expect(responsesBefore).toBe(0);
  expect(arrivalsDuringStall).toEqual([]);
  expect(arrivalsAfterReclaim).toHaveLength(1);
  expect(closed).not.toBeNull();
  expect(recovered).toBe(200);
  expect(probeResponse?.at ?? 0).toBeGreaterThanOrEqual(reclaimedAt);
});
