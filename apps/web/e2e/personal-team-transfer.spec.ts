import { execFileSync } from "node:child_process";
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { z } from "zod";
import { createE2eUser, login } from "./helpers";

const owner = { email: "Admin@Example.COM", password: "supersecret1" };
const member = { email: "w2-transfer-member@example.com", password: "memberpass1" };
const workspaceSchema = z.object({ id: z.string().uuid(), slug: z.string() });
const workspacesSchema = z.object({
  items: z.array(z.object({ id: z.string().uuid(), kind: z.string(), slug: z.string() })),
});
const captureSchema = z.object({
  documentId: z.string().uuid(),
  documentDisplayId: z.string(),
  taskId: z.string().uuid(),
  taskDisplayId: z.string(),
  projectId: z.string().uuid(),
});
const projectSchema = z.object({ id: z.string().uuid(), key: z.string() });
const searchSchema = z.object({ items: z.array(z.object({ id: z.string() }).passthrough()) });
const transferSchema = z.object({
  workspaceId: z.string().uuid(),
  projectId: z.string().uuid(),
  documentId: z.string().uuid(),
  documentNumber: z.number().int(),
  taskId: z.string().uuid().nullable(),
  taskNumber: z.number().int().nullable(),
  replayed: z.boolean(),
});
const graphSchema = z.object({
  documents: z.number(),
  tasks: z.number(),
  origins: z.number(),
  assignees: z.number(),
  events: z.number(),
  eventVerbs: z.array(z.string()),
  receipts: z.number(),
  superuser: z.boolean(),
  bypass: z.boolean(),
});

// A new committed connection with exactly the run's restricted app role; the
// credentials stay in the child environment and never enter evidence.
function committed(workspaceId: string, select: string): unknown {
  z.string().uuid().parse(workspaceId);
  const url = new URL(process.env.DATABASE_APP_URL ?? "");
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!container) throw new Error("isolated PostgreSQL container is required");
  try {
    const output = execFileSync(
      "docker",
      [
        "exec",
        "-i",
        "-e",
        `PGPASSWORD=${decodeURIComponent(url.password)}`,
        container,
        "psql",
        "-X",
        "-qAt",
        "-h",
        "127.0.0.1",
        "-U",
        decodeURIComponent(url.username),
        "-d",
        url.pathname.slice(1),
        "-v",
        "ON_ERROR_STOP=1",
      ],
      {
        input: `BEGIN; SET LOCAL app.tenant_id='${workspaceId}'; ${select}; ROLLBACK;`,
        stdio: "pipe",
        encoding: "utf8",
      },
    );
    return JSON.parse(output.trim()) as unknown;
  } catch {
    throw new Error("restricted committed DB observation failed (credentials omitted)");
  }
}
/** The source pair's committed graph in one tenant, read as the app role. */
function graph(workspaceId: string, documentId: string, taskId: string) {
  for (const id of [documentId, taskId]) z.string().uuid().parse(id);
  return graphSchema.parse(
    committed(
      workspaceId,
      `SELECT json_build_object(
      'documents',(SELECT count(*) FROM fvoci.documents WHERE id='${documentId}'),
      'tasks',(SELECT count(*) FROM fvoci.tasks WHERE id='${taskId}'),
      'origins',(SELECT count(*) FROM fvoci.task_origins WHERE task_id='${taskId}' AND document_id='${documentId}'),
      'assignees',(SELECT count(*) FROM fvoci.task_assignees WHERE task_id='${taskId}'),
      'events',(SELECT count(*) FROM fvoci.events WHERE target_id IN ('${documentId}','${taskId}')),
      'eventVerbs',(SELECT coalesce(json_agg(verb || ':' || channel ORDER BY xact, seq),'[]'::json) FROM fvoci.events WHERE target_id IN ('${documentId}','${taskId}')),
      'receipts',(SELECT count(*) FROM fvoci.personal_transfer_commands),
      'superuser',(SELECT rolsuper FROM pg_roles WHERE rolname=current_user),
      'bypass',(SELECT rolbypassrls FROM pg_roles WHERE rolname=current_user))`,
    ),
  );
}
const relationSchema = z.object({
  relname: z.string(),
  enabled: z.boolean(),
  forced: z.boolean(),
  active: z.boolean(),
  ownerMember: z.boolean(),
});
const witnessSchema = z.object({
  database: z.string(),
  role: z.string(),
  superuser: z.boolean(),
  bypass: z.boolean(),
  tenant: z.string(),
  relations: z.array(relationSchema),
});
/**
 * Actual identity and RLS state of the restricted committed connection used
 * for every graph observation in this run, plus the run's own service ledger
 * (container image/start/PID/labels/mounts; never its environment). Recorded
 * exactly: documents FORCE is false by design, tasks/receipts FORCE true.
 */
async function witness(workspaceId: string, testInfo: TestInfo): Promise<void> {
  const observed = witnessSchema.parse(
    committed(
      workspaceId,
      `SELECT json_build_object('database',current_database(),'role',current_user,
      'superuser',(SELECT rolsuper FROM pg_roles WHERE rolname=current_user),
      'bypass',(SELECT rolbypassrls FROM pg_roles WHERE rolname=current_user),
      'tenant',current_setting('app.tenant_id',true),
      'relations',(SELECT json_agg(json_build_object('relname',relname,'enabled',relrowsecurity,'forced',relforcerowsecurity,
        'active',row_security_active(oid),'ownerMember',pg_has_role(current_user,relowner,'MEMBER')) ORDER BY relname)
        FROM pg_class WHERE oid=ANY(ARRAY['fvoci.documents'::regclass,'fvoci.tasks'::regclass,'fvoci.personal_transfer_commands'::regclass])))`,
    ),
  );
  expect(observed).toMatchObject({ superuser: false, bypass: false, tenant: workspaceId });
  expect(observed.relations.map((relation) => [relation.relname, relation.forced])).toEqual([
    ["documents", false],
    ["personal_transfer_commands", true],
    ["tasks", true],
  ]);
  for (const relation of observed.relations)
    expect(relation).toMatchObject({ enabled: true, active: true, ownerMember: false });
  const ledger = ["FVOCI_TEST_PG_CONTAINER", "FVOCI_TEST_MEILI_CONTAINER"].map((name) => {
    const container = process.env[name] ?? "";
    if (!container) return { name, container: null };
    const inspected = execFileSync(
      "docker",
      [
        "inspect",
        "--format",
        "{{json .Config.Image}}|{{json .State.StartedAt}}|{{json .State.Pid}}|{{json .Config.Labels}}|{{json .Mounts}}",
        container,
      ],
      { encoding: "utf8" },
    ).trim();
    return { name, container, inspected };
  });
  await testInfo.attach("restricted-role-and-service-witness", {
    body: JSON.stringify({ observed, ledger }, null, 1),
    contentType: "application/json",
  });
}
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
  await page.getByLabel("이름", { exact: true }).fill("이전");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Tracer team");
  await page.getByLabel("주소(영문)").fill("tracer");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}
/** Ordinary capture through the real API; the transfer UI is under test here. */
async function capturedPair(page: Page, title: string) {
  const personal = workspaceSchema.parse(
    await (await page.request.post("/api/v1/me/personal-workspace")).json(),
  );
  const response = await page.request.post(`/api/v1/workspaces/${personal.id}/personal-input`, {
    data: { requestId: crypto.randomUUID(), intent: "task", title },
  });
  expect(response.status()).toBe(201);
  return { personal, pair: captureSchema.parse(await response.json()) };
}
async function teamProject(page: Page, key: string, visibility: "workspace" | "private") {
  const team = workspacesSchema
    .parse(await (await page.request.get("/api/v1/me/workspaces")).json())
    .items.find((workspace) => workspace.kind === "team" && workspace.slug === "tracer");
  if (!team) throw new Error("tracer team workspace missing");
  const response = await page.request.post(`/api/v1/workspaces/${team.id}/projects`, {
    data: { key, name: `${key} 공개 프로젝트`, visibility },
  });
  expect(response.status()).toBe(201);
  return { team, project: projectSchema.parse(await response.json()) };
}
async function openTransfer(page: Page) {
  await page.getByRole("button", { name: "팀에 공개하거나 이동", exact: true }).click();
  return page.getByRole("dialog", { name: "팀에 공개하거나 이동", exact: true });
}
const dispositionsSchema = z.object({
  dispositions: z.array(z.object({ item: z.string(), outcome: z.string(), count: z.number() })),
});
function previewed(page: Page) {
  return page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers/preview") &&
      response.request().method() === "POST",
  );
}
/** The dialog discloses exactly what the server's preview says travels, stays or is left out. */
async function expectDisclosure(
  dialog: ReturnType<Page["getByRole"]>,
  preview: Awaited<ReturnType<typeof previewed>>,
) {
  expect(preview.status(), await preview.text()).toBe(200);
  const { dispositions } = dispositionsSchema.parse(await preview.json());
  expect(dispositions.length).toBeGreaterThan(0);
  const shown = dialog.locator("dd[data-outcome]");
  await expect(shown).toHaveCount(dispositions.length);
  expect(
    (await shown.evaluateAll((rows) => rows.map((row) => row.getAttribute("data-outcome")))).sort(),
  ).toEqual(dispositions.map((disposition) => disposition.outcome).sort());
  return dispositions;
}
async function choose(
  dialog: ReturnType<Page["getByRole"]>,
  action: "팀에 사본 공개" | "팀으로 이동",
  projectName: string,
) {
  await dialog.getByRole("radio", { name: action }).check();
  await dialog.getByLabel("대상 팀").selectOption({ label: "Tracer team" });
  await dialog.getByLabel("대상 프로젝트").selectOption({ label: projectName });
  await expect(dialog.getByLabel("작업 상태")).not.toHaveValue("");
  await dialog.getByRole("button", { name: "공개 내용 확인", exact: true }).click();
}
async function memberContext(browser: Browser, testInfo: TestInfo, user = member) {
  createE2eUser(user.email, user.password, "팀원", {
    workspaceSlug: "tracer",
    membershipRole: "member",
  });
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL });
  const page = await context.newPage();
  await login(page, user.email, user.password);
  return { context, page };
}

test("explicit COPY: Cancel has no effects, lost success replays once, new IDs, original stays private", async ({
  page,
  browser,
}, testInfo) => {
  test.setTimeout(90000);
  await setup(page);
  const token = `w2copy${String(Date.now())}`;
  const { personal, pair } = await capturedPair(page, `${token} 비공개 원본 🧑‍💻`);
  const { team, project } = await teamProject(page, "PUBC", "workspace");
  await witness(personal.id, testInfo);
  await page.goto(`/w/${personal.slug}/${pair.documentDisplayId}`);
  const opened = graph(personal.id, pair.documentId, pair.taskId);
  // The host's own save compacts the room's snapshot and records
  // document.collab_snapshot_compacted (src/db/collab.rs, observed at
  // cf81b8e). Baseline after that ACK; a later save of the same committed
  // snapshot is a no-op, so the preview's prepare and Cancel must leave the
  // settled graph exactly unchanged. Assert the ordering semantically.
  await page.getByRole("button", { name: "저장", exact: true }).first().click();
  await expect(page.locator('[data-collab-persisted="true"]').first()).toBeVisible();
  const before = graph(personal.id, pair.documentId, pair.taskId);
  expect({ ...before, events: 0, eventVerbs: [] }).toEqual({
    ...opened,
    events: 0,
    eventVerbs: [],
  });
  expect(before.eventVerbs.slice(0, opened.eventVerbs.length)).toEqual(opened.eventVerbs);
  expect(new Set(before.eventVerbs.slice(opened.eventVerbs.length))).toEqual(
    new Set(
      before.eventVerbs.length > opened.eventVerbs.length
        ? ["document.collab_snapshot_compacted:web"]
        : [],
    ),
  );
  await testInfo.attach("ordered-source-events", {
    body: JSON.stringify({ opened: opened.eventVerbs, settled: before.eventVerbs }),
    contentType: "application/json",
  });
  expect(before).toMatchObject({
    documents: 1,
    tasks: 1,
    origins: 1,
    superuser: false,
    bypass: false,
  });

  let dialog = await openTransfer(page);
  await expect(dialog.getByRole("radio", { name: "팀에 사본 공개" })).toBeChecked();
  await choose(dialog, "팀에 사본 공개", "PUBC 공개 프로젝트");
  await expect(dialog.getByText("PUBC 공개 프로젝트")).toBeVisible();
  await expect(
    dialog.getByText("워크스페이스 멤버와 이 프로젝트에 접근할 수 있는 게스트가 볼 수 있습니다."),
  ).toBeVisible();
  await expect(dialog.getByText("원본은 개인 공간에 남습니다.")).toBeVisible();
  await dialog.screenshot({ path: testInfo.outputPath("copy-disclosure-preview.png") });
  expect(graph(personal.id, pair.documentId, pair.taskId)).toEqual(before);
  let commands = 0;
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/personal-transfers")) commands++;
  });
  await dialog.getByRole("button", { name: "취소", exact: true }).click();
  await dialog.getByRole("button", { name: "닫기", exact: true }).click();
  expect(commands).toBe(0);
  expect(graph(personal.id, pair.documentId, pair.taskId)).toEqual(before);

  dialog = await openTransfer(page);
  await choose(dialog, "팀에 사본 공개", "PUBC 공개 프로젝트");
  let lost: z.infer<typeof transferSchema> | undefined;
  let original: unknown;
  await page.route("**/personal-transfers", async (route) => {
    original = route.request().postDataJSON() as unknown;
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    lost = transferSchema.parse(await response.json());
    // The actual Rust transaction committed; only the browser response is lost.
    await route.abort("failed");
  });
  await dialog.getByRole("button", { name: "팀에 사본 공개", exact: true }).click();
  // While a command's outcome is unknown the same dialog is retitled as the
  // recovery step (a04a01f: correct UI, located by the old name).
  const recovery = page.getByRole("dialog", { name: "미확인 팀 공개·이동 요청 확인", exact: true });
  await expect(recovery.getByRole("status")).toContainText("결과를 아직 확인하지 못했습니다.");
  await expect(recovery.getByRole("button", { name: "같은 요청 다시 확인" })).toBeEnabled();
  if (!lost) throw new Error("committed lost response was not observed");
  await page.unroute("**/personal-transfers");
  const retry = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers") && response.request().method() === "POST",
  );
  await recovery.getByRole("button", { name: "같은 요청 다시 확인" }).click();
  const retried = await retry;
  expect(retried.request().postDataJSON()).toEqual(original);
  const replay = transferSchema.parse(await retried.json());
  expect(replay).toEqual({ ...lost, replayed: true });
  expect(replay.documentId).not.toBe(pair.documentId);
  expect(replay.taskId).not.toBe(pair.taskId);
  expect(replay.workspaceId).toBe(team.id);
  await expect(dialog.getByText("요청이 완료되었습니다.")).toBeVisible();

  // COPY leaves the private pair, its ACL graph and source events unchanged;
  // only the command receipt is added in the source tenant.
  expect(graph(personal.id, pair.documentId, pair.taskId)).toEqual({
    ...before,
    receipts: before.receipts + 1,
  });
  const copyTask = replay.taskId ?? "";
  expect(graph(team.id, replay.documentId, copyTask)).toMatchObject({
    documents: 1,
    tasks: 1,
    origins: 1,
    assignees: 1,
  });
  await dialog.getByRole("button", { name: "팀에서 열기", exact: true }).click();
  await expect(page).toHaveURL(`/w/tracer/${project.key}-${String(replay.taskNumber)}`);

  const { context, page: teammate } = await memberContext(browser, testInfo);
  try {
    const copied = await teammate.request.get(`/api/v1/workspaces/${team.id}/tasks/${copyTask}`);
    expect(copied.status()).toBe(200);
    for (const suffix of [
      `tasks/${pair.taskId}`,
      `documents/${pair.documentId}`,
      `tasks/${pair.taskId}/backlinks`,
    ]) {
      const denied = await teammate.request.get(`/api/v1/workspaces/${personal.id}/${suffix}`);
      expect([403, 404]).toContain(denied.status());
    }
    const origins = await teammate.request.get(
      `/api/v1/workspaces/${team.id}/documents/${replay.documentId}/task-origins`,
    );
    expect(origins.status()).toBe(200);
    expect(JSON.stringify(await origins.json())).not.toContain(pair.documentId);

    // Search: convergence is the committed COPY events processed by the
    // search-index consumer, never a TTL wait. The teammate then finds the
    // copy by its token and never an ID of the private original, in the
    // team, in global search and on a fresh search page; the personal
    // workspace's search stays closed to them.
    await expect
      .poll(() =>
        committed(
          team.id,
          `SELECT (count(DISTINCT target_id)=2 AND COALESCE(bool_and(fvoci.app_outbox_is_processed('search-index',id)),false))::text::json FROM fvoci.events WHERE target_id IN ('${copyTask}','${replay.documentId}') AND verb IN ('task.created','document.created')`,
        ),
      )
      .toBe(true);
    const found = (body: z.infer<typeof searchSchema>) => body.items.map((item) => item.id);
    await expect
      .poll(async () => {
        const ids = found(
          searchSchema.parse(
            await (
              await teammate.request.get(`/api/v1/workspaces/${team.id}/search?q=${token}`)
            ).json(),
          ),
        );
        return ids.includes(copyTask) && ids.includes(replay.documentId);
      })
      .toBe(true);
    const teamSearch = await teammate.request.get(
      `/api/v1/workspaces/${team.id}/search?q=${token}`,
    );
    const globalSearch = await teammate.request.get(`/api/v1/search?q=${token}`);
    for (const response of [teamSearch, globalSearch]) {
      expect(response.status()).toBe(200);
      const text = await response.text();
      expect(text).not.toContain(pair.documentId);
      expect(text).not.toContain(pair.taskId);
      expect(text).not.toContain(personal.id);
    }
    // The same final responses checked above carry both copies.
    for (const response of [teamSearch, globalSearch])
      expect(found(searchSchema.parse(await response.json()))).toEqual(
        expect.arrayContaining([copyTask, replay.documentId]),
      );
    const hidden = await teammate.request.get(
      `/api/v1/workspaces/${personal.id}/search?q=${token}`,
    );
    expect([403, 404]).toContain(hidden.status());
    expect(await hidden.text()).not.toContain(token);
    // The fresh search page lists exactly the two copies, each linking to its
    // own team display route (task and project document), nothing else.
    await teammate.goto(`/w/tracer/search?q=${token}`);
    const results = teammate
      .getByRole("region", { name: "검색", exact: true })
      .getByRole("link", { name: new RegExp(`${token} 비공개 원본`) });
    await expect
      .poll(async () =>
        (
          await results.evaluateAll((links) =>
            links.map((link) => new URL((link as HTMLAnchorElement).href).pathname),
          )
        ).sort(),
      )
      .toEqual(
        [
          `/w/tracer/${project.key}-${String(replay.documentNumber)}`,
          `/w/tracer/${project.key}-${String(replay.taskNumber)}`,
        ].sort(),
      );
  } finally {
    await context.close();
  }
});

test("explicit same-ID MOVE keeps document/task UUIDs, removes the private source and updates a mounted MyTasks", async ({
  page,
  browser,
}, testInfo) => {
  test.setTimeout(90000);
  await setup(page);
  const token = `w2move${String(Date.now())}`;
  const title = `${token} 이동할 작업 日本語`;
  const { personal, pair } = await capturedPair(page, title);
  const { team, project } = await teamProject(page, "PUBM", "private");
  await witness(personal.id, testInfo);
  const fresh = await browser.newContext({ baseURL: testInfo.project.use.baseURL });
  try {
    const mine = await fresh.newPage();
    await login(mine, owner.email, owner.password);
    await mine.goto("/w/tracer/my-tasks");
    await page.goto(`/w/${personal.slug}/${pair.documentDisplayId}`);
    const dialog = await openTransfer(page);
    const movePreview = previewed(page);
    await choose(dialog, "팀으로 이동", "PUBM 공개 프로젝트");
    await expect(
      dialog.getByText("이 프로젝트에 접근 권한이 있는 사람이 볼 수 있습니다."),
    ).toBeVisible();
    const moveDispositions = await expectDisclosure(dialog, await movePreview);
    expect(moveDispositions).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ item: "document", outcome: "moved" }),
        expect.objectContaining({ item: "task", outcome: "moved" }),
      ]),
    );
    await expect(
      dialog.getByText("완료 후 원본은 선택한 팀 프로젝트에서 열 수 있습니다."),
    ).toBeVisible();
    const committedMove = page.waitForResponse(
      (response) =>
        response.url().endsWith("/personal-transfers") && response.request().method() === "POST",
    );
    await dialog.getByRole("button", { name: "팀으로 이동", exact: true }).click();
    const response = await committedMove;
    expect(response.status(), await response.text()).toBe(200);
    const moved = transferSchema.parse(await response.json());
    expect(moved).toMatchObject({
      workspaceId: team.id,
      projectId: project.id,
      documentId: pair.documentId,
      taskId: pair.taskId,
      replayed: false,
    });
    expect(graph(personal.id, pair.documentId, pair.taskId)).toMatchObject({
      documents: 0,
      tasks: 0,
      origins: 0,
      assignees: 0,
    });
    expect(graph(team.id, pair.documentId, pair.taskId)).toMatchObject({
      documents: 1,
      tasks: 1,
      origins: 1,
      assignees: 1,
    });
    // The mounted aggregate converges through its own authorized stream/refetch.
    await expect(mine.getByTestId(`my-task-${pair.taskId}`)).toContainText(title);
    const oldRoute = await mine.request.get(
      `/api/v1/workspaces/${personal.id}/tasks/${pair.taskId}`,
    );
    expect([403, 404]).toContain(oldRoute.status());
    await mine.goto(`/w/tracer/${project.key}-${String(moved.taskNumber)}`);
    await expect(mine.getByTestId("task-edit-title")).toHaveValue(title);
    // Search: once the search-index consumer has processed the team's
    // created events and the personal workspace's deleted events, the same
    // IDs are found in team and global search and no longer in personal
    // search (no TTL wait).
    const processed = (workspace: string, verbs: string) =>
      committed(
        workspace,
        `SELECT (count(DISTINCT target_id)=2 AND COALESCE(bool_and(fvoci.app_outbox_is_processed('search-index',id)),false))::text::json FROM fvoci.events WHERE target_id IN ('${pair.taskId}','${pair.documentId}') AND verb IN (${verbs})`,
      );
    await expect.poll(() => processed(team.id, "'task.created','document.created'")).toBe(true);
    await expect.poll(() => processed(personal.id, "'task.deleted','document.deleted'")).toBe(true);
    const ids = async (url: string) => {
      const response = await mine.request.get(url);
      expect(response.status(), url).toBe(200);
      return searchSchema.parse(await response.json()).items.map((item) => item.id);
    };
    await expect
      .poll(async () => ids(`/api/v1/workspaces/${team.id}/search?q=${token}`))
      .toEqual(expect.arrayContaining([pair.taskId, pair.documentId]));
    expect(await ids(`/api/v1/search?q=${token}`)).toEqual(
      expect.arrayContaining([pair.taskId, pair.documentId]),
    );
    const personalIds = await ids(`/api/v1/workspaces/${personal.id}/search?q=${token}`);
    expect(personalIds).not.toContain(pair.taskId);
    expect(personalIds).not.toContain(pair.documentId);
  } finally {
    await fresh.close();
  }
});
test("task page MOVE refuses an unapplied draft, then saves the body and moves with the same IDs", async ({
  page,
  browser,
}, testInfo) => {
  test.setTimeout(90000);
  await setup(page);
  const token = `w2taskmove${String(Date.now())}`;
  const title = `${token} 작업 화면 이동 日本語`;
  const { personal, pair } = await capturedPair(page, title);
  const { team, project } = await teamProject(page, "PUBT", "private");
  await witness(personal.id, testInfo);
  await page.goto(`/w/${personal.slug}/${pair.taskDisplayId}`);
  await expect(page.getByTestId("task-edit-title")).toHaveValue(title);
  const body = page.getByTestId("task-body");
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 30_000 });
  await body.locator(".fvoci-editor .ProseMirror").click();
  const bodyText = `${token} 이동 전 본문 🎯`;
  await page.keyboard.type(bodyText);
  // An unapplied hierarchy draft on the mounted form refuses before any request.
  await page.getByTestId("task-edit-type").selectOption("bug");
  const transferRequests: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/personal-transfers")) transferRequests.push(request.url());
  });
  const receiptsBefore = graph(personal.id, pair.documentId, pair.taskId).receipts;
  let dialog = await openTransfer(page);
  await choose(dialog, "팀으로 이동", "PUBT 공개 프로젝트");
  await expect(
    dialog.getByText("적용하지 않은 초안을 먼저 저장하거나 취소한 뒤 다시 확인하세요."),
  ).toBeVisible();
  expect(transferRequests).toEqual([]);
  expect(graph(personal.id, pair.documentId, pair.taskId)).toMatchObject({
    documents: 1,
    tasks: 1,
    origins: 1,
    receipts: receiptsBefore,
  });
  await dialog.getByRole("button", { name: "닫기", exact: true }).click();
  await page.getByTestId("task-edit-hierarchy-cancel").click();
  dialog = await openTransfer(page);
  await choose(dialog, "팀으로 이동", "PUBT 공개 프로젝트");
  await expect(
    dialog.getByText("완료 후 원본은 선택한 팀 프로젝트에서 열 수 있습니다."),
  ).toBeVisible();
  const committedMove = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers") && response.request().method() === "POST",
  );
  await dialog.getByRole("button", { name: "팀으로 이동", exact: true }).click();
  const response = await committedMove;
  expect(response.status(), await response.text()).toBe(200);
  const moved = transferSchema.parse(await response.json());
  expect(moved).toMatchObject({
    workspaceId: team.id,
    projectId: project.id,
    documentId: pair.documentId,
    taskId: pair.taskId,
    replayed: false,
  });
  expect(graph(personal.id, pair.documentId, pair.taskId)).toMatchObject({
    documents: 0,
    tasks: 0,
    origins: 0,
  });
  expect(graph(team.id, pair.documentId, pair.taskId)).toMatchObject({
    documents: 1,
    tasks: 1,
    origins: 1,
  });
  // A fresh client reads the body the task page saved before the transfer,
  // and a team task mounts no personal transfer.
  const fresh = await browser.newContext({ baseURL: testInfo.project.use.baseURL });
  try {
    const mine = await fresh.newPage();
    await login(mine, owner.email, owner.password);
    await mine.goto(`/w/tracer/${project.key}-${String(moved.taskNumber)}`);
    await expect(mine.getByTestId("task-edit-title")).toHaveValue(title);
    await expect(mine.getByTestId("task-body").locator(".fvoci-editor .ProseMirror")).toContainText(
      bodyText,
      { timeout: 30_000 },
    );
    await expect(
      mine.getByRole("button", { name: "팀에 공개하거나 이동", exact: true }),
    ).toHaveCount(0);
  } finally {
    await fresh.close();
  }
});

const uploadSchema = z.object({
  attachmentId: z.string().uuid(),
  partSizeBytes: z.number().int(),
  parts: z.array(z.object({ partNumber: z.number().int(), url: z.string() })),
});
/** A real upload through the product routes: create, PUT each part, complete. */
async function uploadTaskFile(
  page: Page,
  workspace: string,
  task: string,
  name: string,
  bytes: Buffer,
) {
  const created = await page.request.post(`/api/v1/workspaces/${workspace}/tasks/${task}/uploads`, {
    data: { name, sizeBytes: bytes.length },
  });
  expect(created.status(), await created.text()).toBe(201);
  const session = uploadSchema.parse(await created.json());
  const parts: { partNumber: number; etag: string }[] = [];
  for (const part of session.parts) {
    const start = (part.partNumber - 1) * session.partSizeBytes;
    const put = await page.request.put(part.url, {
      data: bytes.subarray(start, Math.min(start + session.partSizeBytes, bytes.length)),
      headers: { "content-type": "application/octet-stream" },
    });
    expect(put.status(), await put.text()).toBe(200);
    parts.push({ partNumber: part.partNumber, etag: put.headers().etag ?? "" });
  }
  const completed = await page.request.post(
    `/api/v1/workspaces/${workspace}/attachments/${session.attachmentId}/complete`,
    { data: { parts } },
  );
  expect(completed.ok(), await completed.text()).toBe(true);
  return session.attachmentId;
}

test("COPY with an attachment: the copy gets a new file id a fresh teammate downloads exactly; the original stays private", async ({
  page,
  browser,
}, testInfo) => {
  test.setTimeout(90000);
  await setup(page);
  const token = `w2copyfile${String(Date.now())}`;
  const { personal, pair } = await capturedPair(page, `${token} 첨부 복사 원본`);
  const { team, project } = await teamProject(page, "PUBF", "workspace");
  await witness(personal.id, testInfo);
  const bytes = Buffer.from(Array.from({ length: 70_000 }, (_, i) => (i * 7919) % 251));
  const fileName = `${token} 증빙.bin`;
  const attachment = await uploadTaskFile(page, personal.id, pair.taskId, fileName, bytes);
  // The document body shows the file (product body write).
  const body = {
    type: "doc",
    content: [
      { type: "paragraph", content: [{ type: "text", text: `${token} 첨부를 보이는 본문` }] },
      { type: "attachment", attrs: { id: attachment, name: fileName } },
    ],
  };
  const saved = await page.request.put(
    `/api/v1/workspaces/${personal.id}/documents/${pair.documentId}/body`,
    { data: { contentJson: body } },
  );
  expect(saved.ok(), await saved.text()).toBe(true);
  const sourceFiles = () =>
    committed(
      personal.id,
      `SELECT coalesce(json_agg(json_build_object('id',id,'key',storage_key) ORDER BY id),'[]'::json) FROM fvoci.attachments WHERE workspace_id='${personal.id}'`,
    );
  const before = sourceFiles();

  await page.goto(`/w/${personal.slug}/${pair.documentDisplayId}`);
  const dialog = await openTransfer(page);
  const copyPreview = previewed(page);
  await choose(dialog, "팀에 사본 공개", "PUBF 공개 프로젝트");
  await expect(dialog.getByText("첨부 파일 1개")).toBeVisible();
  // What travels and what stays: the file is copied with a new id and the
  // dialog shows exactly the server's dispositions.
  const copyDispositions = await expectDisclosure(dialog, await copyPreview);
  expect(copyDispositions).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ item: "attachment", outcome: "copied_new_id", count: 1 }),
    ]),
  );
  await expect(dialog.getByText("원본은 개인 공간에 남습니다.")).toBeVisible();
  const committedCopy = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers") && response.request().method() === "POST",
  );
  await dialog.getByRole("button", { name: "팀에 사본 공개", exact: true }).click();
  const response = await committedCopy;
  expect(response.status(), await response.text()).toBe(200);
  const copied = transferSchema.parse(await response.json());
  expect(copied.workspaceId).toBe(team.id);
  expect(copied.documentId).not.toBe(pair.documentId);
  const copyTask = copied.taskId ?? "";
  expect(copyTask).not.toBe(pair.taskId);

  // Committed state (restricted role): one new attachment under the copied
  // task, a different id and key; the copied body names the new id only;
  // the private original's files are unchanged.
  const teamFiles = z
    .array(z.object({ id: z.string().uuid(), task: z.string().uuid(), key: z.string() }))
    .parse(
      committed(
        team.id,
        `SELECT coalesce(json_agg(json_build_object('id',id,'task',task_id,'key',storage_key)),'[]'::json) FROM fvoci.attachments WHERE workspace_id='${team.id}' AND task_id='${copyTask}'`,
      ),
    );
  expect(teamFiles).toHaveLength(1);
  const copiedFile = teamFiles[0];
  if (!copiedFile) throw new Error("copied attachment missing");
  expect(copiedFile.id).not.toBe(attachment);
  const copiedBody = JSON.stringify(
    committed(
      team.id,
      `SELECT content_json FROM fvoci.documents WHERE workspace_id='${team.id}' AND id='${copied.documentId}'`,
    ),
  );
  expect(copiedBody).toContain(copiedFile.id);
  expect(copiedBody).not.toContain(attachment);
  expect(sourceFiles()).toEqual(before);

  // A fresh teammate (its own user, created here) opens the copied task,
  // sees the file and downloads the exact bytes; the private original stays
  // closed to them.
  const { context, page: teammate } = await memberContext(browser, testInfo, {
    email: `${token}-member@example.com`,
    password: member.password,
  });
  try {
    await teammate.goto(`/w/tracer/${project.key}-${String(copied.taskNumber)}`);
    await expect(teammate.getByText(fileName).first()).toBeVisible();
    const download = await teammate.request.get(
      `/api/v1/workspaces/${team.id}/attachments/${copiedFile.id}/download`,
    );
    expect(download.status()).toBe(200);
    expect(Buffer.from(await download.body()).equals(bytes)).toBe(true);
    const denied = await teammate.request.get(
      `/api/v1/workspaces/${personal.id}/attachments/${attachment}/download`,
    );
    expect([403, 404]).toContain(denied.status());
  } finally {
    await context.close();
  }
  // The owner can still download the private original exactly.
  const original = await page.request.get(
    `/api/v1/workspaces/${personal.id}/attachments/${attachment}/download`,
  );
  expect(original.status()).toBe(200);
  expect(Buffer.from(await original.body()).equals(bytes)).toBe(true);
});
