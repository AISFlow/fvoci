import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import path from "node:path";
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
const meiliTaskSchema = z
  .object({
    uid: z.number(),
    indexUid: z.string().nullable().optional(),
    status: z.string(),
    type: z.string(),
    enqueuedAt: z.string(),
    startedAt: z.string().nullable().optional(),
    finishedAt: z.string().nullable().optional(),
    duration: z.string().nullable().optional(),
    details: z.record(z.string(), z.unknown()).nullable().optional(),
    error: z.object({ code: z.string() }).passthrough().nullable().optional(),
  })
  .passthrough();
/**
 * Diagnostic only: this run's isolated Meilisearch tasks enqueued from 10 s
 * before the poll began, read through the harness's own task API with the
 * run's generated key (in memory; never recorded). Called after the poll has
 * settled; waits at most 10 s for those tasks to finish so a task finishing
 * just after the poll is still timed. Records only sanitized task facts:
 * uid, index, type, status, times, duration, numeric document counts and an
 * error code. Never throws.
 */
async function observeMeiliTasks(since: Date) {
  const url = process.env.FVOCI_MEILI_URL;
  const key = process.env.FVOCI_MEILI_KEY;
  if (!url || !key) return { unavailable: "harness Meilisearch URL/key not present" };
  const after = new Date(since.getTime() - 10_000).toISOString();
  const deadline = Date.now() + 10_000;
  try {
    for (;;) {
      const response = await fetch(
        `${url}/tasks?afterEnqueuedAt=${encodeURIComponent(after)}&limit=100`,
        { headers: { Authorization: `Bearer ${key}` }, signal: AbortSignal.timeout(2000) },
      );
      if (!response.ok) return { error: `tasks HTTP ${String(response.status)}` };
      const results = z
        .object({ results: z.array(meiliTaskSchema) })
        .parse(await response.json()).results;
      const pending = results.some((task) => !task.finishedAt);
      if (!pending || Date.now() > deadline)
        return {
          readAt: new Date().toISOString(),
          pendingAtRead: pending,
          tasks: results.map((task) => ({
            uid: task.uid,
            indexUid: task.indexUid ?? null,
            type: task.type,
            status: task.status,
            enqueuedAt: task.enqueuedAt,
            startedAt: task.startedAt ?? null,
            finishedAt: task.finishedAt ?? null,
            duration: task.duration ?? null,
            counts: Object.fromEntries(
              Object.entries(task.details ?? {}).filter(([, value]) => typeof value === "number"),
            ),
            errorCode: task.error?.code ?? null,
          })),
        };
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  } catch (error) {
    return { error: error instanceof Error ? error.name : "task read failed" };
  }
}
/**
 * Diagnostic observer for a search-index outbox poll (test only). Each call
 * records, through the same restricted committed() role and tenant, the exact
 * event rows of the targets with their processed flags, the snapshot xmin and
 * this role's backends holding an xid or xmin. It never throws, so it cannot
 * change the poll; the poll's own predicate is unchanged.
 */
function outboxObserver(workspaceId: string, targets: string[], verbs: string[]) {
  for (const id of [workspaceId, ...targets]) z.string().uuid().parse(id);
  const inTargets = targets.map((id) => `'${id}'`).join(",");
  const inVerbs = verbs.map((verb) => `'${verb.replace(/[^a-z.]/g, "")}'`).join(",");
  const observations: unknown[] = [];
  const startedAt = new Date();
  const observe = () => {
    try {
      observations.push(
        committed(
          workspaceId,
          `SELECT json_build_object('at',clock_timestamp(),'tenant',current_setting('app.tenant_id',true),
          'self',pg_backend_pid(),'snapshotXmin',pg_snapshot_xmin(pg_current_snapshot())::text,
          'rows',(SELECT count(*) FROM fvoci.events WHERE target_id IN (${inTargets}) AND verb IN (${inVerbs})),
          'distinctTargets',(SELECT count(DISTINCT target_id) FROM fvoci.events WHERE target_id IN (${inTargets}) AND verb IN (${inVerbs})),
          'events',(SELECT coalesce(json_agg(json_build_object('id',id,'target',target_id,'verb',verb,'xact',xact::text,'seq',seq,
            'processed',fvoci.app_outbox_is_processed('search-index',id)) ORDER BY xact,seq),'[]'::json)
            FROM fvoci.events WHERE target_id IN (${inTargets}) AND verb IN (${inVerbs})),
          'backends',(SELECT coalesce(json_agg(json_build_object('pid',pid,'state',state,'xactStart',xact_start,
            'backendXid',backend_xid::text,'backendXmin',backend_xmin::text,'wait',wait_event_type) ORDER BY pid),'[]'::json)
            FROM pg_stat_activity WHERE datname=current_database() AND (backend_xid IS NOT NULL OR backend_xmin IS NOT NULL)))`,
        ),
      );
    } catch (error) {
      observations.push({
        at: new Date().toISOString(),
        error: error instanceof Error ? error.message : "observation failed",
      });
    }
  };
  const record = async (testInfo: TestInfo, label: string) => {
    const meiliTasks = await observeMeiliTasks(startedAt);
    const body = JSON.stringify({ workspaceId, targets, verbs, observations, meiliTasks }, null, 1);
    await testInfo.attach(`outbox-observer-${label}`, { body, contentType: "application/json" });
    const dir = process.env.FVOCI_W2_DIAG_DIR;
    if (dir) writeFileSync(path.join(dir, `${label}-${String(Date.now())}.json`), body);
  };
  return { observe, record };
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
  const body = JSON.stringify({ test: testInfo.title, observed, ledger }, null, 1);
  await testInfo.attach("restricted-role-and-service-witness", {
    body,
    contentType: "application/json",
  });
  // Durable copy when the runner provides its diagnostics directory (the same
  // sanitized body: no environment, credentials or session values).
  const dir = process.env.FVOCI_W2_DIAG_DIR;
  if (dir)
    writeFileSync(path.join(dir, `witness-${testInfo.testId}-${String(Date.now())}.json`), body);
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
  documentTitle: z.string(),
  taskTitle: z.string().nullable(),
  dispositions: z.array(z.object({ item: z.string(), outcome: z.string(), count: z.number() })),
});
function previewed(page: Page) {
  return page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers/preview") &&
      response.request().method() === "POST",
  );
}
// The ko translation contract the dialog renders (personalTransfer.files /
// activities / history and personalTransfer.outcome.*).
const OUTCOME_TEXT: Record<string, string> = {
  moved: "같은 ID로 팀에 이동",
  copied_new_id: "새 ID로 팀에 복사",
  retained_private: "개인 공간에 비공개로 남음",
  not_included: "포함하지 않음",
};
type DisclosureRow = { item: string; outcome: string; outcomeText: string };
function expectedRows(preview: z.infer<typeof dispositionsSchema>): DisclosureRow[] {
  return preview.dispositions.map(({ item, outcome, count }) => ({
    item:
      item === "document"
        ? preview.documentTitle
        : item === "task"
          ? (preview.taskTitle ?? "")
          : item === "attachment"
            ? `첨부 파일 ${String(count)}개`
            : item === "activity"
              ? `작업 기록 ${String(count)}개`
              : item === "time_entry"
                ? `시간 기록 ${String(count)}개`
                : item === "timer"
                  ? `내 측정 기록 ${String(count)}개`
                  : `편집 이력 ${String(count)}개`,
    outcome,
    outcomeText: OUTCOME_TEXT[outcome] ?? `missing outcome text for ${outcome}`,
  }));
}
/**
 * The dialog discloses exactly the server's preview, row by row and in order:
 * each <dt> names its item (title or "{count}" label) and the <dd> after it
 * carries that item's outcome and outcome text.
 */
async function expectDisclosure(
  dialog: ReturnType<Page["getByRole"]>,
  preview: Awaited<ReturnType<typeof previewed>>,
) {
  expect(preview.status(), await preview.text()).toBe(200);
  const parsed = dispositionsSchema.parse(await preview.json());
  const { dispositions } = parsed;
  expect(dispositions.length).toBeGreaterThan(0);
  // The one list whose rows carry outcomes (a plain CSS :has(), relative to
  // the dialog).
  const shown = await dialog.locator("dl:has(> dd[data-outcome])").evaluate((list) => {
    const rows: { item: string; outcome: string; outcomeText: string }[] = [];
    for (const term of list.querySelectorAll("dt")) {
      const value = term.nextElementSibling;
      rows.push({
        item: term.textContent.trim(),
        outcome: value?.tagName === "DD" ? (value.getAttribute("data-outcome") ?? "") : "",
        outcomeText: value?.tagName === "DD" ? value.textContent.trim() : "",
      });
    }
    return rows;
  });
  const expected = expectedRows(parsed);
  expect(shown).toEqual(expected);
  // Negative controls: the comparison distinguishes a swapped outcome and a
  // changed count from what is shown.
  const swapped = parsed.dispositions.map((row) => ({
    ...row,
    outcome: row.outcome === "moved" ? "copied_new_id" : "moved",
  }));
  expect(shown).not.toEqual(expectedRows({ ...parsed, dispositions: swapped }));
  const counted = parsed.dispositions.map((row) => ({ ...row, count: row.count + 1 }));
  if (parsed.dispositions.some((row) => !["document", "task"].includes(row.item)))
    expect(shown).not.toEqual(expectedRows({ ...parsed, dispositions: counted }));
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
    const copyObserver = outboxObserver(
      team.id,
      [copyTask, replay.documentId],
      ["task.created", "document.created"],
    );
    try {
      await expect
        .poll(() => {
          copyObserver.observe();
          return committed(
            team.id,
            `SELECT (count(DISTINCT target_id)=2 AND COALESCE(bool_and(fvoci.app_outbox_is_processed('search-index',id)),false))::text::json FROM fvoci.events WHERE target_id IN ('${copyTask}','${replay.documentId}') AND verb IN ('task.created','document.created')`,
          );
        })
        .toBe(true);
    } finally {
      await copyObserver.record(testInfo, "copy-team-created");
    }
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
    // Literal rows: both titles disclosed as moved with the same IDs.
    const moveRows = dialog.locator("dt", { hasText: title });
    await expect(moveRows).toHaveCount(2);
    for (const row of await moveRows.all())
      await expect(row.locator("xpath=following-sibling::dd[1]")).toHaveText("같은 ID로 팀에 이동");
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
    const moveTeam = outboxObserver(
      team.id,
      [pair.taskId, pair.documentId],
      ["task.created", "document.created"],
    );
    try {
      await expect
        .poll(() => {
          moveTeam.observe();
          return processed(team.id, "'task.created','document.created'");
        })
        .toBe(true);
    } finally {
      await moveTeam.record(testInfo, "move-team-created");
    }
    const movePersonal = outboxObserver(
      personal.id,
      [pair.taskId, pair.documentId],
      ["task.deleted", "document.deleted"],
    );
    try {
      await expect
        .poll(() => {
          movePersonal.observe();
          return processed(personal.id, "'task.deleted','document.deleted'");
        })
        .toBe(true);
    } finally {
      await movePersonal.record(testInfo, "move-personal-deleted");
    }
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
  // Literal row: the one file is disclosed as copied with a new ID.
  await expect(
    dialog.locator("dt", { hasText: "첨부 파일 1개" }).locator("xpath=following-sibling::dd[1]"),
  ).toHaveText("새 ID로 팀에 복사");
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

const identitySchema = z.object({ userId: z.string().uuid(), sessionId: z.string().uuid() });
const timerReceiptSchema = z.object({ runId: z.string().uuid(), version: z.number().int() });
/** Labelled admin fixture (isolated run DB, local socket): states no current route creates. */
function adminFixture(sql: string): string {
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!adminUrl || !container) throw new Error("isolated PostgreSQL fixture is required");
  return execFileSync(
    "docker",
    [
      "exec",
      "-i",
      container,
      "psql",
      "-X",
      "-qAt",
      "-U",
      "postgres",
      "-d",
      new URL(adminUrl).pathname.slice(1),
      "-v",
      "ON_ERROR_STOP=1",
    ],
    { input: sql, encoding: "utf8", stdio: "pipe" },
  ).trim();
}

test("MOVE discloses and carries the time records and my timer; COPY keeps them private; a timer running elsewhere refuses with no effects", async ({
  page,
}, testInfo) => {
  test.setTimeout(120000);
  await setup(page);
  const token = `w2time${String(Date.now())}`;
  const { personal, pair } = await capturedPair(page, `${token} 측정한 작업`);
  const { team } = await teamProject(page, "PUBR", "workspace");
  await witness(personal.id, testInfo);
  const me = identitySchema.parse(await (await page.request.get("/api/v1/auth/me")).json());
  const timer = async (workspace: string, task: string, body: Record<string, unknown>) => {
    const response = await page.request.post(
      `/api/v1/workspaces/${workspace}/tasks/${task}/timer`,
      {
        data: {
          expectedActorId: me.userId,
          expectedSessionId: me.sessionId,
          requestId: crypto.randomUUID(),
          runId: null,
          ...body,
        },
      },
    );
    expect(response.status(), await response.text()).toBe(200);
    return timerReceiptSchema.parse(await response.json());
  };
  // Real W5 route: one stopped run (its segment projects one time record).
  const started = await timer(personal.id, pair.taskId, { operation: "start", expectedVersion: 0 });
  await new Promise((resolve) => setTimeout(resolve, 1100));
  await timer(personal.id, pair.taskId, {
    operation: "stop",
    expectedVersion: started.version,
    runId: started.runId,
  });
  const timeRows = (workspace: string) =>
    committed(
      workspace,
      `SELECT count(*)::text::json FROM fvoci.time_entries WHERE task_id='${pair.taskId}'`,
    );
  expect(timeRows(personal.id)).toBe(1);

  // COPY review: both stay with the private original; no time privacy line.
  await page.goto(`/w/${personal.slug}/${pair.documentDisplayId}`);
  let dialog = await openTransfer(page);
  let previewing = previewed(page);
  await choose(dialog, "팀에 사본 공개", "PUBR 공개 프로젝트");
  await expectDisclosure(dialog, await previewing);
  for (const label of ["시간 기록 1개", "내 측정 기록 1개"])
    await expect(
      dialog.locator("dt", { hasText: label }).locator("xpath=following-sibling::dd[1]"),
    ).toHaveText("개인 공간에 비공개로 남음");
  await expect(dialog.getByText("시간 기록은 이 프로젝트에 접근할 수 있는")).toHaveCount(0);
  await dialog.getByRole("button", { name: "취소", exact: true }).click();
  // Cancel returns to the form; close the dialog before opening it again.
  await dialog.getByRole("button", { name: "닫기", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(timeRows(personal.id)).toBe(1);

  // MOVE review: both move with the same IDs; the privacy line is shown.
  dialog = await openTransfer(page);
  previewing = previewed(page);
  await choose(dialog, "팀으로 이동", "PUBR 공개 프로젝트");
  await expectDisclosure(dialog, await previewing);
  for (const label of ["시간 기록 1개", "내 측정 기록 1개"])
    await expect(
      dialog.locator("dt", { hasText: label }).locator("xpath=following-sibling::dd[1]"),
    ).toHaveText("같은 ID로 팀에 이동");
  await expect(
    dialog.getByText(
      "시간 기록은 이 프로젝트에 접근할 수 있는 사람에게 보입니다. 내 측정 기록과 수정 사유는 나에게만 보입니다.",
    ),
  ).toBeVisible();
  const moving = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers") && response.request().method() === "POST",
  );
  await dialog.getByRole("button", { name: "팀으로 이동", exact: true }).click();
  const moved = await moving;
  expect(moved.status(), await moved.text()).toBe(200);
  // The MOVE retires the private source: its page leaves the moved
  // document's route, and the client still settles its durable command (no
  // unconfirmed request remains to recover).
  await expect(page).not.toHaveURL(new RegExp(`/${pair.documentDisplayId}$`));
  await expect(
    page.getByRole("button", { name: "미확인 팀 공개·이동 요청 확인", exact: true }),
  ).toHaveCount(0);
  expect(timeRows(team.id)).toBe(1);
  expect(timeRows(personal.id)).toBe(0);

  // A second pair with a released legacy open record (labelled admin
  // fixture: no current route creates an open record), then a timer running
  // on the moved task: moving the second pair would reinsert an open record
  // the 048 trigger refuses, so it refuses before any effect.
  const second = await capturedPair(page, `${token} 열린 기록`);
  const open = crypto.randomUUID();
  adminFixture(
    `BEGIN; SELECT set_config('app.self_user_id','${me.userId}',true); INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,note) VALUES('${open}','${personal.id}','${second.pair.taskId}','${me.userId}',now()-interval '1 hour','열린 기록'); COMMIT;`,
  );
  const release = await page.request.post("/api/v1/me/task-timer/legacy-release", {
    data: {
      expectedActorId: me.userId,
      expectedSessionId: me.sessionId,
      requestId: crypto.randomUUID(),
      timeEntryId: open,
    },
  });
  expect(release.status(), await release.text()).toBe(200);
  const running = await timer(team.id, pair.taskId, { operation: "start", expectedVersion: 0 });
  const secondGraph = () => graph(personal.id, second.pair.documentId, second.pair.taskId);
  const secondBefore = secondGraph();
  const secondTime = () =>
    committed(
      personal.id,
      `SELECT count(*)::text::json FROM fvoci.time_entries WHERE task_id='${second.pair.taskId}' AND ended_at IS NULL`,
    );
  expect(secondTime()).toBe(1);
  await page.goto(`/w/${personal.slug}/${second.pair.documentDisplayId}`);
  dialog = await openTransfer(page);
  const refused = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers/preview") &&
      response.request().method() === "POST",
  );
  await choose(dialog, "팀으로 이동", "PUBR 공개 프로젝트");
  expect((await refused).status()).toBe(409);
  await expect(dialog.getByRole("alert")).toContainText(
    "다른 작업에서 측정 중이거나 끝나지 않은 시간 기록이 있어 지금은 이동할 수 없습니다. 그 측정을 끝낸 뒤 다시 시도하세요.",
  );
  expect(secondGraph()).toEqual(secondBefore);
  expect(secondTime()).toBe(1);
  // Acting on the message: stop that timer, and the same pair moves with its
  // open record.
  await timer(team.id, pair.taskId, {
    operation: "stop",
    expectedVersion: running.version,
    runId: running.runId,
  });
  await dialog.getByRole("button", { name: "공개 내용 확인", exact: true }).click();
  await expect(
    dialog.locator("dt", { hasText: "시간 기록 1개" }).locator("xpath=following-sibling::dd[1]"),
  ).toHaveText("같은 ID로 팀에 이동");
  const secondMoving = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers") && response.request().method() === "POST",
  );
  await dialog.getByRole("button", { name: "팀으로 이동", exact: true }).click();
  expect((await secondMoving).status()).toBe(200);
  expect(
    committed(
      team.id,
      `SELECT count(*)::text::json FROM fvoci.time_entries WHERE task_id='${second.pair.taskId}' AND ended_at IS NULL`,
    ),
  ).toBe(1);
});

const updateLogSchema = z.array(
  z.object({ seq: z.number(), op: z.string().uuid(), payload: z.string(), at: z.string() }),
);
const nativeSchema = z.object({
  document: z.object({ content: z.string(), hasToken: z.boolean() }),
  documentState: z.object({ state: z.string() }).passthrough().nullable(),
  documentUpdates: updateLogSchema,
  taskState: z.object({ state: z.string() }).passthrough().nullable(),
  taskUpdates: updateLogSchema,
  revisions: z.array(z.object({ id: z.string().uuid() }).passthrough()),
});
const timerGraphSchema = z.object({
  workspaces: z.array(z.string().uuid()),
  runs: z.array(z.object({ id: z.string().uuid() }).passthrough()),
  segments: z.array(z.object({ id: z.string().uuid(), run: z.string().uuid() }).passthrough()),
});
const teamTaskSchema = z.object({ id: z.string().uuid(), number: z.number().int() });
const workflowSchema = z.object({ statuses: z.array(z.object({ id: z.string() }).passthrough()) });

/**
 * One MOVE observed by another signed-in session of the owner that keeps two
 * pages mounted: the personal task page and one destination view (`tasks` or
 * `board`) of a fresh team project that already shows a task. After the MOVE
 * the source is denied with no editable stale form, and the destination view
 * shows the moved task in place (same URL, page marker kept).
 */
async function moveWithOpenViews(
  page: Page,
  browser: Browser,
  testInfo: TestInfo,
  key: string,
  destination: "tasks" | "board",
) {
  const token = `w2views${destination}${String(Date.now())}`;
  const title = `${token} 열린 화면 작업`;
  const { personal, pair } = await capturedPair(page, title);
  const { team, project } = await teamProject(page, key, "workspace");
  const workflow = workflowSchema.parse(
    await (
      await page.request.get(`/api/v1/workspaces/${team.id}/projects/${project.id}/workflow`)
    ).json(),
  );
  const seeded = await page.request.post(
    `/api/v1/workspaces/${team.id}/projects/${project.id}/tasks`,
    { data: { title: `${token} 기존 팀 작업`, type: "task", statusId: workflow.statuses[0]?.id } },
  );
  expect(seeded.status(), await seeded.text()).toBe(201);
  const existing = teamTaskSchema.parse(await seeded.json());

  const other = await browser.newContext({ baseURL: testInfo.project.use.baseURL });
  try {
    const detail = await other.newPage();
    await login(detail, owner.email, owner.password);
    await detail.goto(`/w/${personal.slug}/${pair.taskDisplayId}`);
    await expect(detail.getByTestId("task-edit-title")).toHaveValue(title);
    const view = await other.newPage();
    await view.goto(`/w/tracer/${project.key}/${destination}`);
    const boardPanel = view.locator('section[data-testid="collection-board"]');
    const row = (id: string, number: number) =>
      destination === "tasks"
        ? view.getByTestId(`task-row-${id}`)
        : boardPanel.getByTestId(`collection-card-${project.key}-${String(number)}`);
    await expect(row(existing.id, existing.number)).toBeVisible();
    // Page-state markers: a reload or full navigation would drop them.
    const views = [detail, view];
    const urls = views.map((mounted) => mounted.url());
    for (const mounted of views)
      await mounted.evaluate((mark) => {
        (window as unknown as { w2ViewMark?: string }).w2ViewMark = mark;
      }, token);

    const before = graph(personal.id, pair.documentId, pair.taskId);
    await page.goto(`/w/${personal.slug}/${pair.documentDisplayId}`);
    const dialog = await openTransfer(page);
    const movePreview = previewed(page);
    await choose(dialog, "팀으로 이동", `${key} 공개 프로젝트`);
    await expectDisclosure(dialog, await movePreview);
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
    const teamGraph = graph(team.id, pair.documentId, pair.taskId);
    expect(teamGraph).toMatchObject({ documents: 1, tasks: 1, origins: 1, assignees: 1 });
    const personalGraph = graph(personal.id, pair.documentId, pair.taskId);
    expect(personalGraph).toMatchObject({
      documents: 0,
      tasks: 0,
      origins: 0,
      assignees: 0,
      receipts: before.receipts + 1,
    });
    // Exactly one MOVE: one deleted event per target in the source tenant and
    // one created event per target in the team.
    const once = (verbs: string[], verb: string) =>
      verbs.filter((row) => row.startsWith(`${verb}:`)).length;
    for (const verb of ["task.deleted", "document.deleted"])
      expect(once(personalGraph.eventVerbs, verb), verb).toBe(1);
    for (const verb of ["task.created", "document.created"])
      expect(once(teamGraph.eventVerbs, verb), verb).toBe(1);

    // Server reads from the open session: the old source is gone, the team
    // copy of the same IDs is readable.
    for (const suffix of [`tasks/${pair.taskId}`, `documents/${pair.documentId}`]) {
      const denied = await detail.request.get(`/api/v1/workspaces/${personal.id}/${suffix}`);
      expect([403, 404], suffix).toContain(denied.status());
    }
    const teamRead = await detail.request.get(`/api/v1/workspaces/${team.id}/tasks/${pair.taskId}`);
    expect(teamRead.status()).toBe(200);

    // Both mounted pages converge through their own stream/refetch, in place.
    await expect(
      detail.getByRole("alert").filter({ hasText: "태스크를 찾을 수 없습니다" }),
    ).toBeVisible();
    await expect(detail.getByTestId("task-edit-title")).toHaveCount(0);
    await expect(row(pair.taskId, moved.taskNumber ?? 0)).toContainText(title);
    await expect(row(existing.id, existing.number)).toBeVisible();
    for (const [index, mounted] of views.entries()) {
      expect(mounted.url()).toBe(urls[index]);
      expect(
        await mounted.evaluate(() => (window as unknown as { w2ViewMark?: string }).w2ViewMark),
      ).toBe(token);
    }
    // Watching changed nothing: still exactly the one MOVE.
    expect(graph(team.id, pair.documentId, pair.taskId)).toEqual(teamGraph);
  } finally {
    await other.close();
  }
}

test("MOVE with views open elsewhere: the personal task page is denied in place, and a mounted team task list, then a mounted board, converge without reloading", async ({
  page,
  browser,
}, testInfo) => {
  test.setTimeout(120000);
  await setup(page);
  const personal = workspaceSchema.parse(
    await (await page.request.post("/api/v1/me/personal-workspace")).json(),
  );
  await witness(personal.id, testInfo);
  // Sequential: each MOVE has its own pair, project and observing context.
  await moveWithOpenViews(page, browser, testInfo, "PUBV", "tasks");
  await moveWithOpenViews(page, browser, testInfo, "PUBW", "board");
});

test("MOVE whose committed success is lost replays the same request once: same IDs, file, time records and history, no duplicate effects", async ({
  page,
}, testInfo) => {
  test.setTimeout(120000);
  await setup(page);
  const token = `w2lostmove${String(Date.now())}`;
  const title = `${token} 응답 유실 이동`;
  const { personal, pair } = await capturedPair(page, title);
  const { team, project } = await teamProject(page, "PUBL", "workspace");
  await witness(personal.id, testInfo);
  const me = identitySchema.parse(await (await page.request.get("/api/v1/auth/me")).json());
  // One file shown in the document body, and one stopped run (real W5 route)
  // projecting one time record.
  const bytes = Buffer.from(Array.from({ length: 70_000 }, (_, i) => (i * 7919) % 251));
  const fileName = `${token} 증빙.bin`;
  const attachment = await uploadTaskFile(page, personal.id, pair.taskId, fileName, bytes);
  const saved = await page.request.put(
    `/api/v1/workspaces/${personal.id}/documents/${pair.documentId}/body`,
    {
      data: {
        contentJson: {
          type: "doc",
          content: [
            { type: "paragraph", content: [{ type: "text", text: `${token} 이동할 본문` }] },
            { type: "attachment", attrs: { id: attachment, name: fileName } },
          ],
        },
      },
    },
  );
  expect(saved.ok(), await saved.text()).toBe(true);
  const timer = async (body: Record<string, unknown>) => {
    const response = await page.request.post(
      `/api/v1/workspaces/${personal.id}/tasks/${pair.taskId}/timer`,
      {
        data: {
          expectedActorId: me.userId,
          expectedSessionId: me.sessionId,
          requestId: crypto.randomUUID(),
          runId: null,
          ...body,
        },
      },
    );
    expect(response.status(), await response.text()).toBe(200);
    return timerReceiptSchema.parse(await response.json());
  };
  const started = await timer({ operation: "start", expectedVersion: 0 });
  await new Promise((resolve) => setTimeout(resolve, 1100));
  await timer({ operation: "stop", expectedVersion: started.version, runId: started.runId });

  const ids = (workspace: string, table: "time_entries" | "attachments") =>
    committed(
      workspace,
      `SELECT coalesce(json_agg(id ORDER BY id),'[]'::json) FROM fvoci.${table} WHERE task_id='${pair.taskId}'`,
    );
  const history = (workspace: string) =>
    committed(
      workspace,
      `SELECT json_build_object(
      'revisions',(SELECT count(*) FROM fvoci.revisions WHERE (target_kind='document' AND target_id='${pair.documentId}') OR (target_kind='task' AND target_id='${pair.taskId}')),
      'documentUpdates',(SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id='${pair.documentId}'),
      'taskUpdates',(SELECT count(*) FROM fvoci.task_collab_updates WHERE task_id='${pair.taskId}'))`,
    );
  // Exact native state of this document and task in one tenant: current
  // content, stored states, ordered update logs and revisions, as digests
  // and metadata (no parser; rows restricted to the tenant and these IDs).
  const native = (workspace: string) =>
    nativeSchema.parse(
      committed(
        workspace,
        `SELECT json_build_object(
        'document',(SELECT json_build_object('content',md5(content_json::text),'hasToken',position('${token}' in content_json::text)>0) FROM fvoci.documents WHERE workspace_id='${workspace}' AND id='${pair.documentId}'),
        'documentState',(SELECT json_build_object('state',md5(state),'encoding',encoding,'compacted',compacted_at,'updated',updated_at) FROM fvoci.document_states WHERE workspace_id='${workspace}' AND document_id='${pair.documentId}'),
        'documentUpdates',(SELECT coalesce(json_agg(json_build_object('seq',seq,'op',op_id,'payload',md5(payload),'at',created_at) ORDER BY seq),'[]'::json) FROM fvoci.document_collab_updates WHERE workspace_id='${workspace}' AND document_id='${pair.documentId}'),
        'taskState',(SELECT json_build_object('state',md5(state),'encoding',encoding,'compacted',compacted_at,'updated',updated_at,'generation',writer_generation,'cutoff',snapshot_cutoff_seq,'tail',tail_seq) FROM fvoci.task_states WHERE workspace_id='${workspace}' AND task_id='${pair.taskId}'),
        'taskUpdates',(SELECT coalesce(json_agg(json_build_object('seq',seq,'op',op_id,'payload',md5(payload),'at',created_at) ORDER BY seq),'[]'::json) FROM fvoci.task_collab_updates WHERE workspace_id='${workspace}' AND task_id='${pair.taskId}'),
        'revisions',(SELECT coalesce(json_agg(json_build_object('id',id,'kind',target_kind,'snapshot',md5(y_snapshot),'content',md5(content_json::text),'text',md5(text),'reason',reason,'by',created_by,'at',created_at) ORDER BY id),'[]'::json) FROM fvoci.revisions WHERE workspace_id='${workspace}' AND target_id IN ('${pair.documentId}','${pair.taskId}')))`,
      ),
    );
  // My timer rows are actor-scoped (048 self_timer RLS): read through the
  // same restricted role with a transaction-local self user (a validated
  // UUID), canonical and ordered, without the workspace column.
  const actor = z.string().uuid().parse(me.userId);
  // 048 self_timer filters by user only, so each read also names the
  // validated workspace: the source and destination are observed separately.
  const timerRows = (workspace: string) => {
    const tenant = z.string().uuid().parse(workspace);
    return timerGraphSchema.parse(
      committed(
        workspace,
        `SET LOCAL app.self_user_id='${actor}'; SELECT json_build_object(
      'workspaces',(SELECT coalesce(json_agg(DISTINCT workspace_id),'[]'::json) FROM (SELECT workspace_id FROM fvoci.task_timer_runs WHERE workspace_id='${tenant}' AND task_id='${pair.taskId}' UNION ALL SELECT workspace_id FROM fvoci.task_timer_segments WHERE workspace_id='${tenant}' AND task_id='${pair.taskId}') w),
      'runs',(SELECT coalesce(json_agg(json_build_object('id',id,'user',user_id,'task',task_id,'status',status,'version',version,'started',started_at,'stopped',stopped_at,'note',note) ORDER BY id),'[]'::json) FROM fvoci.task_timer_runs WHERE workspace_id='${tenant}' AND task_id='${pair.taskId}'),
      'segments',(SELECT coalesce(json_agg(json_build_object('id',id,'run',run_id,'user',user_id,'task',task_id,'started',started_at,'ended',ended_at,'timeEntry',time_entry_id) ORDER BY id),'[]'::json) FROM fvoci.task_timer_segments WHERE workspace_id='${tenant}' AND task_id='${pair.taskId}'))`,
      ),
    );
  };
  const sourceTimer = timerRows(personal.id);
  expect(sourceTimer.workspaces).toEqual([personal.id]);
  expect(sourceTimer.runs.map((run) => run.id)).toEqual([started.runId]);
  expect(sourceTimer.segments.length).toBeGreaterThan(0);
  for (const segment of sourceTimer.segments) expect(segment.run).toBe(started.runId);
  const sourceTime = ids(personal.id, "time_entries");
  expect(sourceTime).toHaveLength(1);
  expect(ids(personal.id, "attachments")).toEqual([attachment]);
  const before = graph(personal.id, pair.documentId, pair.taskId);

  await page.goto(`/w/${personal.slug}/${pair.documentDisplayId}`);
  const dialog = await openTransfer(page);
  const movePreview = previewed(page);
  await choose(dialog, "팀으로 이동", "PUBL 공개 프로젝트");
  await expectDisclosure(dialog, await movePreview);
  for (const label of ["첨부 파일 1개", "시간 기록 1개"])
    await expect(
      dialog.locator("dt", { hasText: label }).locator("xpath=following-sibling::dd[1]"),
    ).toHaveText("같은 ID로 팀에 이동");
  let original: unknown;
  const sent: { bytes: Buffer | null } = { bytes: null };
  let originalCookie: string | null = null;
  let lostSeen!: (value: z.infer<typeof transferSchema>) => void;
  const lostResponse = new Promise<z.infer<typeof transferSchema>>((resolve) => {
    lostSeen = resolve;
  });
  await page.route("**/personal-transfers", async (route) => {
    original = route.request().postDataJSON() as unknown;
    sent.bytes = route.request().postDataBuffer();
    originalCookie = await route.request().headerValue("cookie");
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    const body = transferSchema.parse(await response.json());
    // The actual Rust transaction committed; only the browser response is lost.
    await route.abort("failed");
    lostSeen(body);
  });
  await dialog.getByRole("button", { name: "팀으로 이동", exact: true }).click();
  const lost = await lostResponse;
  await page.unroute("**/personal-transfers");
  expect(lost).toMatchObject({
    workspaceId: team.id,
    projectId: project.id,
    documentId: pair.documentId,
    taskId: pair.taskId,
    replayed: false,
  });
  // Committed exactly once: the same IDs moved with the file and time record.
  expect(graph(personal.id, pair.documentId, pair.taskId)).toMatchObject({
    documents: 0,
    tasks: 0,
    origins: 0,
    assignees: 0,
    receipts: before.receipts + 1,
  });
  const committedTeam = graph(team.id, pair.documentId, pair.taskId);
  expect(committedTeam).toMatchObject({ documents: 1, tasks: 1, origins: 1, assignees: 1 });
  expect(ids(team.id, "time_entries")).toEqual(sourceTime);
  expect(ids(personal.id, "time_entries")).toEqual([]);
  expect(ids(team.id, "attachments")).toEqual([attachment]);
  expect(ids(personal.id, "attachments")).toEqual([]);
  // The run and its segments moved as the same rows; only the workspace maps.
  const movedTimer = timerRows(team.id);
  expect(movedTimer).toEqual({ ...sourceTimer, workspaces: [team.id] });
  expect(timerRows(personal.id)).toEqual({ workspaces: [], runs: [], segments: [] });
  const committedHistory = history(team.id);
  // The moved document carries its current content and native updates.
  const committedNative = native(team.id);
  expect(committedNative.document).toMatchObject({ hasToken: true });
  expect(
    committedNative.documentUpdates.length + (committedNative.documentState ? 1 : 0),
  ).toBeGreaterThan(0);
  // The source tenant committed exactly one deleted event per target.
  const committedPersonal = graph(personal.id, pair.documentId, pair.taskId);
  for (const verb of ["task.deleted", "document.deleted"])
    expect(committedPersonal.eventVerbs.filter((row) => row.startsWith(`${verb}:`))).toHaveLength(
      1,
    );

  // A lost MOVE success can leave no source route: from a page without one,
  // the shell's recovery entry replays the stored command.
  await page.goto("/w/tracer/my-tasks");
  const entry = page.getByRole("button", { name: "미확인 팀 공개·이동 요청 확인", exact: true });
  await entry.click();
  const recovery = page.getByRole("dialog", { name: "미확인 팀 공개·이동 요청 확인", exact: true });
  await expect(recovery.getByRole("button", { name: "같은 요청 다시 확인" })).toBeEnabled();
  const retry = page.waitForResponse(
    (response) =>
      response.url().endsWith("/personal-transfers") && response.request().method() === "POST",
  );
  await recovery.getByRole("button", { name: "같은 요청 다시 확인" }).click();
  const retried = await retry;
  expect(retried.request().postDataJSON()).toEqual(original);
  // Byte-identical, not only JSON-equal.
  const originalBytes = sent.bytes;
  if (!originalBytes) throw new Error("original request body was not captured");
  const retriedBytes = retried.request().postDataBuffer();
  expect(retriedBytes?.equals(originalBytes)).toBe(true);
  // Same session (compared in memory only; never recorded).
  expect(await retried.request().headerValue("cookie")).toBe(originalCookie);
  const replay = transferSchema.parse(await retried.json());
  expect(replay).toEqual({ ...lost, replayed: true });
  // On success the same dialog is retitled from the recovery step back to
  // its normal title (as in the COPY case).
  const completed = page.getByRole("dialog", { name: "팀에 공개하거나 이동", exact: true });
  await expect(completed.getByText("요청이 완료되었습니다.")).toBeVisible();

  // No second transfer and no duplicate effects.
  expect(graph(personal.id, pair.documentId, pair.taskId).receipts).toBe(before.receipts + 1);
  expect(graph(team.id, pair.documentId, pair.taskId)).toEqual(committedTeam);
  expect(ids(team.id, "time_entries")).toEqual(sourceTime);
  expect(ids(team.id, "attachments")).toEqual([attachment]);
  expect(history(team.id)).toEqual(committedHistory);
  expect(native(team.id)).toEqual(committedNative);
  expect(graph(personal.id, pair.documentId, pair.taskId)).toEqual(committedPersonal);
  expect(timerRows(team.id)).toEqual(movedTimer);
  expect(timerRows(personal.id)).toEqual({ workspaces: [], runs: [], segments: [] });
  for (const verb of ["task.created", "document.created"])
    expect(committedTeam.eventVerbs.filter((row) => row.startsWith(`${verb}:`))).toHaveLength(1);
  const download = await page.request.get(
    `/api/v1/workspaces/${team.id}/attachments/${attachment}/download`,
  );
  expect(download.status()).toBe(200);
  expect(Buffer.from(await download.body()).equals(bytes)).toBe(true);

  // Search: once the consumer processed the one created pair (no TTL wait),
  // the team finds the same IDs once and personal search no longer does.
  const observer = outboxObserver(
    team.id,
    [pair.taskId, pair.documentId],
    ["task.created", "document.created"],
  );
  try {
    await expect
      .poll(() => {
        observer.observe();
        return committed(
          team.id,
          `SELECT (count(DISTINCT target_id)=2 AND COALESCE(bool_and(fvoci.app_outbox_is_processed('search-index',id)),false))::text::json FROM fvoci.events WHERE target_id IN ('${pair.taskId}','${pair.documentId}') AND verb IN ('task.created','document.created')`,
        );
      })
      .toBe(true);
  } finally {
    await observer.record(testInfo, "lost-move-team-created");
  }
  const personalObserver = outboxObserver(
    personal.id,
    [pair.taskId, pair.documentId],
    ["task.deleted", "document.deleted"],
  );
  try {
    await expect
      .poll(() => {
        personalObserver.observe();
        return committed(
          personal.id,
          `SELECT (count(DISTINCT target_id)=2 AND COALESCE(bool_and(fvoci.app_outbox_is_processed('search-index',id)),false))::text::json FROM fvoci.events WHERE target_id IN ('${pair.taskId}','${pair.documentId}') AND verb IN ('task.deleted','document.deleted')`,
        );
      })
      .toBe(true);
  } finally {
    await personalObserver.record(testInfo, "lost-move-personal-deleted");
  }
  const found = async (url: string) => {
    const response = await page.request.get(url);
    expect(response.status(), url).toBe(200);
    return searchSchema.parse(await response.json()).items.map((item) => item.id);
  };
  await expect
    .poll(async () => found(`/api/v1/workspaces/${team.id}/search?q=${token}`))
    .toEqual(expect.arrayContaining([pair.taskId, pair.documentId]));
  const teamIds = await found(`/api/v1/workspaces/${team.id}/search?q=${token}`);
  expect(teamIds.filter((id) => id === pair.taskId)).toHaveLength(1);
  expect(teamIds.filter((id) => id === pair.documentId)).toHaveLength(1);
  const personalIds = await found(`/api/v1/workspaces/${personal.id}/search?q=${token}`);
  expect(personalIds).not.toContain(pair.taskId);
  expect(personalIds).not.toContain(pair.documentId);

  // The recovered command is settled: closing leaves no unconfirmed request,
  // also after a reload.
  await completed.getByRole("button", { name: "닫기", exact: true }).click();
  await expect(completed).toBeHidden();
  await expect(entry).toHaveCount(0);
  await page.reload();
  await expect(page.getByTestId("my-tasks")).toBeVisible();
  await expect(entry).toHaveCount(0);
});
