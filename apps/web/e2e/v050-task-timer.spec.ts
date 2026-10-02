import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Route } from "@playwright/test";
import { z } from "zod";
import { createE2eUser, login } from "./helpers";
import { isoToDatetimeLocalInTimeZone } from "../src/lib/datetime";

const credentials = { email: "timer@example.com", password: "supersecret1" };
const identityShape = z.object({ userId: z.string(), sessionId: z.string() });
const workspaces = z.object({ items: z.array(z.object({ id: z.string(), slug: z.string() })) });
const taskShape = z.object({ id: z.string(), number: z.number(), statusId: z.string() });
const timerShape = z.object({
  run: z
    .object({
      id: z.string(),
      status: z.enum(["running", "paused", "stopped"]),
      version: z.number(),
      runningSince: z.string().nullable(),
      elapsedMilliseconds: z.number(),
    })
    .nullable(),
  actualMilliseconds: z.number(),
  canControl: z.boolean(),
});

// Test-only invoker witness inside this group's isolated database. No product
// policy is modified and no credential or task content enters diagnostics.
function diagnosticDatabase() {
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  const admin = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  const app = process.env.DATABASE_APP_URL;
  if (!container || !admin || !app) throw new Error("isolated timer diagnostic database missing");
  return { container, database: new URL(admin).pathname.slice(1), role: new URL(app).username };
}
function diagnosticSql(sql: string): string {
  const { container, database } = diagnosticDatabase();
  return execFileSync(
    "docker",
    [
      "exec",
      "-i",
      container,
      "psql",
      "-U",
      "postgres",
      "-d",
      database,
      "-v",
      "ON_ERROR_STOP=1",
      "-qAt",
    ],
    { input: sql, encoding: "utf8", stdio: ["pipe", "pipe", "pipe"] },
  ).trim();
}
const witnessRows = z.array(
  z.object({
    pid: z.number(),
    role: z.string(),
    actor: z.string(),
    tenant: z.string().nullable(),
    system: z.string().nullable(),
    tables: z.array(
      z.object({
        name: z.string(),
        superuser: z.boolean(),
        bypass: z.boolean(),
        nonowner: z.boolean(),
        force: z.boolean(),
        active: z.boolean(),
      }),
    ),
  }),
);
test.beforeAll(() => {
  const { role } = diagnosticDatabase();
  if (!/^[a-zA-Z0-9_]+$/.test(role)) throw new Error("unexpected isolated role identifier");
  diagnosticSql(`
    CREATE TABLE IF NOT EXISTS public.w5_timer_runtime_proof (id uuid PRIMARY KEY, value jsonb NOT NULL);
    GRANT INSERT ON public.w5_timer_runtime_proof TO "${role}";
    CREATE OR REPLACE FUNCTION public.w5_timer_runtime_witness() RETURNS trigger
      LANGUAGE plpgsql SECURITY INVOKER SET search_path='' AS $$
    BEGIN
      INSERT INTO public.w5_timer_runtime_proof(id,value)
      SELECT NEW.id,jsonb_build_object('pid',pg_backend_pid(),'role',current_user,'actor',public.app_self_user_id(),
        'tenant',nullif(current_setting('app.tenant_id',true),''),'system',nullif(current_setting('app.system_ctx',true),''),
        'tables',(SELECT jsonb_agg(jsonb_build_object('name',c.relname,'superuser',r.rolsuper,'bypass',r.rolbypassrls,
          'nonowner',c.relowner<>r.oid,'force',c.relforcerowsecurity,'active',row_security_active(c.oid)) ORDER BY c.relname)
          FROM pg_roles r CROSS JOIN pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
          WHERE r.rolname=current_user AND n.nspname='fvoci' AND c.relname IN
          ('time_entries','task_timer_runs','task_timer_segments','task_timer_legacy_open','task_timer_commands','task_timer_audit')));
      RETURN NEW;
    END; $$;
    DROP TRIGGER IF EXISTS w5_timer_runtime_witness ON fvoci.task_timer_audit;
    CREATE TRIGGER w5_timer_runtime_witness AFTER INSERT ON fvoci.task_timer_audit
      FOR EACH ROW EXECUTE FUNCTION public.w5_timer_runtime_witness();
  `);
});
test.afterEach(async ({ page }, testInfo) => {
  const rows = witnessRows.parse(
    JSON.parse(
      diagnosticSql(
        "SELECT COALESCE(jsonb_agg(value ORDER BY id),'[]'::jsonb) FROM public.w5_timer_runtime_proof",
      ),
    ),
  );
  const serverBin = process.env.FVOCI_E2E_SERVER_BIN;
  if (!serverBin) throw new Error("own server binary missing");
  const configuredBin = realpathSync(serverBin);
  const binaryHash = createHash("sha256").update(readFileSync(configuredBin)).digest("hex");
  const native = [];
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      // main.rs deliberately makes the server nondumpable. Bind the readable
      // launch identity to its supervised parent and actual listening origin;
      // /proc/exe identity remains unavailable, rather than disabling hardening.
      const argv = readFileSync(`/proc/${entry}/cmdline`, "utf8").split("\0");
      if (argv[0] !== configuredBin) continue;
      const stat = readFileSync(`/proc/${entry}/stat`, "utf8");
      const parentPid = stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1];
      if (!parentPid || !/^\d+$/.test(parentPid)) continue;
      const parentArgv = readFileSync(`/proc/${parentPid}/cmdline`, "utf8").split("\0");
      const launcher = path.resolve("../../scripts/web-e2e-inner.sh");
      if (!parentArgv.includes(launcher)) continue;
      const allowed = new Set([
        "RUN_DIR",
        "SERVER_LOG",
        "FVOCI_STATIC_DIR",
        "FVOCI_W5_EVIDENCE_DIR",
      ]);
      const namespace = new Map<string, string>();
      for (const value of readFileSync(`/proc/${parentPid}/environ`, "utf8").split("\0")) {
        const separator = value.indexOf("=");
        const key = value.slice(0, separator);
        if (separator > 0 && allowed.has(key)) namespace.set(key, value.slice(separator + 1));
      }
      if (
        namespace.get("RUN_DIR") !== process.env.FVOCI_E2E_RESULT_DIR ||
        namespace.get("FVOCI_STATIC_DIR") !== process.env.FVOCI_STATIC_DIR ||
        namespace.get("FVOCI_W5_EVIDENCE_DIR") !== process.env.FVOCI_W5_EVIDENCE_DIR
      )
        continue;
      const serverLog = namespace.get("SERVER_LOG");
      if (!serverLog) continue;
      const origin = readFileSync(serverLog, "utf8")
        .split("\n")
        .find((line) => line.includes("fvoci-server listening on "))
        ?.split("fvoci-server listening on ")[1]
        ?.trim();
      if (origin !== new URL(page.url()).origin) continue;
      native.push({
        pid: Number(entry),
        parentPid: Number(parentPid),
        launcher,
        configuredBin,
        knownFileSha256: binaryHash,
        listeningOrigin: origin,
        procExeIdentity: "NOTCAPTURED: intentional nondumpability",
      });
    } catch {
      /* Processes can disappear between directory read and inspection. */
    }
  }
  expect(createHash("sha256").update(readFileSync(configuredBin)).digest("hex")).toBe(binaryHash);
  const scripts = await page
    .locator('script[src*="/assets/"]')
    .evaluateAll((elements) =>
      elements
        .map((element) => element.getAttribute("src"))
        .filter((value): value is string => Boolean(value)),
    );
  const assets = [];
  for (const src of scripts) {
    const pathname = new URL(src, page.url()).pathname;
    const response = await page.request.get(pathname);
    expect(response.ok(), pathname).toBe(true);
    const served = createHash("sha256")
      .update(await response.body())
      .digest("hex");
    const staticDir = process.env.FVOCI_STATIC_DIR;
    if (!staticDir) throw new Error("own static namespace missing");
    const copied = createHash("sha256")
      .update(readFileSync(path.join(staticDir, pathname)))
      .digest("hex");
    expect(served, pathname).toBe(copied);
    assets.push({ path: pathname, servedSha256: served, ownStaticSha256: copied });
  }
  const { container, database, role } = diagnosticDatabase();
  const proof = {
    test: testInfo.title,
    rows,
    native,
    assets,
    serverOrigin: new URL(page.url()).origin,
    container,
    database,
    role,
  };
  const target = testInfo.outputPath("timer-runtime-proof.json");
  writeFileSync(target, JSON.stringify(proof, null, 2));
  await testInfo.attach("timer-runtime-proof", { path: target, contentType: "application/json" });
  const evidenceDir = process.env.FVOCI_W5_EVIDENCE_DIR;
  if (evidenceDir) {
    mkdirSync(evidenceDir, { recursive: true });
    writeFileSync(
      path.join(evidenceDir, `runtime-${testInfo.testId}.json`),
      JSON.stringify(proof, null, 2),
    );
  }
  expect(native).toHaveLength(1);
  expect(assets.length).toBeGreaterThan(0);
  // View-only case may have no write in its fresh worker. The first flow and
  // revocation case must prove the real server invoker's context before denial.
  if (!testInfo.title.startsWith("MyTasks timer controls")) expect(rows.length).toBeGreaterThan(0);
  for (const row of rows) {
    expect(row.role).toBe(role);
    expect(row.system).toBeNull();
    expect(row.tables).toHaveLength(6);
    for (const table of row.tables) {
      expect(table.superuser).toBe(false);
      expect(table.bypass).toBe(false);
      expect(table.nonowner).toBe(true);
      expect(table.force).toBe(true);
      expect(table.active).toBe(true);
    }
  }
});
test.afterAll(() => {
  diagnosticSql(
    "DROP TRIGGER IF EXISTS w5_timer_runtime_witness ON fvoci.task_timer_audit; DROP FUNCTION IF EXISTS public.w5_timer_runtime_witness(); DROP TABLE IF EXISTS public.w5_timer_runtime_proof;",
  );
});

test("detail start commits server intervals; a new MyTasks client pauses, resumes and stops the same task", async ({
  page,
  browser,
}) => {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃" })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("연구");
    await page.getByLabel("이메일").fill(credentials.email);
    await page.getByLabel("비밀번호").fill(credentials.password);
    await page.getByLabel("워크스페이스 이름").fill("읽기 연구 계획");
    await page.getByLabel("주소(영문)").fill("w5timer");
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else await login(page, credentials.email, credentials.password);
  const workspaceResponse = await page.request.get("/api/v1/me/workspaces");
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = workspaces
    .parse(await workspaceResponse.json())
    .items.find((row) => row.slug === "w5timer");
  expect(workspace).toBeTruthy();
  if (!workspace) throw new Error("missing timer fixture workspace");
  const me = z
    .object({ userId: z.string() })
    .parse(await (await page.request.get("/api/v1/auth/me")).json());
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key: "READ", name: "연구 목표와 자료", visibility: "workspace" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  const taskResponse = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${project.id}/tasks`,
    { data: { title: "한글 자료 읽기와 연결된 연구 노트" } },
  );
  expect(taskResponse.status(), await taskResponse.text()).toBe(201);
  const task = taskShape.parse(await taskResponse.json());
  const assigned = await page.request.patch(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`, {
    data: { assigneeIds: [me.userId], estimate: "30" },
  });
  expect(assigned.ok(), await assigned.text()).toBe(true);
  const detailUrl = `/w/w5timer/READ-${String(task.number)}`;
  const apiUrl = `/api/v1/workspaces/${workspace.id}/tasks/${task.id}/timer`;
  await page.goto(detailUrl);
  const detail = page.getByTestId(`task-stopwatch-${task.id}`);
  await expect(detail.getByTestId("timer-start")).toBeEnabled();
  await detail.getByLabel("측정 메모").fill("읽기 구간");
  await detail.getByTestId("timer-start").click();
  await expect(detail.getByTestId("timer-state")).toHaveText("측정 중");
  const committed = timerShape.parse(await (await page.request.get(apiUrl)).json());
  expect(committed.run?.status).toBe("running");
  // Fresh browser context/session, not this page's optimistic state/cache.
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const fresh = await context.newPage();
    await login(fresh, credentials.email, credentials.password);
    await fresh.goto("/w/w5timer/my-tasks");
    const mine = fresh.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mine.getByTestId("timer-state")).toHaveText("측정 중");
    expect(timerShape.parse(await (await fresh.request.get(apiUrl)).json()).run?.id).toBe(
      committed.run?.id,
    );
    await mine.getByTestId("timer-pause").click();
    await expect(mine.getByTestId("timer-state")).toHaveText("일시정지");
    const paused = timerShape.parse(await (await fresh.request.get(apiUrl)).json());
    expect(paused.run?.runningSince).toBeNull();
    await fresh.reload();
    await expect(mine.getByTestId("timer-state")).toHaveText("일시정지");
    const stillPaused = timerShape.parse(await (await fresh.request.get(apiUrl)).json());
    expect(stillPaused.run?.elapsedMilliseconds).toBe(paused.run?.elapsedMilliseconds);
    await mine.getByTestId("timer-resume").click();
    await expect(mine.getByTestId("timer-state")).toHaveText("측정 중");
    await mine.getByTestId("timer-stop").click();
    await expect(mine.getByTestId("timer-start")).toBeEnabled();
    expect(timerShape.parse(await (await fresh.request.get(apiUrl)).json()).run).toBeNull();
    const after = taskShape.parse(
      await (await fresh.request.get(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`)).json(),
    );
    expect(after.statusId).toBe(task.statusId);
    await page.reload();
    await expect(detail.getByTestId("timer-start")).toBeEnabled();
    await expect(detail.getByTestId("timer-actual")).toHaveText(
      await mine.getByTestId("timer-actual").innerText(),
    );
  } finally {
    await context.close();
  }
});

async function timerWorkspace(page: import("@playwright/test").Page): Promise<string> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃" })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("연구");
    await page.getByLabel("이메일").fill(credentials.email);
    await page.getByLabel("비밀번호").fill(credentials.password);
    await page.getByLabel("워크스페이스 이름").fill("읽기 연구 계획");
    await page.getByLabel("주소(영문)").fill("w5timer");
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else await login(page, credentials.email, credentials.password);
  const workspaceResponse = await page.request.get("/api/v1/me/workspaces");
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = workspaces
    .parse(await workspaceResponse.json())
    .items.find((row) => row.slug === "w5timer");
  expect(workspace).toBeTruthy();
  if (!workspace) throw new Error("missing timer fixture workspace");
  return workspace.id;
}

async function permissionFixture(
  page: import("@playwright/test").Page,
  key: string,
  role: "member" | "viewer",
) {
  const workspaceId = await timerWorkspace(page);
  const email = `${key.toLowerCase()}@example.com`;
  createE2eUser(email, credentials.password, "권한 검사", {
    workspaceSlug: "w5timer",
    membershipRole: "member",
  });
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key, name: "권한 회수 검사", visibility: "private" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  return { workspaceId, email, project, role };
}

test("authoritative revoked timer GET retires mounted private state while 503 preserves the server anchor", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await permissionFixture(page, "TACL", "member");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const editor = await context.newPage();
    await login(editor, fixture.email, credentials.password);
    const me = identityShape.parse(await (await editor.request.get("/api/v1/auth/me")).json());
    const memberUrl = `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`;
    const grant = await page.request.post(memberUrl, {
      data: { userId: me.userId, role: fixture.role },
    });
    expect(grant.ok(), await grant.text()).toBe(true);
    const taskResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      { data: { title: "권한 회수 뒤 사적인 측정" } },
    );
    expect(taskResponse.status(), await taskResponse.text()).toBe(201);
    const task = taskShape.parse(await taskResponse.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    const sentinelProjectResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects`,
      { data: { key: "SAFE", name: "계속 볼 수 있는 작업", visibility: "workspace" } },
    );
    expect(sentinelProjectResponse.status(), await sentinelProjectResponse.text()).toBe(201);
    const sentinelProject = z
      .object({ id: z.string() })
      .parse(await sentinelProjectResponse.json());
    const sentinelResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${sentinelProject.id}/tasks`,
      { data: { title: "권한 회수와 무관한 내 작업" } },
    );
    expect(sentinelResponse.status(), await sentinelResponse.text()).toBe(201);
    const sentinelTask = taskShape.parse(await sentinelResponse.json());
    const sentinelAssign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${sentinelTask.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(sentinelAssign.ok(), await sentinelAssign.text()).toBe(true);
    await editor.goto("/w/w5timer/my-tasks");
    const mounted = editor.getByTestId(`task-stopwatch-${task.id}`);
    const sentinel = editor.getByTestId(`task-stopwatch-${sentinelTask.id}`);
    await expect(sentinel.getByTestId("timer-actual")).toBeVisible();
    const sentinelActual = await sentinel.getByTestId("timer-actual").innerText();
    await expect(mounted.getByTestId("timer-start")).toBeEnabled();
    await mounted.getByTestId("timer-start").click();
    await expect(mounted.getByTestId("timer-state")).toHaveText("측정 중");
    const committed = timerShape.parse(await (await editor.request.get(timerUrl)).json());
    expect(committed.run?.id).toBeTruthy();
    const timerMatch = (url: URL) => url.pathname === timerUrl;
    const unavailableHandler = async (route: Route) => {
      if (route.request().method() !== "GET") return route.continue();
      const capture = new URL(route.request().url()).searchParams;
      expect(capture.get("expectedActorId")).toBe(me.userId);
      expect(capture.get("expectedSessionId")).toBe(me.sessionId);
      await route.fulfill({
        status: 503,
        contentType: "application/problem+json",
        body: JSON.stringify({ type: "about:blank", title: "일시적인 연결 실패", status: 503 }),
      });
    };
    await editor.route(timerMatch, unavailableHandler);
    const unavailableWaitStarted = Date.now();
    const unavailable = editor.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 503,
    );
    // Keep the actual consumer foregrounded, then observe its existing 5s
    // polling response before the unchanged UI assertions. The app disables
    // refetchOnWindowFocus, so this does not pretend focus itself refetches.
    await editor.bringToFront();
    await unavailable;
    await testInfo.attach("timer-real-503-precondition", {
      body: JSON.stringify({
        status: 503,
        method: "GET",
        observedWaitMilliseconds: Date.now() - unavailableWaitStarted,
      }),
      contentType: "application/json",
    });
    await expect(mounted.getByRole("alert")).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveText("측정 중");
    await expect(mounted.getByTestId("timer-actual")).toBeVisible();
    const sentinelTimerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${sentinelTask.id}/timer`;
    const sentinelUnavailable = editor.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === sentinelTimerUrl &&
        response.request().method() === "GET" &&
        response.status() === 503,
    );
    await editor.route(
      (url) => url.pathname === sentinelTimerUrl,
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const capture = new URL(route.request().url()).searchParams;
        expect(capture.get("expectedActorId")).toBe(me.userId);
        expect(capture.get("expectedSessionId")).toBe(me.sessionId);
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "다른 작업 연결 실패", status: 503 }),
        });
      },
    );
    await sentinelUnavailable;
    await expect(sentinel.getByTestId("timer-actual")).toHaveText(sentinelActual);
    // Only the unrelated list transport is held to keep the original consumer
    // mounted. The timer denial below comes from real Rust/current project ACL.
    await editor.route(
      (url) => url.pathname === `/api/v1/workspaces/${fixture.workspaceId}/tasks`,
      async (route) => {
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "목록 연결 실패", status: 503 }),
        });
      },
    );
    await editor.unroute(timerMatch, unavailableHandler);
    const denialWaitStarted = Date.now();
    const realDenial = editor.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 404,
    );
    const revoke = await page.request.delete(`${memberUrl}/${me.userId}`);
    expect(revoke.ok(), await revoke.text()).toBe(true);
    const denial = await realDenial;
    await testInfo.attach("timer-real-404-precondition", {
      body: JSON.stringify({
        status: denial.status(),
        method: "GET",
        observedWaitMilliseconds: Date.now() - denialWaitStarted,
      }),
      contentType: "application/json",
    });
    expect((await denial.text()).includes("권한 회수 뒤 사적인 측정")).toBe(false);
    await expect(mounted).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-actual")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-elapsed")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-start")).toBeDisabled();
    await expect(sentinel.getByTestId("timer-actual")).toHaveText(sentinelActual);
    await expect(sentinel.getByTestId("timer-elapsed")).toBeVisible();
    expect((await editor.request.get(sentinelTimerUrl)).status()).toBe(200);
    const actualDenied = await editor.request.get(timerUrl);
    expect(actualDenied.status()).toBe(404);
    const membership = await editor.request.get("/api/v1/me/workspaces");
    expect(
      workspaces.parse(await membership.json()).items.some((row) => row.id === fixture.workspaceId),
    ).toBe(true);
  } finally {
    await context.close();
  }
});

test("MyTasks timer controls follow actual View Edit archive and demotion capability", async ({
  page,
  browser,
}) => {
  const fixture = await permissionFixture(page, "TCAP", "viewer");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const viewer = await context.newPage();
    await login(viewer, fixture.email, credentials.password);
    const me = z
      .object({ userId: z.string() })
      .parse(await (await viewer.request.get("/api/v1/auth/me")).json());
    const membersUrl = `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`;
    const grant = await page.request.post(membersUrl, {
      data: { userId: me.userId, role: "viewer" },
    });
    expect(grant.ok(), await grant.text()).toBe(true);
    const taskResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      { data: { title: "읽기 권한으로 보는 내 작업" } },
    );
    expect(taskResponse.status(), await taskResponse.text()).toBe(201);
    const task = taskShape.parse(await taskResponse.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const entriesUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/time-entries`;
    const capability = async () =>
      z
        .object({ canCreate: z.boolean() })
        .parse(await (await viewer.request.get(entriesUrl)).json()).canCreate;
    expect(await capability()).toBe(false);
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    const initialControl = viewer.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 200 &&
        !z.object({ canControl: z.boolean() }).parse(await response.json()).canControl,
    );
    await viewer.goto("/w/w5timer/my-tasks");
    await initialControl;
    const timer = viewer.getByTestId(`task-stopwatch-${task.id}`);
    await expect(timer).toBeVisible();
    await expect(timer.getByTestId("timer-actual")).toBeVisible();
    await expect(timer.getByTestId("timer-elapsed")).toBeVisible();
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const nextControl = (expected: boolean) =>
      viewer.waitForResponse(async (response) => {
        if (
          new URL(response.url()).pathname !== timerUrl ||
          response.request().method() !== "GET" ||
          response.status() !== 200
        )
          return false;
        return (
          z.object({ canControl: z.boolean() }).parse(await response.json()).canControl === expected
        );
      });
    const promotedControl = nextControl(true);
    const promote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(promote.ok(), await promote.text()).toBe(true);
    expect(await capability()).toBe(true);
    await promotedControl;
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const demotedControl = nextControl(false);
    const demote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "viewer" },
    });
    expect(demote.ok(), await demote.text()).toBe(true);
    expect(await capability()).toBe(false);
    await demotedControl;
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const restoredControl = nextControl(true);
    const again = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(again.ok(), await again.text()).toBe(true);
    await restoredControl;
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const archivedControl = nextControl(false);
    const archive = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/archive`,
    );
    expect(archive.ok(), await archive.text()).toBe(true);
    expect(await capability()).toBe(false);
    await archivedControl;
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
  } finally {
    await context.close();
  }
});

test("captured timer GET cannot cache another actor run under a stale mounted identity", async ({
  page,
  browser,
}, testInfo) => {
  await timerWorkspace(page);
  // Keep this identity boundary independent of earlier tests' project streams.
  // The real MyTasks consumer still installs its normal workspace/project SSEs.
  const slug = "w5timer-cookie";
  const workspaceResponse = await page.request.post("/api/v1/workspaces", {
    data: { name: "측정 작성자 경계", slug },
  });
  expect(workspaceResponse.status(), await workspaceResponse.text()).toBe(201);
  const workspaceId = z.object({ id: z.string() }).parse(await workspaceResponse.json()).id;
  const email = "cookie@example.com";
  createE2eUser(email, credentials.password, "작성자 경계", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "COOKIE", name: "작성자 경계", visibility: "private" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const fixture = {
    workspaceId,
    email,
    project: z.object({ id: z.string() }).parse(await projectResponse.json()),
  };
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let releaseOwner = () => {};
  let releaseMe = () => {};
  try {
    const other = await context.newPage();
    await login(other, fixture.email, credentials.password);
    const identity = identityShape;
    const actorA = identity.parse(await (await page.request.get("/api/v1/auth/me")).json());
    const actorB = identity.parse(await (await other.request.get("/api/v1/auth/me")).json());
    expect(actorB.userId).not.toBe(actorA.userId);
    const grant = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`,
      {
        data: { userId: actorB.userId, role: "member" },
      },
    );
    expect(grant.ok(), await grant.text()).toBe(true);
    const created = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      {
        data: { title: "같은 작업에서 분리된 개인 측정" },
      },
    );
    expect(created.status(), await created.text()).toBe(201);
    const task = taskShape.parse(await created.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      {
        data: { assigneeIds: [actorA.userId] },
      },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    const actorARun = await startPausedTimer(page, timerUrl, actorA, "이전 작성자의 실제 구간");
    const start = await other.request.post(timerUrl, {
      data: {
        expectedActorId: actorB.userId,
        expectedSessionId: actorB.sessionId,
        requestId: crypto.randomUUID(),
        operation: "start",
        expectedVersion: 0,
        runId: null,
        note: "다른 작성자의 사적인 구간",
      },
    });
    expect(start.ok(), await start.text()).toBe(true);
    const run = z.object({ runId: z.string(), version: z.number() }).parse(await start.json());
    const pause = await other.request.post(timerUrl, {
      data: {
        expectedActorId: actorB.userId,
        expectedSessionId: actorB.sessionId,
        requestId: crypto.randomUUID(),
        operation: "pause",
        expectedVersion: run.version,
        runId: run.runId,
      },
    });
    expect(pause.ok(), await pause.text()).toBe(true);
    await page.goto(`/w/${slug}/my-tasks`);
    const mounted = page.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mounted.getByTestId("timer-actual")).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
    await expect(mounted.getByTestId("timer-elapsed")).toBeVisible();
    await expect(mounted.getByTestId("timer-resume")).toBeEnabled();
    await expect(page.getByTestId("timer-owner")).toBeVisible();
    const actorRead = timerShape.parse(await (await page.request.get(timerUrl)).json());
    expect(actorRead.run?.id).toBe(actorARun.runId);
    const meDelivery = new Promise<void>((resolve) => {
      releaseMe = resolve;
    });
    await page.route(
      (url) => url.pathname === "/api/v1/auth/me",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const native = await route.fetch();
        // Deliver the unchanged real identity only after both old consumers have
        // consumed their denials. Product identity/navigation guards remain live.
        await meDelivery;
        if (!page.isClosed()) await route.fulfill({ response: native });
      },
    );
    const ownerDelivery = new Promise<void>((resolve) => {
      releaseOwner = resolve;
    });
    type OwnerObservation = {
      status: number;
      capturedActorParameter: string | null;
      capturedSessionParameter: string | null;
      returnedOtherRun: boolean;
    };
    let observeOwner: (value: OwnerObservation) => void = () => {};
    const ownerRead = new Promise<OwnerObservation>((resolve) => {
      observeOwner = resolve;
    });
    await page.route(
      (url) => url.pathname === "/api/v1/me/task-timer",
      async (route) => {
        // Fetch the actual Rust response with the browser's original headers.
        // Hold only its delivery so me refresh cannot cancel the task oracle.
        if (route.request().method() !== "GET") return route.continue();
        const url = new URL(route.request().url());
        if (url.searchParams.get("expectedSessionId") !== actorA.sessionId) return route.continue();
        const native = await route.fetch();
        const value = z
          .object({ runId: z.string().nullable().optional() })
          .parse(await native.json());
        observeOwner({
          status: native.status(),
          capturedActorParameter: url.searchParams.get("expectedActorId"),
          capturedSessionParameter: url.searchParams.get("expectedSessionId"),
          returnedOtherRun: value.runId === run.runId,
        });
        await ownerDelivery;
        if (!page.isClosed()) await route.fulfill({ response: native });
      },
    );
    await page.route(
      (url) => url.pathname === timerUrl,
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        if (
          new URL(route.request().url()).searchParams.get("expectedSessionId") !== actorA.sessionId
        )
          return route.continue();
        const native = await route.fetch();
        // Both ordinary polls must reach Rust before either denial can refresh
        // me and cancel the other captured scope. No query is forced or forged.
        await ownerRead;
        if (!page.isClosed()) await route.fulfill({ response: native });
      },
    );
    const switchedResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === timerUrl && response.request().method() === "GET",
    );
    // A real, independently logged-in credential replaces the cookie while
    // the actual mounted me/query capture still belongs to actor A.
    await page.context().addCookies(await context.cookies());
    await page.bringToFront();
    const response = await switchedResponse;
    const ownerObservation = await ownerRead;
    const body: unknown = await response.json();
    const observed = z
      .object({ run: z.object({ id: z.string() }).nullable().optional() })
      .passthrough()
      .parse(body);
    const authenticated = identity.parse(await (await page.request.get("/api/v1/auth/me")).json());
    expect(authenticated.userId).toBe(actorB.userId);
    if (response.ok() && observed.run?.id === run.runId) {
      // Record the actual mounted consumer's effect before the guard oracle.
      await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
    }
    // The native denial must arrive while the nonempty old consumer remains
    // mounted. Detachment/navigation cannot stand in for privacy retirement.
    expect(response.status()).toBe(409);
    await expect(mounted).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-actual")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-elapsed")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-start")).toBeDisabled();
    await expect(page.getByTestId("timer-owner")).toBeVisible();
    const ownerDelivered = page.waitForResponse(
      (reply) =>
        new URL(reply.url()).pathname === "/api/v1/me/task-timer" &&
        new URL(reply.url()).searchParams.get("expectedSessionId") === actorA.sessionId &&
        reply.status() === 409,
    );
    releaseOwner();
    await ownerDelivered;
    await expect(page.getByTestId("timer-owner")).toHaveCount(0);
    await expect(mounted).toBeVisible();
    const successorOwner = page.waitForResponse(
      (reply) =>
        new URL(reply.url()).pathname === "/api/v1/me/task-timer" &&
        new URL(reply.url()).searchParams.get("expectedActorId") === actorB.userId &&
        new URL(reply.url()).searchParams.get("expectedSessionId") === actorB.sessionId &&
        reply.status() === 200,
    );
    releaseMe();
    const successor = await successorOwner;
    expect(z.object({ runId: z.string().nullable() }).parse(await successor.json()).runId).toBe(
      run.runId,
    );
    await expect(page.getByTestId("timer-owner")).toBeVisible();
    await testInfo.attach("timer-cookie-read-guard", {
      body: JSON.stringify({
        status: response.status(),
        capturedActorParameter: new URL(response.url()).searchParams.get("expectedActorId"),
        capturedSessionParameter: new URL(response.url()).searchParams.get("expectedSessionId"),
        expectedActor: actorA.userId,
        expectedSession: actorA.sessionId,
        authenticatedActor: authenticated.userId,
        returnedOtherRun: observed.run?.id === run.runId,
        oldTaskRetirementObservedBeforeIdentityDelivery: true,
        oldOwnerRetirementObservedBeforeIdentityDelivery: true,
        successorAuthorizedOwnerRun: run.runId,
        ownerObservation,
      }),
      contentType: "application/json",
    });
    expect(
      response.status(),
      "stale scoped query must be rejected before any other actor state",
    ).toBe(409);
    expect(new URL(response.url()).searchParams.get("expectedActorId")).toBe(actorA.userId);
    expect(new URL(response.url()).searchParams.get("expectedSessionId")).toBe(actorA.sessionId);
    expect(observed.run).toBeUndefined();
    expect(ownerObservation.status).toBe(409);
    expect(ownerObservation.capturedActorParameter).toBe(actorA.userId);
    expect(ownerObservation.capturedSessionParameter).toBe(actorA.sessionId);
    expect(ownerObservation.returnedOtherRun).toBe(false);
  } finally {
    releaseOwner();
    releaseMe();
    await context.close();
  }
});

async function startPausedTimer(
  page: import("@playwright/test").Page,
  timerUrl: string,
  actor: z.infer<typeof identityShape>,
  note: string,
) {
  const start = await page.request.post(timerUrl, {
    data: {
      expectedActorId: actor.userId,
      expectedSessionId: actor.sessionId,
      requestId: crypto.randomUUID(),
      operation: "start",
      expectedVersion: 0,
      runId: null,
      note,
    },
  });
  expect(start.ok(), await start.text()).toBe(true);
  const run = z.object({ runId: z.string(), version: z.number() }).parse(await start.json());
  const pause = await page.request.post(timerUrl, {
    data: {
      expectedActorId: actor.userId,
      expectedSessionId: actor.sessionId,
      requestId: crypto.randomUUID(),
      operation: "pause",
      expectedVersion: run.version,
      runId: run.runId,
    },
  });
  expect(pause.ok(), await pause.text()).toBe(true);
  return z.object({ runId: z.string(), version: z.number() }).parse(await pause.json());
}

test("a same-actor new-session denial cannot retire a populated successor or returning scope", async ({
  page,
  browser,
}, testInfo) => {
  await timerWorkspace(page);
  const slug = "w5timer-session";
  const workspaceResponse = await page.request.post("/api/v1/workspaces", {
    data: { name: "세션 전환 측정", slug },
  });
  expect(workspaceResponse.status(), await workspaceResponse.text()).toBe(201);
  const workspaceId = z.object({ id: z.string() }).parse(await workspaceResponse.json()).id;
  const email = "timer-session@example.com";
  createE2eUser(email, credentials.password, "같은 작성자의 세션", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "SESSION", name: "세션 전환 측정", visibility: "private" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  const firstContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  const secondContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let releaseTransitionMe = () => {};
  let transitionMeDelivery = Promise.resolve();
  const events: {
    kind: string;
    endpoint: string;
    session: string | null;
    status?: number;
    failure?: string;
  }[] = [];
  try {
    const first = await firstContext.newPage();
    const second = await secondContext.newPage();
    await login(first, email, credentials.password);
    await login(second, email, credentials.password);
    const s1 = identityShape.parse(await (await first.request.get("/api/v1/auth/me")).json());
    const s2 = identityShape.parse(await (await second.request.get("/api/v1/auth/me")).json());
    expect(s1.userId).toBe(s2.userId);
    expect(s1.sessionId).not.toBe(s2.sessionId);
    const grant = await page.request.post(
      `/api/v1/workspaces/${workspaceId}/projects/${project.id}/members`,
      {
        data: { userId: s1.userId, role: "member" },
      },
    );
    expect(grant.ok(), await grant.text()).toBe(true);
    const created = await page.request.post(
      `/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`,
      {
        data: { title: "새 세션에도 남아야 하는 개인 구간" },
      },
    );
    expect(created.status(), await created.text()).toBe(201);
    const task = taskShape.parse(await created.json());
    const assign = await page.request.patch(`/api/v1/workspaces/${workspaceId}/tasks/${task.id}`, {
      data: { assigneeIds: [s1.userId] },
    });
    expect(assign.ok(), await assign.text()).toBe(true);
    const timerUrl = `/api/v1/workspaces/${workspaceId}/tasks/${task.id}/timer`;
    const ownerUrl = "/api/v1/me/task-timer";
    const run = await startPausedTimer(first, timerUrl, s1, "실제 세션 경계 구간");
    const control = await second.request.get(timerUrl, {
      params: {
        expectedActorId: s2.userId,
        expectedSessionId: s2.sessionId,
      },
    });
    expect(control.status(), await control.text()).toBe(200);
    expect(timerShape.parse(await control.json()).run?.id).toBe(run.runId);
    await first.goto(`/w/${slug}/my-tasks`);
    const mounted = first.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
    await expect(mounted.getByTestId("timer-resume")).toBeEnabled();
    await expect(first.getByTestId("timer-owner")).toBeVisible();
    await first.route(
      (url) => url.pathname === "/api/v1/auth/me",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const native = await route.fetch();
        const delivery = transitionMeDelivery;
        await delivery;
        if (!first.isClosed()) await route.fulfill({ response: native });
      },
    );
    const cookies1 = await firstContext.cookies();
    const cookies2 = await secondContext.cookies();
    const endpoint = (url: string) => {
      const parsed = new URL(url);
      return [timerUrl, ownerUrl].includes(parsed.pathname) ? parsed : undefined;
    };
    first.on("requestfailed", (request) => {
      const url = endpoint(request.url());
      if (url && request.method() === "GET")
        events.push({
          kind: "requestfailed",
          endpoint: url.pathname,
          session: url.searchParams.get("expectedSessionId"),
          failure: request.failure()?.errorText,
        });
    });
    first.on("response", (response) => {
      const url = endpoint(response.url());
      if (url && response.request().method() === "GET")
        events.push({
          kind: "response",
          endpoint: url.pathname,
          session: url.searchParams.get("expectedSessionId"),
          status: response.status(),
        });
    });
    // Real cookie changes and the existing poll/me refresh exercise S1 -> S2
    // -> S1 -> S2. No injected query/cache, fake auth, timer body, or reload.
    for (const [previous, successor, cookies] of [
      [s1, s2, cookies2],
      [s2, s1, cookies1],
      [s1, s2, cookies2],
    ] as const) {
      const transitionStart = events.length;
      transitionMeDelivery = new Promise<void>((resolve) => {
        releaseTransitionMe = resolve;
      });
      const denied = first.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === timerUrl &&
          new URL(response.url()).searchParams.get("expectedSessionId") === previous.sessionId &&
          response.status() === 409,
      );
      const refreshed = first.waitForResponse(
        async (response) =>
          new URL(response.url()).pathname === "/api/v1/auth/me" &&
          response.status() === 200 &&
          identityShape.parse(await response.json()).sessionId === successor.sessionId,
      );
      const successorRead = first.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === timerUrl &&
          new URL(response.url()).searchParams.get("expectedSessionId") === successor.sessionId &&
          response.status() === 200,
      );
      const successorOwner = first.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === ownerUrl &&
          new URL(response.url()).searchParams.get("expectedSessionId") === successor.sessionId &&
          response.status() === 200,
      );
      await firstContext.addCookies(cookies);
      await first.bringToFront();
      const oldResult = await denied;
      expect(
        z.object({ params: z.object({ code: z.string() }) }).parse(await oldResult.json()).params
          .code,
      ).toBe("timer_context_changed");
      releaseTransitionMe();
      await refreshed;
      const result = await successorRead;
      expect(timerShape.parse(await result.json()).run?.id).toBe(run.runId);
      expect(
        z.object({ runId: z.string().nullable() }).parse(await (await successorOwner).json()).runId,
      ).toBe(run.runId);
      await expect(mounted).toBeVisible();
      await expect(mounted.getByTestId("timer-state")).toHaveText("일시정지");
      await expect(mounted.getByTestId("timer-resume")).toBeEnabled();
      await expect(first.getByTestId("timer-owner")).toBeVisible();
      expect(
        events
          .slice(transitionStart)
          .filter(
            (event) => event.kind === "requestfailed" && event.session === successor.sessionId,
          ),
        "an old denial must not cancel the new session's authorized timer query",
      ).toEqual([]);
    }
  } finally {
    releaseTransitionMe();
    await testInfo.attach("timer-real-session-boundaries", {
      body: JSON.stringify({ events, productQueryOrAuthInjection: false }),
      contentType: "application/json",
    });
    await firstContext.close();
    await secondContext.close();
  }
});

const personalRecordShape = z.object({
  id: z.string(),
  kind: z.enum(["manual", "segment"]),
  startedAt: z.string(),
  endedAt: z.string().nullable(),
  note: z.string().nullable(),
  revision: z.number(),
});
const recordResultShape = z.object({ record: personalRecordShape });

function timerDatabaseEffects(actor: string, task: string): string {
  if (![actor, task].every((id) => /^[0-9a-f-]{36}$/.test(id)))
    throw new Error("invalid fixture UUID");
  return diagnosticSql(`SELECT jsonb_build_object(
    'runs',(SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY r.id),'[]'::jsonb) FROM fvoci.task_timer_runs r WHERE r.user_id='${actor}'),
    'segments',(SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY s.id),'[]'::jsonb) FROM fvoci.task_timer_segments s WHERE s.user_id='${actor}'),
    'receipts',(SELECT COALESCE(jsonb_agg(to_jsonb(c) ORDER BY c.request_id),'[]'::jsonb) FROM fvoci.task_timer_commands c WHERE c.user_id='${actor}'),
    'audit',(SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]'::jsonb) FROM fvoci.task_timer_audit a WHERE a.user_id='${actor}'),
    'legacy',(SELECT COALESCE(jsonb_agg(to_jsonb(l) ORDER BY l.time_entry_id),'[]'::jsonb) FROM fvoci.task_timer_legacy_open l WHERE l.user_id='${actor}'),
    'history',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY e.id),'[]'::jsonb) FROM fvoci.time_entries e WHERE e.user_id='${actor}'),
    'events',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY e.id),'[]'::jsonb) FROM fvoci.events e WHERE e.target_id='${task}'),
    'taskAudit',(SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]'::jsonb) FROM fvoci.audit_log a WHERE a.target_id='${task}'),
    'task',(SELECT jsonb_build_object('id',t.id,'statusId',t.status_id,'startDate',t.start_date,'dueDate',t.due_date,'dueAt',t.due_at,'recurrence',t.recurrence,'estimate',t.estimate,'updatedAt',t.updated_at) FROM fvoci.tasks t WHERE t.id='${task}')
  )`);
}

// Observe the real API client's JSON consumption without changing requests,
// status, body or Vue state. A MessageChannel task runs after the response's
// promise continuations and Vue's microtask flush, including stale-run return.
async function observeOwnerCompletion(page: import("@playwright/test").Page): Promise<void> {
  await page.evaluate(() => {
    const witness = document.createElement("script");
    witness.type = "application/json";
    witness.dataset.testid = "owner-delivery-witness";
    document.body.append(witness);
    const requested: string[] = [];
    const completed: string[] = [];
    const publish = () => {
      witness.textContent = JSON.stringify({ requested, completed });
    };
    publish();
    const nativeFetch = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input, init) => {
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      const url = input instanceof Request ? input.url : String(input);
      if (
        method !== "POST" ||
        new URL(url, location.href).pathname !== "/api/v1/me/task-timer/stop"
      )
        return nativeFetch(input, init);
      let body: unknown;
      if (input instanceof Request) body = await input.clone().json();
      else {
        const requestBody = init?.body;
        if (typeof requestBody !== "string")
          throw new Error("owner request witness requires the actual JSON request");
        body = JSON.parse(requestBody);
      }
      if (
        typeof body !== "object" ||
        body === null ||
        !("runId" in body) ||
        typeof body.runId !== "string"
      )
        throw new Error("owner request witness requires the actual run identity");
      const runId = body.runId;
      requested.push(runId);
      publish();
      const response = await nativeFetch(input, init);
      const nativeJson = response.json.bind(response);
      response.json = async () => {
        const result: unknown = await nativeJson();
        const delivery = new MessageChannel();
        delivery.port1.onmessage = () => {
          completed.push(runId);
          publish();
          delivery.port1.close();
          delivery.port2.close();
        };
        delivery.port2.postMessage(runId);
        return result;
      };
      return response;
    };
  });
}
async function ownerCompletion(
  page: import("@playwright/test").Page,
  runId: string,
): Promise<void> {
  await expect
    .poll(async () => {
      const value = await page.getByTestId("owner-delivery-witness").textContent();
      return z.object({ completed: z.array(z.string()) }).parse(JSON.parse(value ?? "null"))
        .completed;
    })
    .toContain(runId);
}

async function ordinaryTimerTask(page: import("@playwright/test").Page, key: string) {
  await timerWorkspace(page);
  const slug = `w5-${key.toLowerCase()}-timer`;
  const createdWorkspace = await page.request.post("/api/v1/workspaces", {
    data: { name: "독립 측정 검증", slug },
  });
  expect(createdWorkspace.status(), await createdWorkspace.text()).toBe(201);
  const workspaceId = z.object({ id: z.string() }).parse(await createdWorkspace.json()).id;
  const email = `timer-${key.toLowerCase()}@example.com`;
  createE2eUser(email, credentials.password, "기록 검증 작성자", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const loggedOut = await page.request.post("/api/v1/auth/logout");
  expect(loggedOut.ok(), await loggedOut.text()).toBe(true);
  await login(page, email, credentials.password);
  const actor = identityShape.parse(await (await page.request.get("/api/v1/auth/me")).json());
  const createdProject = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key, name: "기록 검증", visibility: "workspace" },
  });
  expect(createdProject.status(), await createdProject.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await createdProject.json());
  const created = await page.request.post(
    `/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`,
    {
      data: { title: `${key} 기록 검증` },
    },
  );
  expect(created.status(), await created.text()).toBe(201);
  const task = taskShape.parse(await created.json());
  const assigned = await page.request.patch(`/api/v1/workspaces/${workspaceId}/tasks/${task.id}`, {
    data: { assigneeIds: [actor.userId] },
  });
  expect(assigned.ok(), await assigned.text()).toBe(true);
  return {
    workspaceId,
    actor,
    task,
    slug,
    email,
    detail: `/w/${slug}/${key}-${String(task.number)}`,
    timerUrl: `/api/v1/workspaces/${workspaceId}/tasks/${task.id}/timer`,
  };
}

test("mounted personal manual correction keeps a conflicting draft and fresh-client day week history", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TMAN");
  const me = z
    .object({ timezone: z.string() })
    .parse(await (await page.request.get("/api/v1/auth/me")).json());
  await page.goto(fixture.detail);
  const panel = page.getByTestId("task-personal-time-records");
  await expect(panel.getByRole("button", { name: "기록 추가", exact: true })).toBeEnabled();
  const sampled = z
    .object({ serverNow: z.string() })
    .parse(await (await page.request.get(fixture.timerUrl)).json());
  const end = new Date(Date.parse(sampled.serverNow) - 60_000).toISOString();
  const start = new Date(Date.parse(end) - 900_000).toISOString();
  await panel.getByRole("button", { name: "기록 추가", exact: true }).click();
  await panel
    .getByLabel("시작", { exact: true })
    .fill(isoToDatetimeLocalInTimeZone(start, me.timezone));
  await panel
    .getByLabel("종료", { exact: true })
    .fill(isoToDatetimeLocalInTimeZone(end, me.timezone));
  await panel.getByLabel("메모", { exact: true }).fill("최초 읽기 기록");
  await panel.getByLabel("기록·수정 사유", { exact: true }).fill("읽은 시간을 직접 입력");
  const createdResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === `${fixture.timerUrl}/history` &&
      response.request().method() === "POST",
  );
  await panel.getByRole("button", { name: "기록 추가", exact: true }).click();
  const created = await createdResponse;
  expect(created.status(), await created.text()).toBe(200);
  const first = recordResultShape.parse(await created.json()).record;
  const row = panel.locator(`[data-record-id="${first.id}"]`);
  await expect(row).toContainText("최초 읽기 기록");
  await expect(panel.getByTestId("task-personal-time-summary")).toContainText("00:15:00.000");
  await row.getByRole("button", { name: "기록 수정", exact: true }).click();
  await panel.getByLabel("메모", { exact: true }).fill("충돌 뒤 보존할 내 초안");
  await panel.getByLabel("기록·수정 사유", { exact: true }).fill("독서 메모 수정");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    const correctionUrl = `${fixture.timerUrl}/records/${first.id}/correct`;
    const other = await fresh.request.post(correctionUrl, {
      data: {
        expectedActorId: identity.userId,
        expectedSessionId: identity.sessionId,
        requestId: crypto.randomUUID(),
        kind: first.kind,
        expectedRevision: first.revision,
        expectedStartedAt: first.startedAt,
        expectedEndedAt: first.endedAt,
        expectedNote: first.note,
        startedAt: first.startedAt,
        endedAt: first.endedAt,
        note: "다른 창의 현재 기록",
        reason: "다른 창 수정",
      },
    });
    expect(other.status(), await other.text()).toBe(200);
    const beforeConflict = timerDatabaseEffects(identity.userId, fixture.task.id);
    const conflictResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === correctionUrl &&
        response.request().method() === "POST",
    );
    await panel.getByRole("button", { name: "기록 수정", exact: true }).last().click();
    const conflict = await conflictResponse;
    expect(conflict.status(), await conflict.text()).toBe(409);
    expect(
      z.object({ params: z.object({ code: z.string() }) }).parse(await conflict.json()).params.code,
    ).toBe("time_record_version");
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeConflict);
    await expect(panel.getByLabel("메모", { exact: true })).toHaveValue("충돌 뒤 보존할 내 초안");
    await expect(row).toContainText("다른 창의 현재 기록");
    await panel
      .getByRole("button", { name: "현재 기록을 확인하고 다시 적용", exact: true })
      .click();
    const correctedResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === correctionUrl &&
        response.request().method() === "POST",
    );
    await panel.getByRole("button", { name: "기록 수정", exact: true }).last().click();
    const corrected = await correctedResponse;
    expect(corrected.status(), await corrected.text()).toBe(200);
    const final = recordResultShape.parse(await corrected.json()).record;
    expect(final.revision).toBe(2);
    expect(final.startedAt).toBe(first.startedAt);
    expect(final.endedAt).toBe(first.endedAt);
    await expect(row).toContainText("충돌 뒤 보존할 내 초안");
    await expect(panel.getByTestId("task-personal-time-summary")).toContainText("00:15:00.000");
    expect(diagnosticSql(`SELECT note FROM fvoci.time_entries WHERE id='${first.id}'`)).toBe(
      "최초 읽기 기록",
    );
    expect(
      diagnosticSql(
        `SELECT count(*) FROM fvoci.task_timer_audit WHERE time_entry_id='${first.id}' AND reason='독서 메모 수정'`,
      ),
    ).toBe("1");
    await fresh.goto(fixture.detail);
    await expect(
      fresh.getByTestId("task-personal-time-records").locator(`[data-record-id="${first.id}"]`),
    ).toContainText("충돌 뒤 보존할 내 초안");
    await expect(fresh.getByTestId("task-personal-time-summary")).toContainText("00:15:00.000");
    expect(
      taskShape.parse(
        await (
          await fresh.request.get(
            `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}`,
          )
        ).json(),
      ).statusId,
    ).toBe(fixture.task.statusId);
    const evidence = process.env.FVOCI_W5_EVIDENCE_DIR;
    if (!evidence) throw new Error("missing owned evidence namespace");
    for (const zoom of [100, 200]) {
      await page.setViewportSize({ width: 320, height: 900 });
      await page.evaluate((percent) => {
        document.documentElement.style.fontSize = `${String(percent)}%`;
      }, zoom);
      await panel.scrollIntoViewIfNeeded();
      await expect
        .poll(() => panel.evaluate((element) => element.scrollWidth <= element.clientWidth))
        .toBe(true);
      await page.screenshot({
        path: path.join(evidence, `manual-personal-320-text-${String(zoom)}.png`),
        fullPage: true,
      });
    }
    await page.evaluate(() => {
      document.documentElement.style.fontSize = "";
    });
    await page.setViewportSize({ width: 1280, height: 900 });
    await row.getByRole("button", { name: "기록 수정", exact: true }).click();
    await panel.getByLabel("시작", { exact: true }).focus();
    await page.keyboard.press("Tab");
    await expect(panel.getByLabel("종료", { exact: true })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(panel.getByLabel("메모", { exact: true })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(panel.getByLabel("기록·수정 사유", { exact: true })).toBeFocused();
    await page.screenshot({
      path: path.join(evidence, "manual-personal-keyboard-focus.png"),
      fullPage: true,
    });
    await testInfo.attach("personal-manual-correction-contract", {
      body: JSON.stringify({
        record: first.id,
        revision: final.revision,
        rawRangePreserved: true,
        conflictFullSnapshotUnchanged: true,
        raw034NoteUnchanged: true,
        correctionReasonPersisted: true,
        freshSessionRead: identity.sessionId !== fixture.actor.sessionId,
      }),
      contentType: "application/json",
    });
  } finally {
    await context.close();
  }
});

test("a committed withheld owner stop cannot keep a successor run's current control pending", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TABA");
  const run1 = await startPausedTimer(page, fixture.timerUrl, fixture.actor, "첫 번째 측정");
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  const owner = page.getByTestId("timer-owner");
  await expect(owner.getByRole("button", { name: "현재 측정 종료", exact: true })).toBeEnabled();
  await observeOwnerCompletion(page);
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let release = () => {};
  const delivery = new Promise<void>((resolve) => {
    release = resolve;
  });
  let committed = () => {};
  const committedSignal = new Promise<void>((resolve) => {
    committed = resolve;
  });
  let body: unknown;
  let outcome: unknown;
  await page.route(
    (url) => url.pathname === "/api/v1/me/task-timer/stop",
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      body = route.request().postDataJSON();
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      outcome = await response.json();
      committed();
      await delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  try {
    await owner.getByRole("button", { name: "현재 측정 종료", exact: true }).click();
    await committedSignal;
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    expect(
      timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run,
    ).toBeNull();
    const beforeReplay = timerDatabaseEffects(identity.userId, fixture.task.id);
    const requestBody = z
      .object({
        expectedActorId: z.string(),
        expectedSessionId: z.string(),
        requestId: z.string(),
        runId: z.string(),
        expectedVersion: z.number(),
      })
      .parse(body);
    expect(requestBody.runId).toBe(run1.runId);
    const replayed = await fresh.request.post("/api/v1/me/task-timer/stop", { data: requestBody });
    expect(replayed.status(), await replayed.text()).toBe(200);
    expect(await replayed.json()).toEqual(outcome);
    const changed = await fresh.request.post("/api/v1/me/task-timer/stop", {
      data: { ...requestBody, expectedVersion: requestBody.expectedVersion + 1 },
    });
    expect(changed.status(), await changed.text()).toBe(409);
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeReplay);
    const run2 = await startPausedTimer(fresh, fixture.timerUrl, identity, "後続 측정");
    const successor = page.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer" &&
        response.status() === 200 &&
        z.object({ runId: z.string().nullable() }).parse(await response.json()).runId ===
          run2.runId,
    );
    await page.bringToFront();
    await successor;
    const beforeDelivery = timerDatabaseEffects(identity.userId, fixture.task.id);
    await testInfo.attach("committed-response-withheld-run-ABA", {
      body: JSON.stringify({
        run1: run1.runId,
        run2: run2.runId,
        actualNativeCommit200: true,
        browserResponseStillWithheld: true,
        genuineFreshSessionReplaySameOutcome: true,
        changedPayload409: true,
        replayFullSnapshotUnchanged: true,
      }),
      contentType: "application/json",
    });
    // Original product negative: an R1 pending completion cannot disable the
    // actual canonical R2 control. Keep this literal oracle through the fix.
    await expect(owner.getByRole("button", { name: "현재 측정 종료", exact: true })).toBeEnabled();
    release();
    await ownerCompletion(page, run1.runId);
    await expect(owner.getByRole("button", { name: "현재 측정 종료", exact: true })).toBeEnabled();
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeDelivery);
    expect(timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run?.id).toBe(
      run2.runId,
    );
  } finally {
    release();
    await context.close();
  }
});

test("an ordinary mounted time-entry GET cannot render another actor's private correction under stale identity", async ({
  page,
  browser,
}, testInfo) => {
  const fixture = await ordinaryTimerTask(page, "TPRIV");
  const email = "timer-private-overlay@example.com";
  createE2eUser(email, credentials.password, "다른 작성자", {
    workspaceSlug: fixture.slug,
    membershipRole: "member",
  });
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  let releaseMe = () => {};
  const meDelivery = new Promise<void>((resolve) => {
    releaseMe = resolve;
  });
  try {
    const other = await context.newPage();
    await login(other, email, credentials.password);
    const identity = identityShape.parse(await (await other.request.get("/api/v1/auth/me")).json());
    const created = await other.request.post(`${fixture.timerUrl}/history`, {
      data: {
        expectedActorId: identity.userId,
        expectedSessionId: identity.sessionId,
        requestId: crypto.randomUUID(),
        startedAt: "2026-09-30T00:00:00Z",
        endedAt: "2026-09-30T00:15:00Z",
        note: "공유된 원래 기록",
        reason: "수동 기록",
      },
    });
    expect(created.status(), await created.text()).toBe(200);
    const row = recordResultShape.parse(await created.json()).record;
    const corrected = await other.request.post(`${fixture.timerUrl}/records/${row.id}/correct`, {
      data: {
        expectedActorId: identity.userId,
        expectedSessionId: identity.sessionId,
        requestId: crypto.randomUUID(),
        kind: row.kind,
        expectedRevision: row.revision,
        expectedStartedAt: row.startedAt,
        expectedEndedAt: row.endedAt,
        expectedNote: row.note,
        startedAt: row.startedAt,
        endedAt: row.endedAt,
        note: "다른 작성자만 볼 사적인 수정",
        reason: "개인 수정",
      },
    });
    expect(corrected.status(), await corrected.text()).toBe(200);
    await page.goto(`/w/${fixture.slug}/my-tasks`);
    await expect(page.getByTestId(`my-task-${fixture.task.id}`)).toBeVisible();
    const before = timerDatabaseEffects(identity.userId, fixture.task.id);
    await page.route(
      (url) => url.pathname === "/api/v1/auth/me",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const response = await route.fetch();
        await meDelivery;
        if (!page.isClosed()) await route.fulfill({ response });
      },
    );
    // Hold only captured timer transports with a genuine network-unavailable
    // status. The ordinary GET below is actual Rust/DB, never a fake private DTO.
    await page.route(
      (url) =>
        url.pathname === "/api/v1/me/task-timer" || url.pathname.startsWith(fixture.timerUrl),
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const capture = new URL(route.request().url()).searchParams;
        expect(capture.get("expectedActorId")).toBe(fixture.actor.userId);
        expect(capture.get("expectedSessionId")).toBe(fixture.actor.sessionId);
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", status: 503, title: "측정 연결 실패" }),
        });
      },
    );
    await page.context().addCookies(await context.cookies());
    const entriesPath = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${fixture.task.id}/time-entries`;
    const ordinary = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === entriesPath && response.request().method() === "GET",
    );
    await page.getByTestId(`my-task-${fixture.task.id}`).click();
    const response = await ordinary;
    const parsed = z
      .object({ items: z.array(z.object({ id: z.string(), note: z.string().nullable() })) })
      .safeParse(await response.json());
    if (
      response.status() === 200 &&
      parsed.success &&
      parsed.data.items.some((item) => item.id === row.id)
    )
      await expect(page.locator(`[data-time-entry-id="${row.id}"]`)).toBeVisible();
    await testInfo.attach("ordinary-real-private-overlay-stale-capture", {
      body: JSON.stringify({
        status: response.status(),
        staleActor: fixture.actor.userId,
        authenticatedActor: identity.userId,
        expectedActorParameter: new URL(response.url()).searchParams.get("expectedActorId"),
        expectedSessionParameter: new URL(response.url()).searchParams.get("expectedSessionId"),
        returnedOtherPrivateCorrection:
          parsed.success &&
          parsed.data.items.some((item) => item.note === "다른 작성자만 볼 사적인 수정"),
        privateResponseMocked: false,
        identityDeliveryWithheld: true,
      }),
      contentType: "application/json",
    });
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(before);
    // Original privacy oracle, unchanged after any granted captured GET fix.
    await expect(page.getByTestId("task-time-entries")).not.toContainText(
      "다른 작성자만 볼 사적인 수정",
    );
  } finally {
    releaseMe();
    await context.close();
  }
});

test("a late R1 response cannot clear the genuine pending stop of canonical R2", async ({
  page,
  browser,
}) => {
  const fixture = await ordinaryTimerTask(page, "TPEND");
  const run1 = await startPausedTimer(page, fixture.timerUrl, fixture.actor, "First pending owner");
  await page.goto(`/w/${fixture.slug}/my-tasks`);
  const owner = page.getByTestId("timer-owner");
  const button = owner.getByRole("button", { name: "현재 측정 종료", exact: true });
  await expect(button).toBeEnabled();
  await observeOwnerCompletion(page);
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  const held = [run1.runId, ""].map(() => {
    let release = () => {};
    let committed = () => {};
    const delivery = new Promise<void>((resolve) => {
      release = resolve;
    });
    const commit = new Promise<void>((resolve) => {
      committed = resolve;
    });
    const operation: {
      delivery: Promise<void>;
      commit: Promise<void>;
      release: () => void;
      committed: () => void;
      body: unknown;
    } = { delivery, commit, release, committed, body: undefined };
    return operation;
  });
  const [first, second] = held;
  if (!first || !second) throw new Error("missing held response fixture");
  let index = 0;
  let releaseOwner = () => {};
  const ownerDelivery = new Promise<void>((resolve) => {
    releaseOwner = resolve;
  });
  await page.route(
    (url) => url.pathname === "/api/v1/me/task-timer/stop",
    async (route) => {
      if (route.request().method() !== "POST") return route.continue();
      const operation = held[index++];
      if (!operation) throw new Error("unexpected third owner command");
      operation.body = route.request().postDataJSON();
      const response = await route.fetch();
      expect(response.status(), await response.text()).toBe(200);
      operation.committed();
      await operation.delivery;
      if (!page.isClosed()) await route.fulfill({ response });
    },
  );
  try {
    await button.click();
    await first.commit;
    const fresh = await context.newPage();
    await login(fresh, fixture.email, credentials.password);
    const identity = identityShape.parse(await (await fresh.request.get("/api/v1/auth/me")).json());
    const run2 = await startPausedTimer(fresh, fixture.timerUrl, identity, "Second pending owner");
    const successor = page.waitForResponse(
      async (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer" &&
        response.status() === 200 &&
        z.object({ runId: z.string().nullable() }).parse(await response.json()).runId ===
          run2.runId,
    );
    await page.bringToFront();
    await successor;
    await expect(button).toBeEnabled();
    // Keep the actual canonical R2 read visible until its own completion; do
    // not let a later owner-null poll remove the control before this oracle.
    await page.route(
      (url) => url.pathname === "/api/v1/me/task-timer",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue();
        const response = await route.fetch();
        await ownerDelivery;
        if (!page.isClosed()) await route.fulfill({ response });
      },
    );
    await button.click();
    await second.commit;
    expect(z.object({ runId: z.string() }).parse(second.body).runId).toBe(run2.runId);
    const beforeDelivery = timerDatabaseEffects(identity.userId, fixture.task.id);
    await expect(button).toBeDisabled();
    const firstResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/v1/me/task-timer/stop" &&
        z.object({ runId: z.string() }).parse(response.request().postDataJSON()).runId ===
          run1.runId,
    );
    first.release();
    await firstResponse;
    await ownerCompletion(page, run1.runId);
    await expect(button).toBeDisabled();
    await button.evaluate((element) => {
      if (!(element instanceof HTMLButtonElement))
        throw new Error("owner control is not a native button");
      element.click();
    });
    const attempts = z
      .object({ requested: z.array(z.string()) })
      .parse(
        JSON.parse((await page.getByTestId("owner-delivery-witness").textContent()) ?? "null"),
      );
    expect(attempts.requested).toEqual([run1.runId, run2.runId]);
    expect(index).toBe(2);
    await expect(button).toBeDisabled();
    expect(timerDatabaseEffects(identity.userId, fixture.task.id)).toBe(beforeDelivery);
    second.release();
    releaseOwner();
    await expect(owner).toHaveCount(0);
    expect(
      timerShape.parse(await (await fresh.request.get(fixture.timerUrl)).json()).run,
    ).toBeNull();
  } finally {
    for (const operation of held) operation.release();
    releaseOwner();
    await context.close();
  }
});
