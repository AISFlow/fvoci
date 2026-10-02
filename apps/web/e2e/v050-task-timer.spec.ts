import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { z } from "zod";
import { createE2eUser, login } from "./helpers";

const credentials = { email: "timer@example.com", password: "supersecret1" };
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
    const me = z
      .object({ userId: z.string() })
      .parse(await (await editor.request.get("/api/v1/auth/me")).json());
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
    await editor.goto("/w/w5timer/my-tasks");
    const mounted = editor.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mounted.getByTestId("timer-start")).toBeEnabled();
    await mounted.getByTestId("timer-start").click();
    await expect(mounted.getByTestId("timer-state")).toHaveText("측정 중");
    const committed = timerShape.parse(await (await editor.request.get(timerUrl)).json());
    expect(committed.run?.id).toBeTruthy();
    await editor.route(`**${timerUrl}`, async (route) => {
      if (route.request().method() !== "GET") return route.continue();
      await route.fulfill({
        status: 503,
        contentType: "application/problem+json",
        body: JSON.stringify({ type: "about:blank", title: "일시적인 연결 실패", status: 503 }),
      });
    });
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
    await editor.unroute(`**${timerUrl}`);
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
    await viewer.goto("/w/w5timer/my-tasks");
    const timer = viewer.getByTestId(`task-stopwatch-${task.id}`);
    await expect(timer).toBeVisible();
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const promote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(promote.ok(), await promote.text()).toBe(true);
    expect(await capability()).toBe(true);
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const demote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "viewer" },
    });
    expect(demote.ok(), await demote.text()).toBe(true);
    expect(await capability()).toBe(false);
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const again = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(again.ok(), await again.text()).toBe(true);
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const archive = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/archive`,
    );
    expect(archive.ok(), await archive.text()).toBe(true);
    expect(await capability()).toBe(false);
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
  } finally {
    await context.close();
  }
});
