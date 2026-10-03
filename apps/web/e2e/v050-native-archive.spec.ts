import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import type { Editor } from "@tiptap/core";
import JSZip from "jszip";
import * as Y from "yjs";
import { z } from "zod";
import { ownedServerChildEnv } from "../e2e-pending/collab-restart";
import { createE2eUser, login, watchCspViolations } from "./helpers";
import {
  ZOTERO_FIXTURE_KEY,
  ZOTERO_USER_LIBRARY,
  expectAuthoredContent,
  expectMode22Import,
  expectNoUpstreamGet,
  expectPrivateZoteroDenied,
  expectReconnectedSince0,
  expectRestoredDisconnected,
  expectRestoredLibrary,
  expectRetiredReceiptReplay,
} from "./zotero-archive-oracles";

test.describe.configure({ mode: "serial" });
const ids = z.object({ id: z.string().uuid() }).passthrough();
const workspaces = z.object({
  items: z.array(z.object({ id: z.string().uuid(), slug: z.string() }).passthrough()),
});

// A real refused ordinary-project flow. It does not replace the required
// two-installation native history/attachment/peer-edit positive fixture.
test("native archive shows limits and refuses an unsupported member grant without downloading an incomplete artifact", async ({
  page,
}) => {
  const csp = watchCspViolations(page);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("보관 검증");
  await page.getByLabel("이메일").fill("native-owner@example.com");
  await page.getByLabel("비밀번호").fill("nativearchive123");
  await page.getByLabel("워크스페이스 이름").fill("원본 프로젝트 한글 🧪");
  await page.getByLabel("주소(영문)").fill("native-source");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const list = await page.request.get("/api/v1/me/workspaces");
  expect(list.ok()).toBe(true);
  const workspace = workspaces
    .parse(await list.json())
    .items.find((item) => item.slug === "native-source");
  expect(workspace).toBeDefined();
  if (!workspace) throw new Error("ordinary setup workspace missing");
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key: "NATIVE", name: "독립 기대값 프로젝트 🧪", visibility: "private" },
  });
  expect(projectResponse.status()).toBe(201);
  const project = ids
    .extend({ rootDocumentId: z.string().uuid() })
    .parse(await projectResponse.json());
  // Task time, dependencies, saved views and recurring tasks are supported
  // models now; a second person's project grant (team/multi-author) is still
  // refused.
  createE2eUser("native-member@example.com", "nativemember123", "멤버", {
    workspaceSlug: "native-source",
    membershipRole: "member",
  });
  const members = await page.request.get(`/api/v1/workspaces/${workspace.id}/members`);
  expect(members.ok()).toBe(true);
  const member = z
    .object({ items: z.array(z.object({ email: z.string(), userId: z.string() }).passthrough()) })
    .passthrough()
    .parse(await members.json())
    .items.find((item) => item.email === "native-member@example.com");
  if (!member) throw new Error("ordinary second member missing");
  const grant = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${project.id}/members`,
    { data: { userId: member.userId, role: "viewer" } },
  );
  expect(grant.status(), await grant.text()).toBe(201);
  await page.goto("/w/native-source/settings");
  await page.getByLabel("보관할 프로젝트").selectOption(project.id);
  const source = page
    .locator(".native-archive")
    .filter({ has: page.getByRole("button", { name: "네이티브 보관 파일 다운로드" }) });
  await expect(
    source.getByText(/같은 데이터베이스의 빈 워크스페이스만으로는 충분하지 않습니다/),
  ).toBeVisible();
  await expect(source.getByText(/SHA-256 해시는 파일 변경 여부/)).toBeVisible();
  let downloads = 0;
  page.on("download", () => {
    downloads++;
  });
  const response = page.waitForResponse((response) =>
    response.url().endsWith(`/projects/${project.id}/native-archive`),
  );
  await source.getByRole("button", { name: "네이티브 보관 파일 다운로드" }).click();
  expect((await response).status()).toBe(422);
  // Two alerts: the generic refusal and the server's named unsupported model.
  const alerts = source.getByRole("alert");
  await expect(alerts).toHaveCount(2);
  await expect(alerts.nth(0)).toContainText("완전한 보관 파일이나 복원이 생성되지 않았습니다");
  await expect(alerts.nth(1)).toHaveText("multiple authors or grants");
  expect(downloads).toBe(0);
  expect(csp).toEqual([]);
});

test("native archive authentication rejects malformed bytes before preflight", async ({
  browser,
  baseURL,
}) => {
  const anonymous = await browser.newContext({ baseURL });
  try {
    const response = await anonymous.request.post(
      "/api/v1/workspaces/ffffffff-ffff-4fff-8fff-ffffffffffff/native-archive/preflight",
      {
        headers: { "content-type": "application/json" },
        data: "malformed archive JSON",
      },
    );
    expect(response.status()).toBe(401);
  } finally {
    await anonymous.close();
  }
});

// ---------------------------------------------------------------------------
// W7 #328 first vertical pair: ordinary source installation A (this runner's
// server) -> settings export -> independent ZIP reader -> separate destination
// installation B (own database, restricted app role, storage, port 0) ->
// preflight/confirm -> durable job -> fresh client API/UI/native reader -> peer
// edit and new revision. Expected literals below are declared before any
// archive writer runs; server-created IDs are frozen from ordinary API results
// before export and never read back from the archive under test.
const SOURCE_OWNER = { email: "native-owner@example.com", password: "nativearchive123" };
const DESTINATION_OWNER = {
  email: "native-destination@example.com",
  password: "nativedestination123",
};
const DESTINATION_PROBE = { email: "native-probe@example.com", password: "nativeprobe123" };
const FIXTURE = {
  projectKey: "W7ARC",
  projectName: "원본 프로젝트 🧪",
  documentTitle: "이력 문서 한글🙂",
  taskTitle: "연결 태스크 🧪",
  taskType: "task",
  taskPriority: "medium",
  startDate: "2026-10-02",
  dueDate: "2026-10-03",
  taskOriginAnchor: "w7-task-origin",
  label: { name: "검토 라벨 🧪", color: "teal" },
  comments: {
    top: { body: "검토 댓글 🧪", chosung: "ㄱㅌ ㄷㄱ 🧪" },
    reply: { body: "답글 🙂", chosung: "ㄷㄱ 🙂" },
    task: { body: "태스크 확인", chosung: "ㅌㅅㅋ ㅎㅇ" },
  },
  // Display names of the two setup owners ("{given} {family}").
  sourcePersonName: "보관 검증 김",
  destinationPersonName: "복원 검증 이",
  link: "https://example.com/w7-source",
  documentBefore: "수정 전 한글🙂 굵게 🧪 삭제될 문장",
  documentAfter: "수정 후 한글🙂 굵게 🧪",
  taskBefore: "태스크 본문 한글🙂 삭제할 꼬리",
  taskAfter: "태스크 본문 한글🙂 🧪",
  peerEdit: "다시 ",
  files: {
    document: {
      name: "문서 자료 🧪.txt",
      text: "문서 첨부\n한글🙂\n",
      size: 25,
      sha256: "ff1008564bcec4c1df225b8566df3dc17a9e632d148786141b2a06de4e5c26a2",
    },
    task: {
      name: "태스크 자료 🙂.txt",
      text: "태스크 첨부\n🧪 끝\n",
      size: 26,
      sha256: "d402a56f8adcdfce3573f801ae2e7bb4aec2a669f78bd0f81e120800f53b4ada",
    },
  },
} as const;
const UUID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
const FORBIDDEN_ARCHIVE_KEYS =
  /password|pepper|session|token|secret|credential|storage_key|sealed|api_key|webhook|vapid|zotero/i;
// The five portable Zotero mirror classes are the only zotero-named keys an
// archive may carry (credentials and sync state never); they are empty here
// because this export selects no connector.
const PORTABLE_ZOTERO_KEYS = [
  "zotero_connectors",
  "zotero_references",
  "zotero_collections",
  "zotero_memberships",
  "zotero_links",
];
const forbiddenArchiveKeys = (keys: Iterable<string>): string[] =>
  [...keys].filter(
    (key) => FORBIDDEN_ARCHIVE_KEYS.test(key) && !PORTABLE_ZOTERO_KEYS.includes(key),
  );

const sha256 = (bytes: Uint8Array): string => createHash("sha256").update(bytes).digest("hex");
const idOf = z.object({ id: z.string().regex(UUID) }).passthrough();
const meSchema = z.object({ userId: z.string().regex(UUID) }).passthrough();
const workspaceMeta = z.object({ id: z.string().regex(UUID), slug: z.string() }).passthrough();
const projectSchema = z
  .object({
    id: z.string().regex(UUID),
    key: z.string(),
    name: z.string(),
    visibility: z.string(),
    rootDocumentId: z.string().regex(UUID),
    createdBy: z.string(),
  })
  .passthrough();
const documentMeta = z
  .object({
    id: z.string().regex(UUID),
    number: z.number(),
    title: z.string(),
    parentId: z.string().nullable(),
  })
  .passthrough();
const workflowSchema = z
  .object({
    id: z.string().regex(UUID),
    statuses: z.array(z.object({ id: z.string().regex(UUID), name: z.string() }).passthrough()),
  })
  .passthrough();
const taskSchema = z
  .object({
    id: z.string().regex(UUID),
    number: z.number(),
    title: z.string(),
    type: z.string(),
    priority: z.string(),
    statusId: z.string().regex(UUID),
    startDate: z.string().nullable(),
    dueDate: z.string().nullable(),
    createdBy: z.string(),
    assigneeIds: z.array(z.string()),
    labelIds: z.array(z.string()),
  })
  .passthrough();
const revisionList = z.object({
  items: z.array(z.object({ id: z.string().regex(UUID), reason: z.string() }).passthrough()),
});
const revisionDetail = z
  .object({ id: z.string().regex(UUID), contentJson: z.unknown(), ySnapshot: z.string() })
  .passthrough();
const activityChange = z.object({ field: z.string(), from: z.unknown(), to: z.unknown() }).strict();
const activityList = z.object({
  items: z.array(
    z
      .object({
        id: z.string().regex(UUID),
        type: z.string(),
        kind: z.string().optional(),
        changes: z.array(activityChange).optional(),
      })
      .passthrough(),
  ),
});
const commentSchema = z
  .object({
    id: z.string().regex(UUID),
    parentId: z.string().nullable(),
    createdBy: z.string(),
    body: z.string(),
    resolvedAt: z.string().nullable(),
    reactions: z.record(z.object({ count: z.number(), reactedByMe: z.boolean() })),
  })
  .passthrough();
const commentList = z.object({ items: z.array(commentSchema) }).passthrough();
const labelSchema = z
  .object({ id: z.string().regex(UUID), name: z.string(), color: z.string() })
  .passthrough();
const uploadSchema = z.object({
  attachmentId: z.string().regex(UUID),
  partSizeBytes: z.number(),
  parts: z.array(z.object({ partNumber: z.number(), url: z.string() })),
});
const preflightSchema = z
  .object({
    archiveHash: z.string().regex(/^[a-f0-9]{64}$/),
    sourceWorkspaceId: z.string().regex(UUID),
    destinationWorkspaceId: z.string().regex(UUID),
    destinationActorId: z.string().regex(UUID),
    projectId: z.string().regex(UUID),
    documentCount: z.number(),
    taskCount: z.number(),
    attachmentCount: z.number(),
    revisionCount: z.number(),
    complete: z.boolean(),
  })
  .passthrough();
const collectionSchema = z
  .object({ id: z.string().regex(UUID), name: z.string(), version: z.number() })
  .passthrough();
const itemLookupSchema = z
  .object({
    item: z
      .object({ id: z.string().regex(UUID), collectionId: z.string(), taskId: z.string() })
      .passthrough(),
  })
  .passthrough();
// Raw stored comments of one document and one task, oldest first.
const commentsSelect = (document: string, task: string) =>
  `SELECT coalesce(jsonb_agg(jsonb_build_object('id', id, 'parent', parent_id, 'by', created_by,
    'body', body, 'chosung', chosung, 'resolved', resolved_at IS NOT NULL, 'reactions', reactions)
    ORDER BY created_at, id), '[]'::jsonb) FROM fvoci.comments
    WHERE document_id = '${document}' OR task_id = '${task}'`;
// Raw stored changes of a task's "changed" activities, keyed by activity ID.
const changedActivitySelect = (task: string) =>
  `SELECT coalesce(jsonb_object_agg(id, changes), '{}'::jsonb) FROM fvoci.task_activity
    WHERE task_id = '${task}' AND kind = 'changed'`;
const baselineSelect = (project: string) =>
  `SELECT jsonb_build_object('collections', (SELECT jsonb_agg(jsonb_build_object('id', id, 'kind', kind,
    'name', name, 'version', version, 'deleted', deleted_at IS NOT NULL, 'created', created_at,
    'updated', updated_at) ORDER BY id) FROM fvoci.collections WHERE project_id = '${project}'),
    'items', (SELECT jsonb_agg(jsonb_build_object('id', i.id, 'collection', i.collection_id,
    'task', i.task_id, 'version', i.version, 'created', i.created_at, 'updated', i.updated_at) ORDER BY i.id)
    FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id = i.collection_id
    WHERE c.project_id = '${project}'))`;
const jobSchema = z
  .object({
    id: z.string().regex(UUID),
    status: z.string(),
    archiveHash: z.string(),
    projectId: z.string().nullable().optional(),
    diagnostic: z.string().nullable().optional(),
  })
  .passthrough();

type TiptapJson = {
  type: string;
  text?: string;
  attrs?: Record<string, unknown>;
  content?: TiptapJson[];
};
type EditorElement = HTMLElement & { editor: Editor };

/** Restricted app-role read in one READ ONLY transaction under the given tenant. */
function appRoleRead(connection: string, tenant: string, select: string): unknown {
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!container?.startsWith("fvoci-rust-test-pg-")) throw new Error("owned PG fixture missing");
  const app = new URL(connection);
  if (
    app.hostname !== "127.0.0.1" ||
    !/^fvoci_app_fvoci_e2e_[a-f0-9]{16}$/.test(app.username) ||
    !/^\/fvoci_e2e_[a-f0-9]{16}$/.test(app.pathname) ||
    !UUID.test(tenant)
  )
    throw new Error("Refusing a non-fixture DB connection");
  const result = spawnSync(
    "docker",
    [
      "exec",
      "-i",
      container,
      "psql",
      "-X",
      "-qAt",
      "-U",
      app.username,
      "-d",
      app.pathname.slice(1),
    ],
    {
      input: `\\set ON_ERROR_STOP 1
BEGIN READ ONLY;
SET LOCAL app.tenant_id = '${tenant}';
SELECT jsonb_build_object('role', current_user, 'superuser', r.rolsuper, 'bypassRls', r.rolbypassrls,
  'value', (${select})) FROM pg_roles r WHERE r.rolname = current_user;
ROLLBACK;`,
      encoding: "utf8",
      timeout: 15000,
    },
  );
  if (result.status !== 0) throw new Error(`app-role read failed: ${result.stderr}`);
  const witness = z
    .object({
      role: z.string(),
      superuser: z.boolean(),
      bypassRls: z.boolean(),
      value: z.unknown(),
    })
    .parse(JSON.parse(result.stdout));
  expect(witness).toMatchObject({ role: app.username, superuser: false, bypassRls: false });
  return witness.value;
}

// Setup/admin/test credentials that must never reach a normal server process.
const SENSITIVE_SERVER_ENV = [
  "DATABASE_URL",
  "FVOCI_MIGRATION_URL",
  "FVOCI_E2E_ADMIN_DATABASE_URL",
  "TEST_DATABASE_URL",
  "FVOCI_TEST_DATABASE_URL",
  "MEILI_MASTER_KEY",
  "FVOCI_MEILI_MASTER_KEY",
  "FVOCI_MEILI_KEY",
  "POSTGRES_PASSWORD",
];
function checkServerEnvNames(names: string[]): void {
  expect(names.filter((name) => SENSITIVE_SERVER_ENV.includes(name))).toEqual([]);
  expect(names).toContain("DATABASE_APP_URL");
}
/** The restricted role sees RLS enabled on archive tables it does not own. */
function roleWitness(connection: string, tenant: string): unknown {
  const value = z
    .array(z.object({ t: z.string(), rls: z.boolean(), force: z.boolean(), owned: z.boolean() }))
    .parse(
      appRoleRead(
        connection,
        tenant,
        `SELECT jsonb_agg(jsonb_build_object('t', c.relname, 'rls', c.relrowsecurity,
          'force', c.relforcerowsecurity, 'owned', pg_get_userbyid(c.relowner) = current_user)
          ORDER BY c.relname) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE n.nspname = 'fvoci' AND c.relname IN ('projects', 'documents', 'tasks',
          'document_states', 'task_states', 'revisions', 'attachments', 'import_jobs',
          'task_activity', 'events')`,
      ),
    );
  expect(value).toHaveLength(10);
  for (const table of value)
    expect([table.t, table.rls, table.owned]).toEqual([table.t, true, false]);
  return value;
}

const nativeRow = z.object({
  state: z.string(),
  cutoff: z.number(),
  tail: z.number(),
  updates: z.array(z.object({ seq: z.number(), op: z.string(), hex: z.string() })),
  receipts: z.array(
    z.object({
      op: z.string(),
      seq: z.number(),
      len: z.number(),
      sha: z.string(),
      actor: z.string(),
    }),
  ),
  content: z.unknown(),
  text: z.string(),
});
type NativeRow = z.infer<typeof nativeRow>;
/** One statement = one snapshot: state, cutoff/tail, retained updates, receipts and body together. */
function nativeSelect(kind: "document" | "task", id: string): string {
  if (!UUID.test(id)) throw new Error("invalid native target");
  const parent = kind === "document" ? "documents" : "tasks";
  return `SELECT jsonb_build_object('state', encode(s.state, 'hex'),
    'cutoff', s.snapshot_cutoff_seq, 'tail', s.tail_seq,
    'updates', coalesce((SELECT jsonb_agg(jsonb_build_object('seq', u.seq, 'op', u.op_id,
      'hex', encode(u.payload, 'hex')) ORDER BY u.seq)
      FROM fvoci.${kind}_collab_updates u WHERE u.${kind}_id = s.${kind}_id
      AND u.seq > s.snapshot_cutoff_seq AND u.seq <= s.tail_seq), '[]'::jsonb),
    'receipts', coalesce((SELECT jsonb_agg(jsonb_build_object('op', r.op_id, 'seq', r.seq,
      'len', r.payload_len, 'sha', encode(r.payload_sha256, 'hex'), 'actor', r.actor_user_id)
      ORDER BY r.seq, r.op_id) FROM fvoci.${kind}_collab_op_receipts r
      WHERE r.${kind}_id = s.${kind}_id), '[]'::jsonb),
    'content', p.content_json, 'text', p.text)
    FROM fvoci.${kind}_states s JOIN fvoci.${parent} p ON p.id = s.${kind}_id
    WHERE s.${kind}_id = '${id}'`;
}
/** ACK binding: the retained tail is contiguous and every retained update has a matching receipt. */
function checkBinding(row: NativeRow, actor: string): void {
  expect(row.updates.map((update) => update.seq)).toEqual(
    Array.from({ length: row.tail - row.cutoff }, (_, index) => row.cutoff + index + 1),
  );
  for (const update of row.updates) {
    const bytes = Buffer.from(update.hex, "hex");
    const receipt = row.receipts.find(
      (entry) => entry.op === update.op && entry.seq === update.seq,
    );
    expect(receipt, `receipt for seq ${String(update.seq)}`).toMatchObject({
      len: bytes.length,
      sha: sha256(bytes),
    });
  }
  expect(new Set(row.receipts.map((entry) => entry.op)).size).toBe(row.receipts.length);
  // Receipts of compacted ops keep only metadata; their shape is still bounded
  // (the historical payload hash cannot be recomputed: explicit limit).
  for (const entry of row.receipts) {
    expect(entry.op).toMatch(UUID);
    expect(Number.isInteger(entry.seq) && Number.isInteger(entry.len)).toBe(true);
    expect(entry.len >= 1 && entry.len <= 8 * 1024 * 1024).toBe(true);
    expect(entry.sha).toMatch(/^[a-f0-9]{64}$/);
  }
  expect(row.receipts.every((entry) => entry.seq >= 1 && entry.seq <= row.tail)).toBe(true);
  expect([...new Set(row.receipts.map((entry) => entry.actor))]).toEqual(
    row.receipts.length ? [actor] : [],
  );
}
const receiptFacts = (row: NativeRow) =>
  row.receipts.map(({ op, seq, len, sha }) => ({ op, seq, len, sha }));

/** Independent installed-Yjs reader of the persisted native store (gc=false). */
function nativeDoc(row: NativeRow): Y.Doc {
  const doc = new Y.Doc({ gc: false });
  Y.applyUpdate(doc, Buffer.from(row.state, "hex"));
  for (const update of row.updates) Y.applyUpdate(doc, Buffer.from(update.hex, "hex"));
  return doc;
}
/** The product declares exactly one root: the editor's XML fragment
 * (FVOCI_YDOC_FRAGMENT, collab-engine FRAGMENT). Any other root fails. */
function rootFacts(doc: Y.Doc): string[] {
  const names = [...doc.share.keys()].sort();
  for (const name of names)
    if (name !== "prosemirror") throw new Error(`undeclared native root type: ${name}`);
  expect(doc.getXmlFragment("prosemirror")).toBeInstanceOf(Y.XmlFragment);
  return names;
}

/** Small JSON facts in the test output and, when provided, the owned evidence directory. */
function record(out: string, name: string, value: unknown): void {
  const text = JSON.stringify(value, null, 2);
  writeFileSync(path.join(out, name), text);
  const evidence = process.env.FVOCI_W7_NATIVE_EVIDENCE_DIR;
  if (evidence) {
    mkdirSync(evidence, { recursive: true });
    writeFileSync(path.join(evidence, name), text);
  }
}

async function entryBytes(zip: JSZip, name: string): Promise<Uint8Array> {
  const file = zip.file(name);
  if (!file) throw new Error(`archive entry missing: ${name}`);
  return file.async("uint8array");
}
async function entryText(zip: JSZip, name: string): Promise<string> {
  return Buffer.from(await entryBytes(zip, name)).toString("utf8");
}

async function okJson<T>(
  response: Promise<{
    ok(): boolean;
    status(): number;
    text(): Promise<string>;
    json(): Promise<unknown>;
  }>,
  schema: z.ZodType<T>,
): Promise<T> {
  const res = await response;
  if (!res.ok()) throw new Error(`HTTP ${String(res.status())}: ${await res.text()}`);
  return schema.parse(await res.json());
}

async function uploadFile(
  request: APIRequestContext,
  uploadPath: string,
  workspaceId: string,
  file: { name: string; text: string },
): Promise<string> {
  const bytes = Buffer.from(file.text, "utf8");
  const upload = await okJson(
    request.post(uploadPath, { data: { name: file.name, sizeBytes: bytes.length } }),
    uploadSchema,
  );
  const parts: { partNumber: number; etag: string }[] = [];
  for (const part of upload.parts) {
    const put = await request.put(part.url, {
      headers: { "content-type": "application/octet-stream" },
      data: bytes.subarray(
        (part.partNumber - 1) * upload.partSizeBytes,
        part.partNumber * upload.partSizeBytes,
      ),
    });
    expect(put.ok(), await put.text()).toBe(true);
    const etag = put.headers()["etag"];
    if (!etag) throw new Error("upload part etag missing");
    parts.push({ partNumber: part.partNumber, etag });
  }
  const complete = await request.post(
    `/api/v1/workspaces/${workspaceId}/attachments/${upload.attachmentId}/complete`,
    { data: { parts } },
  );
  expect(complete.ok(), await complete.text()).toBe(true);
  return upload.attachmentId;
}

async function openItem(page: Page, slug: string, displayId: string, scope: string): Promise<void> {
  const navigation = await page.goto(`/w/${slug}/${displayId}`);
  expect(navigation?.status()).toBe(200);
  const region = page.locator(scope);
  await expect(region.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 20000 });
  await expect(region.locator(".fvoci-editor .ProseMirror")).toBeVisible();
}

type EditAction =
  | { kind: "set"; content: TiptapJson }
  | { kind: "replace"; find: string; replacement: string }
  | { kind: "insertAfter"; find: string; text: string }
  | { kind: "deleteNode"; nodeType: string }
  | { kind: "insertNodeAtStart"; anchor: string; node: TiptapJson };

/** Real editor commands through the mounted Tiptap/Collaboration binding. */
async function edit(page: Page, scope: string, action: EditAction): Promise<void> {
  await page.locator(`${scope} .fvoci-editor .ProseMirror`).evaluate((root, input) => {
    const editor = (root as EditorElement).editor;
    const chain = () => editor.chain().focus();
    if (input.kind === "set") {
      if (!editor.commands.setContent(input.content)) throw new Error("setContent refused");
      return;
    }
    if (input.kind === "deleteNode" || input.kind === "insertNodeAtStart") {
      let target = -1;
      let size = 0;
      editor.state.doc.descendants((node, pos) => {
        if (target >= 0) return false;
        const id: unknown = node.attrs.id;
        if (input.kind === "deleteNode" ? node.type.name === input.nodeType : id === input.anchor) {
          target = input.kind === "deleteNode" ? pos : pos + 1;
          size = node.nodeSize;
        }
        return true;
      });
      if (target < 0) throw new Error(`edit target not found: ${JSON.stringify(input)}`);
      const ok =
        input.kind === "deleteNode"
          ? chain()
              .deleteRange({ from: target, to: target + size })
              .run()
          : chain().insertContentAt(target, input.node).run();
      if (!ok) throw new Error("node edit refused");
      return;
    }
    let from = -1;
    editor.state.doc.descendants((node, pos) => {
      if (from >= 0) return false;
      if (node.isText && node.text) {
        const index = node.text.indexOf(input.find);
        if (index >= 0) from = pos + index;
      }
      return true;
    });
    if (from < 0) throw new Error(`text not found: ${input.find}`);
    const range = { from, to: from + input.find.length };
    const ok =
      input.kind === "insertAfter"
        ? chain().insertContentAt(range.to, input.text).run()
        : input.replacement === ""
          ? chain().deleteRange(range).run()
          : chain().insertContentAt(range, input.replacement).run();
    if (!ok) throw new Error("text edit refused");
  }, action);
}

async function persist(page: Page, scope: string): Promise<void> {
  // The body save control sits beside the collab status; titles use the same label.
  const region = page.locator(`${scope} .document-page__collab`).first();
  await region.getByRole("button", { name: "저장", exact: true }).click();
  await expect(region.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 20000 });
  await expect(region.locator('[data-collab-pending="false"]')).toBeVisible();
}

// ---- Full-fidelity native/JSON structure. Every non-null attribute of every
// block, inline node and mark is kept (y-prosemirror never stores null attrs
// or ychange); adjacent text with identical marks is merged. Any shape the
// fixture cannot represent fails instead of being skipped.
type Attrs = Record<string, unknown>;
type FullMark = { type: string; attrs: Attrs };
type FullInline = { text: string; marks: FullMark[] } | { node: string; attrs: Attrs };
type FullBlock = { type: string; attrs: Attrs; content: FullInline[] };
function cleanAttrs(attrs: unknown): Attrs {
  if (attrs === null || attrs === undefined || attrs === true) return {};
  if (typeof attrs !== "object")
    throw new Error(`unrepresentable attribute value ${JSON.stringify(attrs)}`);
  return Object.fromEntries(
    Object.entries(attrs)
      .filter(([key, value]) => value !== null && value !== undefined && key !== "ychange")
      .sort(([a], [b]) => a.localeCompare(b)),
  );
}
function pushFullText(content: FullInline[], text: string, marks: FullMark[]): void {
  const sorted = [...marks].sort((a, b) => a.type.localeCompare(b.type));
  const last = content[content.length - 1];
  if (last && "text" in last && JSON.stringify(last.marks) === JSON.stringify(sorted))
    last.text += text;
  else if (text) content.push({ text, marks: sorted });
}
function fullJsonStructure(doc: unknown): FullBlock[] {
  return ((doc as TiptapJson).content ?? []).map((node) => {
    const content: FullInline[] = [];
    for (const child of node.content ?? []) {
      const marks = (child as { marks?: { type: string; attrs?: unknown }[] }).marks ?? [];
      if (child.type === "text")
        pushFullText(
          content,
          child.text ?? "",
          marks.map((mark) => ({ type: mark.type, attrs: cleanAttrs(mark.attrs) })),
        );
      else {
        if (child.content?.length) throw new Error(`nested inline content in ${child.type}`);
        content.push({ node: child.type, attrs: cleanAttrs(child.attrs) });
      }
    }
    return { type: node.type, attrs: cleanAttrs(node.attrs), content };
  });
}
function fullNativeStructure(doc: Y.Doc): FullBlock[] {
  return doc
    .getXmlFragment("prosemirror")
    .toArray()
    .map((child) => {
      if (!(child instanceof Y.XmlElement)) throw new Error("unexpected root-level native type");
      const content: FullInline[] = [];
      for (const inline of child.toArray()) {
        if (inline instanceof Y.XmlText) {
          for (const op of inline.toDelta() as { insert: unknown; attributes?: Attrs }[]) {
            if (typeof op.insert !== "string")
              throw new Error("unrepresentable embedded insert in native text");
            const marks = Object.entries(op.attributes ?? {})
              .filter(([, value]) => value !== null && value !== undefined)
              .map(([name, value]) => ({
                type: name.split("--")[0] ?? name,
                attrs: cleanAttrs(value),
              }));
            pushFullText(content, op.insert, marks);
          }
        } else if (inline instanceof Y.XmlElement) {
          if (inline.length) throw new Error(`nested inline content in ${inline.nodeName}`);
          content.push({ node: inline.nodeName, attrs: cleanAttrs(inline.getAttributes()) });
        } else throw new Error("unexpected inline native type");
      }
      return { type: child.nodeName, attrs: cleanAttrs(child.getAttributes()), content };
    });
}
function fullHistorical(doc: Y.Doc, snapshotBase64: string): FullBlock[] {
  return fullNativeStructure(
    Y.createDocFromSnapshot(doc, Y.decodeSnapshot(Buffer.from(snapshotBase64, "base64"))),
  );
}

// ---- Declared projection compared with independently declared literals:
// marks by name (link with its declared href), mention entity/target/label,
// emoji shortcode atoms (MarkedEmoji keeps marked emoji as text), file
// id/name/image, embed target, and only the declared block anchors (UniqueID
// assigns the other block IDs at random). Unknown shapes fail.
type Run =
  | { text: string; marks: string[] }
  | { mention: { entity: string; id: string; label: string } }
  | { emoji: string };
type Block = {
  type: string;
  anchor?: string;
  runs?: Run[];
  ref?: { entity: string; id: string; name?: string; image?: boolean };
};
const ANCHORS = new Set(["w7-document-anchor", "w7-task-origin"]);
function declared(blocks: FullBlock[]): Block[] {
  return blocks.map((full) => {
    const a = full.attrs;
    if (full.type === "attachment") {
      if (full.content.length) throw new Error("attachment with content");
      return {
        type: full.type,
        ref: {
          entity: "attachment",
          id: String(a.id),
          name: String(a.name),
          image: a.image === true,
        },
      };
    }
    if (full.type === "embed")
      return { type: full.type, ref: { entity: String(a.entity), id: String(a.ref) } };
    if (full.type !== "paragraph") throw new Error(`undeclared block type ${full.type}`);
    const runs: Run[] = full.content.map((inline): Run => {
      if ("text" in inline)
        return {
          text: inline.text,
          marks: inline.marks.map((mark) =>
            mark.type === "link" ? `link:${String(mark.attrs.href)}` : mark.type,
          ),
        };
      if (inline.node === "mention")
        return {
          mention: {
            entity: String(inline.attrs.entity),
            id: String(inline.attrs.id),
            label: String(inline.attrs.label),
          },
        };
      if (inline.node === "emoji") return { emoji: String(inline.attrs.name) };
      throw new Error(`undeclared inline node ${inline.node}`);
    });
    const id = a.id;
    return {
      type: full.type,
      ...(typeof id === "string" && ANCHORS.has(id) ? { anchor: id } : {}),
      runs,
    };
  });
}
const jsonStructure = (doc: unknown): Block[] => declared(fullJsonStructure(doc));
const nativeStructure = (doc: Y.Doc): Block[] => declared(fullNativeStructure(doc));
const historicalStructure = (doc: Y.Doc, snapshot: string): Block[] =>
  declared(fullHistorical(doc, snapshot));
/** Public relative position of the item at `index` of a block's first inline type. */
function itemAt(
  doc: Y.Doc,
  blockIndex: number,
  inlineIndex: number,
): { client: number; clock: number } {
  const element = doc.getXmlFragment("prosemirror").get(blockIndex);
  if (!(element instanceof Y.XmlElement)) throw new Error("block missing");
  const json = Y.relativePositionToJSON(
    Y.createRelativePositionFromTypeIndex(element, inlineIndex),
  ) as {
    item?: { client: number; clock: number };
  };
  if (!json.item) throw new Error("no item at position");
  return json.item;
}
function itemState(
  doc: Y.Doc,
  id: { client: number; clock: number },
): "absent" | "live" | "deleted" {
  const update = Y.decodeUpdate(Y.encodeStateAsUpdate(doc));
  const present = update.structs.some(
    (struct) =>
      struct.id.client === id.client &&
      struct.id.clock <= id.clock &&
      id.clock < struct.id.clock + struct.length,
  );
  if (!present) return "absent";
  return Y.isDeleted(update.ds, Y.createID(id.client, id.clock)) ? "deleted" : "live";
}

/** Matched DB ACK: poll the restricted app-role row until stored JSON and native both match. */
async function waitDurable(
  connection: string,
  tenant: string,
  kind: "document" | "task",
  id: string,
  expected: Block[],
  actor: string,
): Promise<NativeRow> {
  const read = () => nativeRow.parse(appRoleRead(connection, tenant, nativeSelect(kind, id)));
  await expect.poll(() => jsonStructure(read().content), { timeout: 20000 }).toEqual(expected);
  const current = read();
  expect(jsonStructure(current.content)).toEqual(expected);
  // Stored body and native store agree on every attribute, not only declared ones.
  expect(fullNativeStructure(nativeDoc(current))).toEqual(fullJsonStructure(current.content));
  checkBinding(current, actor);
  return current;
}

async function setupThroughUi(
  page: Page,
  owner: { email: string; password: string },
  workspace: { name: string; slug: string },
): Promise<void> {
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("이");
  await page.getByLabel("이름", { exact: true }).fill("복원 검증");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(workspace.name);
  await page.getByLabel("주소(영문)").fill(workspace.slug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}

/** The W6 Zotero CLI's fixed synthetic test pepper (src/bin/e2e-fixture/zotero.rs). */
const ZOTERO_CLI_PEPPER = { keys: JSON.stringify({ test: "a".repeat(64) }), active: "test" };

/** A second isolated installation: own database, NOSUPERUSER/NOBYPASSRLS role, storage and port 0. */
class Installation {
  url = "";
  envNames: string[] = [];
  readonly database = `fvoci_e2e_${randomBytes(8).toString("hex")}`;
  readonly role = `fvoci_app_${this.database}`;
  readonly pepper: string;
  readonly pepperId: string;
  private readonly rolePassword = randomBytes(16).toString("hex");
  private child: ChildProcess | null = null;
  private provisioned = false;
  /** Only resources this installation actually created are ever removed. */
  protected readonly created = { storage: false, database: false, role: false };
  private readonly admin: URL;
  private readonly container: string;
  constructor(
    readonly dir: string,
    pepper?: { keys: string; active: string },
    readonly search?: { url: string; index: string },
  ) {
    this.pepper = pepper?.keys ?? JSON.stringify({ dst: randomBytes(32).toString("hex") });
    this.pepperId = pepper?.active ?? "dst";
    const admin = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
    const container = process.env.FVOCI_TEST_PG_CONTAINER;
    if (!admin || !container?.startsWith("fvoci-rust-test-pg-"))
      throw new Error("isolated PG fixture missing");
    this.admin = new URL(admin);
    this.container = container;
  }
  get adminUrl(): string {
    const url = new URL(this.admin.toString());
    url.pathname = `/${this.database}`;
    return url.toString();
  }
  get appUrl(): string {
    const url = new URL(this.admin.toString());
    url.username = this.role;
    url.password = this.rolePassword;
    url.pathname = `/${this.database}`;
    return url.toString();
  }
  protected psql(database: string, sql: string): void {
    const result = spawnSync(
      "docker",
      [
        "exec",
        "-i",
        this.container,
        "psql",
        "-X",
        "-q",
        "-U",
        "postgres",
        "-d",
        database,
        "-v",
        "ON_ERROR_STOP=1",
      ],
      { input: sql, encoding: "utf8", timeout: 30000 },
    );
    if (result.status !== 0) throw new Error(`psql failed: ${result.stderr}`);
  }
  private tool(args: string[], env: Record<string, string>): string {
    const serverBin = process.env.FVOCI_E2E_SERVER_BIN;
    if (!serverBin) throw new Error("FVOCI_E2E_SERVER_BIN missing");
    const result = spawnSync(path.join(path.dirname(serverBin), args[0] ?? ""), args.slice(1), {
      env: { PATH: process.env.PATH ?? "", HOME: process.env.HOME ?? "", ...env },
      encoding: "utf8",
      timeout: 120000,
    });
    if (result.status !== 0) throw new Error(`${args[0] ?? ""} failed: ${result.stderr}`);
    return result.stdout;
  }
  get storageDir(): string {
    return path.join(this.dir, "storage");
  }
  async start(): Promise<void> {
    this.provision();
    await this.startServer();
  }
  /** Database, restricted role and storage only; disposed once by dispose(). */
  provision(): void {
    if (this.provisioned) throw new Error("installation already provisioned");
    this.provisioned = true;
    if (existsSync(this.storageDir)) throw new Error("installation storage already exists");
    mkdirSync(this.storageDir, { recursive: true });
    this.created.storage = true;
    this.psql("postgres", `CREATE DATABASE "${this.database}"`);
    this.created.database = true;
    this.tool(["fvoci-migrate"], { DATABASE_URL: this.adminUrl });
    this.psql(
      this.database,
      `CREATE ROLE "${this.role}" LOGIN PASSWORD '${this.rolePassword}' NOSUPERUSER NOBYPASSRLS`,
    );
    this.created.role = true;
    this.tool(["fvoci-migrate", "--grant-app-role", this.role], { DATABASE_URL: this.adminUrl });
    if (this.search) {
      const master = process.env.MEILI_MASTER_KEY;
      if (!master) throw new Error("test-owned Meili preparation key missing");
      this.tool(["fvoci-migrate", "--ensure-meili-key", path.join(this.storageDir, "meili.key")], {
        FVOCI_MEILI_URL: this.search.url,
        FVOCI_MEILI_INDEX: this.search.index,
        MEILI_MASTER_KEY: master,
      });
    }
  }
  async startServer(): Promise<void> {
    const env = ownedServerChildEnv("127.0.0.1:0");
    env.DATABASE_APP_URL = this.appUrl;
    env.FVOCI_STORAGE_DIR = path.join(this.dir, "storage");
    env.PASSWORD_PEPPER_KEYS = this.pepper;
    env.PASSWORD_PEPPER_ACTIVE_KEY_ID = this.pepperId;
    env.RUST_LOG = "warn,tower_http=debug";
    if (this.search) {
      env.FVOCI_MEILI_URL = this.search.url;
      env.FVOCI_MEILI_INDEX = this.search.index;
      env.FVOCI_MEILI_KEY_FILE = path.join(this.storageDir, "meili.key");
    }
    this.envNames = Object.keys(env).sort();
    const serverBin = process.env.FVOCI_E2E_SERVER_BIN;
    if (!serverBin) throw new Error("FVOCI_E2E_SERVER_BIN missing");
    mkdirSync(path.join(this.dir, "destination"), { recursive: true });
    const log = path.join(this.dir, "destination", "server.log");
    const child = spawn(serverBin, [], { detached: true, env, stdio: ["ignore", "pipe", "pipe"] });
    this.child = child;
    this.url = await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error("destination startup deadline"));
      }, 30000);
      let output = "";
      const capture = (bytes: Buffer): void => {
        appendFileSync(log, bytes);
        output = (output + bytes.toString()).slice(-8192);
        const url = /fvoci-server listening on (http:\/\/127\.0\.0\.1:\d+)/.exec(output)?.[1];
        if (url) {
          clearTimeout(timer);
          resolve(url);
        }
      };
      child.stdout.on("data", capture);
      child.stderr.on("data", capture);
      child.once("exit", (code) => {
        clearTimeout(timer);
        reject(new Error(`destination exited before readiness: ${String(code)}`));
      });
    });
  }
  createUser(owner: { email: string; password: string }): string {
    const targetDir = process.env.CARGO_TARGET_DIR;
    if (!targetDir) throw new Error("CARGO_TARGET_DIR missing");
    const result = spawnSync(path.join(targetDir, "debug/fvoci-e2e-fixture"), [], {
      env: {
        PATH: process.env.PATH ?? "",
        DATABASE_URL: this.adminUrl,
        PASSWORD_PEPPER_KEYS: this.pepper,
        PASSWORD_PEPPER_ACTIVE_KEY_ID: this.pepperId,
        E2E_USER_EMAIL: owner.email,
        E2E_USER_PASSWORD: owner.password,
        E2E_USER_GIVEN_NAME: "검증",
      },
      encoding: "utf8",
      timeout: 60000,
    });
    if (result.status !== 0) throw new Error(`fixture user failed: ${result.stderr}`);
    return result.stdout.trim();
  }
  async stop(): Promise<void> {
    await this.stopServer();
    this.dispose();
  }
  /** Stops and reaps the server only; database and storage stay owned here. */
  async stopServer(): Promise<{ code: number | null; signal: NodeJS.Signals | null }> {
    const child = this.child;
    this.child = null;
    if (child?.pid === undefined) return { code: null, signal: null };
    if (child.exitCode !== null || child.signalCode !== null)
      return { code: child.exitCode, signal: child.signalCode };
    const exited = once(child, "exit") as Promise<[number | null, NodeJS.Signals | null]>;
    child.kill("SIGTERM");
    const timer = setTimeout(() => {
      if (child.pid !== undefined) process.kill(-child.pid, "SIGKILL");
    }, 20000);
    const [code, signal] = await exited;
    clearTimeout(timer);
    return { code, signal };
  }
  /** Drops the database/role and removes storage, exactly once. */
  dispose(): void {
    // Every created resource is attempted independently; each is marked done
    // only after it succeeded, so a retry resumes the failed ones; nothing
    // pre-existing or foreign is touched. Failures are aggregated.
    const errors: unknown[] = [];
    const step = (resource: keyof Installation["created"], run: () => void): void => {
      if (!this.created[resource]) return;
      try {
        run();
        this.created[resource] = false;
      } catch (error) {
        errors.push(error);
      }
    };
    step("database", () => {
      this.psql("postgres", `DROP DATABASE IF EXISTS "${this.database}" WITH (FORCE)`);
    });
    step("role", () => {
      this.psql("postgres", `DROP ROLE IF EXISTS "${this.role}"`);
    });
    step("storage", () => {
      rmSync(this.storageDir, { recursive: true, force: true });
    });
    if (errors.length) throw new AggregateError(errors, "installation cleanup failed");
  }
  /**
   * The W6 Zotero CLI (fake upstream, no real network) on this installation's
   * database with its storage lent (FVOCI_E2E_ZOTERO_STORAGE_DIR, never
   * removed by the CLI). Only allowlisted names reach the child.
   */
  async serveZotero(): Promise<ZoteroCli> {
    const targetDir = process.env.CARGO_TARGET_DIR;
    const serverBin = process.env.FVOCI_E2E_SERVER_BIN;
    const dist = process.env.FVOCI_STATIC_DIR;
    const engine = process.env.FVOCI_COLLAB_ENGINE;
    if (!targetDir || !serverBin || !dist || !engine) throw new Error("CLI inputs missing");
    const env: NodeJS.ProcessEnv = {
      PATH: process.env.PATH ?? "",
      HOME: process.env.HOME ?? "",
      DATABASE_APP_URL: this.appUrl,
      FVOCI_COLLAB_ENGINE: engine,
      FVOCI_E2E_SERVER_BIN: serverBin,
      FVOCI_E2E_DIST: dist,
      FVOCI_E2E_ZOTERO_STORAGE_DIR: this.storageDir,
      RUST_LOG: "warn",
    };
    mkdirSync(path.join(this.dir, "zotero-cli"), { recursive: true });
    const log = path.join(this.dir, "zotero-cli", `cli-${String(Date.now())}.log`);
    const child = spawn(path.join(targetDir, "debug/fvoci-e2e-fixture"), ["zotero-readonly"], {
      detached: true,
      env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    try {
      return await ZoteroCli.start(child, log, Object.keys(env).sort());
    } catch (error) {
      // A startup failure still owns a live child: reap it before any
      // database/storage disposal can run.
      await reapChild(child);
      throw error;
    }
  }
}

/** SIGKILL the child's process group (if still running) and await its exit. */
async function reapChild(child: ChildProcess): Promise<void> {
  if (child.pid === undefined || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  try {
    process.kill(-child.pid, "SIGKILL");
  } catch {
    // Already gone between the check and the signal; exit still settles.
  }
  await exited;
}

/** One JSON line per command on stdin, one JSON reply line on stdout. */
class ZoteroCli {
  origin = "";
  private lines: string[] = [];
  private waiters: { resolve: (line: string) => void; reject: (error: Error) => void }[] = [];
  private exited = false;
  /** Set on a reply timeout: the line protocol can no longer be trusted. */
  private retired = false;
  private buffer = "";
  private constructor(
    private readonly child: ChildProcess,
    readonly envNames: string[],
  ) {}
  static async start(child: ChildProcess, log: string, envNames: string[]): Promise<ZoteroCli> {
    const cli = new ZoteroCli(child, envNames);
    child.stderr?.on("data", (bytes: Buffer) => {
      appendFileSync(log, bytes);
    });
    child.stdout?.on("data", (bytes: Buffer) => {
      cli.buffer += bytes.toString();
      let newline = cli.buffer.indexOf("\n");
      while (newline >= 0) {
        const line = cli.buffer.slice(0, newline);
        cli.buffer = cli.buffer.slice(newline + 1);
        // After retirement every line is discarded, never taken as a reply.
        if (!cli.retired) {
          const waiter = cli.waiters.shift();
          if (waiter) waiter.resolve(line);
          else cli.lines.push(line);
        }
        newline = cli.buffer.indexOf("\n");
      }
    });
    child.once("exit", (code, signal) => {
      cli.exited = true;
      for (const waiter of cli.waiters.splice(0))
        waiter.reject(new Error(`zotero CLI exited (${String(code)}, ${String(signal)})`));
    });
    const ready = z.object({ origin: z.string().regex(/^http:\/\/127\.0\.0\.1:\d+$/) });
    cli.origin = ready.parse(JSON.parse(await cli.next(60000))).origin;
    return cli;
  }
  private next(timeout = 60000): Promise<string> {
    const queued = this.lines.shift();
    if (queued !== undefined) return Promise.resolve(queued);
    if (this.exited) return Promise.reject(new Error("zotero CLI already exited"));
    return new Promise((resolve, reject) => {
      const waiter = {
        resolve: (line: string) => {
          clearTimeout(timer);
          resolve(line);
        },
        reject: (error: Error) => {
          clearTimeout(timer);
          reject(error);
        },
      };
      // A timeout poisons the sequential protocol: the waiter leaves the
      // queue, every later send is refused before writing, queued or late
      // lines are discarded, and the child is reaped (no correlation guess).
      const timer = setTimeout(() => {
        const index = this.waiters.indexOf(waiter);
        if (index >= 0) this.waiters.splice(index, 1);
        this.retired = true;
        this.lines.length = 0;
        // Awaited again by kill()/stop() before any disposal.
        reapChild(this.child).catch(() => undefined);
        reject(new Error("zotero CLI reply deadline"));
      }, timeout);
      this.waiters.push(waiter);
    });
  }
  async send(command: Record<string, unknown>, timeout = 60000): Promise<unknown> {
    if (this.retired || this.exited) throw new Error("zotero CLI protocol retired");
    this.child.stdin?.write(`${JSON.stringify(command)}\n`);
    return JSON.parse(await this.next(timeout)) as unknown;
  }
  /** stop -> {stopped, ownedResources 0} and process exit 0, else reaped and thrown. */
  async stop(): Promise<void> {
    if (this.child.exitCode !== null || this.child.signalCode !== null)
      throw new Error(`zotero CLI already exited: ${String(this.child.exitCode)}`);
    const exited = once(this.child, "exit") as Promise<[number | null, NodeJS.Signals | null]>;
    try {
      expect(await this.send({ command: "stop" })).toEqual({ stopped: true, ownedResources: 0 });
    } finally {
      const timer = setTimeout(() => {
        if (this.child.pid !== undefined) process.kill(-this.child.pid, "SIGKILL");
      }, 20000);
      const [code, signal] = await exited;
      clearTimeout(timer);
      expect([code, signal]).toEqual([0, null]);
    }
  }
  /** Failure-path reaping only (the run's first failure is kept): awaits exit. */
  kill(): Promise<void> {
    return reapChild(this.child);
  }
}

test("native archive restores an ordinary private project into a separate installation with native history", async ({
  browser,
  baseURL,
}, testInfo) => {
  test.setTimeout(420_000);
  const sourceApp = process.env.DATABASE_APP_URL;
  if (!sourceApp || !baseURL) throw new Error("source installation app role/base URL missing");
  const out = testInfo.outputPath("native-pair");
  mkdirSync(out, { recursive: true });
  const docBytes = Buffer.from(FIXTURE.files.document.text, "utf8");
  const taskBytes = Buffer.from(FIXTURE.files.task.text, "utf8");
  // Independent byte literals before upload.
  expect([docBytes.length, sha256(docBytes)]).toEqual([
    FIXTURE.files.document.size,
    FIXTURE.files.document.sha256,
  ]);
  expect([taskBytes.length, sha256(taskBytes)]).toEqual([
    FIXTURE.files.task.size,
    FIXTURE.files.task.sha256,
  ]);

  // ---- Source installation A: ordinary owner, project, bodies, revisions, files.
  const source = await browser.newContext({ baseURL });
  const destinationInstall = new Installation(out);
  try {
    const page = await source.newPage();
    const csp = watchCspViolations(page);
    await login(page, SOURCE_OWNER.email, SOURCE_OWNER.password);
    const sourceUser = await okJson(page.request.get("/api/v1/auth/me"), meSchema);
    // Launch proof (names only) for the source normal server, from the launcher.
    const namesFile = process.env.FVOCI_E2E_SERVER_ENV_NAMES;
    if (!namesFile) throw new Error("server env names file missing");
    const sourceServerEnvNames = readFileSync(namesFile, "utf8").split("\n").filter(Boolean);
    checkServerEnvNames(sourceServerEnvNames);
    // The private-start boundary: the owner's own personal workspace (ordinary
    // self-assignment from a document is a personal-workspace operation).
    const sourceWorkspace = await okJson(
      page.request.post("/api/v1/me/personal-workspace"),
      workspaceMeta,
    );
    const ws = sourceWorkspace.id;
    const sourceSlug = sourceWorkspace.slug;
    const project = await okJson(
      page.request.post(`/api/v1/workspaces/${ws}/projects`, {
        data: { key: FIXTURE.projectKey, name: FIXTURE.projectName, visibility: "private" },
      }),
      projectSchema,
    );
    const workflow = await okJson(
      page.request.get(`/api/v1/workspaces/${ws}/projects/${project.id}/workflow`),
      workflowSchema,
    );
    const root = await okJson(
      page.request.get(
        `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${project.rootDocumentId}`,
      ),
      documentMeta,
    );
    const document = await okJson(
      page.request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/documents`, {
        data: { title: FIXTURE.documentTitle, parentId: root.id },
      }),
      documentMeta,
    );
    const created = await okJson(
      page.request.post(`/api/v1/workspaces/${ws}/documents/${document.id}/tasks`, {
        data: {
          projectId: project.id,
          requestId: randomUUID(),
          anchor: FIXTURE.taskOriginAnchor,
          selfAssign: true,
          task: {
            title: FIXTURE.taskTitle,
            type: FIXTURE.taskType,
            priority: FIXTURE.taskPriority,
            startDate: FIXTURE.startDate,
            dueDate: FIXTURE.dueDate,
          },
        },
      }),
      z.object({ taskId: z.string().regex(UUID) }),
    );
    const task = await okJson(
      page.request.get(`/api/v1/workspaces/${ws}/tasks/${created.taskId}`),
      taskSchema,
    );
    expect(task.assigneeIds).toEqual([sourceUser.userId]);
    // An ordinary project label assigned through two ordinary task patches:
    // label + unassign, then reassign (two recorded "changed" activities, one
    // naming the label, both naming the source actor as an assignee).
    const label = await okJson(
      page.request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/labels`, {
        data: FIXTURE.label,
      }),
      labelSchema,
    );
    for (const data of [
      { labelIds: [label.id], assigneeIds: [] },
      { assigneeIds: [sourceUser.userId] },
    ]) {
      const patched = await page.request.patch(`/api/v1/workspaces/${ws}/tasks/${task.id}`, {
        data,
      });
      expect(patched.status(), await patched.text()).toBe(200);
    }
    const labeledTask = await okJson(
      page.request.get(`/api/v1/workspaces/${ws}/tasks/${task.id}`),
      taskSchema,
    );
    expect([labeledTask.labelIds, labeledTask.assigneeIds]).toEqual([
      [label.id],
      [sourceUser.userId],
    ]);
    // Ordinary comments: a document comment (padded body; stored trimmed), a
    // reply, a reaction and a resolution, and a task comment.
    const docComments = `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${document.id}/comments`;
    const topComment = await okJson(
      page.request.post(docComments, { data: { body: `  ${FIXTURE.comments.top.body}  ` } }),
      commentSchema,
    );
    const replyComment = await okJson(
      page.request.post(docComments, {
        data: { body: FIXTURE.comments.reply.body, parentId: topComment.id },
      }),
      commentSchema,
    );
    for (const action of ["reactions", "resolve"] as const) {
      const done = await page.request.post(
        `/api/v1/workspaces/${ws}/comments/${topComment.id}/${action}`,
        { data: action === "reactions" ? { emoji: "👍", on: true } : {} },
      );
      expect(done.ok(), await done.text()).toBe(true);
    }
    const taskComment = await okJson(
      page.request.post(`/api/v1/workspaces/${ws}/tasks/${task.id}/comments`, {
        data: { body: FIXTURE.comments.task.body },
      }),
      commentSchema,
    );
    const storedComments = (by: string) => [
      {
        id: topComment.id,
        parent: null,
        by,
        ...FIXTURE.comments.top,
        resolved: true,
        reactions: { "👍": [by] },
      },
      {
        id: replyComment.id,
        parent: topComment.id,
        by,
        ...FIXTURE.comments.reply,
        resolved: false,
        reactions: {},
      },
      {
        id: taskComment.id,
        parent: null,
        by,
        ...FIXTURE.comments.task,
        resolved: false,
        reactions: {},
      },
    ];
    const docFile = await uploadFile(
      page.request,
      `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${document.id}/uploads`,
      ws,
      FIXTURE.files.document,
    );
    const taskFile = await uploadFile(
      page.request,
      `/api/v1/workspaces/${ws}/tasks/${task.id}/uploads`,
      ws,
      FIXTURE.files.task,
    );
    // Source file metadata and bytes through ordinary APIs, before export.
    const sourceFiles: Record<string, unknown> = {};
    for (const [fileId, literal] of [
      [docFile, FIXTURE.files.document],
      [taskFile, FIXTURE.files.task],
    ] as const) {
      const meta = await okJson(
        page.request.get(`/api/v1/workspaces/${ws}/attachments/${fileId}`),
        z.object({ id: z.string(), name: z.string(), sizeBytes: z.number() }).passthrough(),
      );
      expect([meta.id, meta.name, meta.sizeBytes]).toEqual([fileId, literal.name, literal.size]);
      const bytes = await page.request.get(
        `/api/v1/workspaces/${ws}/attachments/${fileId}/download`,
      );
      expect(bytes.ok()).toBe(true);
      expect(sha256(await bytes.body())).toBe(literal.sha256);
      sourceFiles[fileId] = meta;
    }
    const docScope = ".document-page";
    const taskScope = '[data-testid="task-body"]';
    const item = (number: number) => `${FIXTURE.projectKey}-${String(number)}`;
    // Independently declared typed structures (StarterKit's trailing node adds
    // the empty paragraph after the final block attachment).
    const trailing: Block = { type: "paragraph", runs: [] };
    const textRuns = (head: string, deleted: boolean): Run[] => [
      { text: head, marks: [] },
      { text: "한글🙂", marks: ["bold"] },
      { text: " 굵게 ", marks: ["italic"] },
      { text: "🧪", marks: [`link:${FIXTURE.link}`] },
      ...(deleted ? [] : [{ text: " 삭제될 문장", marks: [] }]),
    ];
    const docBlocks = (runs: Run[]): Block[] => [
      { type: "paragraph", anchor: "w7-document-anchor", runs },
      {
        type: "paragraph",
        anchor: FIXTURE.taskOriginAnchor,
        runs: [{ mention: { entity: "task", id: task.id, label: FIXTURE.taskTitle } }],
      },
      {
        type: "attachment",
        ref: { entity: "attachment", id: docFile, name: FIXTURE.files.document.name, image: false },
      },
      trailing,
    ];
    const taskBlocks = (runs: Run[]): Block[] => [
      { type: "paragraph", runs },
      {
        type: "paragraph",
        runs: [{ mention: { entity: "document", id: document.id, label: FIXTURE.documentTitle } }],
      },
      { type: "embed", ref: { entity: "document", id: root.id } },
      {
        type: "attachment",
        ref: { entity: "attachment", id: taskFile, name: FIXTURE.files.task.name, image: false },
      },
      trailing,
    ];
    const expectedDocBefore = docBlocks(textRuns("수정 전 ", false));
    const expectedDocAfter = docBlocks(textRuns("수정 후 ", true));
    const expectedDocPeer = docBlocks(textRuns(`수정 후 ${FIXTURE.peerEdit}`, true));
    // Unmarked 🙂/🧪 become emoji atoms (library shortcodes), marked ones above stay text.
    const expectedTaskBefore = taskBlocks([
      { text: "태스크 본문 한글", marks: [] },
      { emoji: "slightly_smiling_face" },
      { text: " 삭제할 꼬리", marks: [] },
    ]);
    const expectedTaskAfter = taskBlocks([
      { text: "태스크 본문 한글", marks: [] },
      { emoji: "slightly_smiling_face" },
      { text: " ", marks: [] },
      { emoji: "test_tube" },
    ]);
    // Root document: open once so the ordinary room initializes its native state.
    await openItem(page, sourceSlug, item(root.number), docScope);
    await expect
      .poll(() =>
        appRoleRead(
          sourceApp,
          ws,
          `SELECT count(*) FROM fvoci.document_states WHERE document_id = '${root.id}'`,
        ),
      )
      .toBe(1);
    // Project document: before body with anchors, marks, task mention and file node.
    await openItem(page, sourceSlug, item(document.number), docScope);
    await edit(page, docScope, {
      kind: "set",
      content: {
        type: "doc",
        content: [
          {
            type: "paragraph",
            attrs: { id: "w7-document-anchor" },
            content: [
              { type: "text", text: "수정 전 " },
              { type: "text", text: "한글🙂", marks: [{ type: "bold" }] } as TiptapJson,
              { type: "text", text: " 굵게 ", marks: [{ type: "italic" }] } as TiptapJson,
              {
                type: "text",
                text: "🧪",
                marks: [{ type: "link", attrs: { href: FIXTURE.link } }],
              } as TiptapJson,
              { type: "text", text: " 삭제될 문장" },
            ],
          },
          {
            type: "paragraph",
            attrs: { id: FIXTURE.taskOriginAnchor },
            content: [
              { type: "mention", attrs: { entity: "task", id: task.id, label: FIXTURE.taskTitle } },
            ],
          },
          {
            type: "attachment",
            attrs: { id: docFile, name: FIXTURE.files.document.name, image: false },
          },
        ],
      },
    });
    await persist(page, docScope);
    const sourceDocBeforeRow = await waitDurable(
      sourceApp,
      ws,
      "document",
      document.id,
      expectedDocBefore,
      sourceUser.userId,
    );
    const mentionBefore = itemAt(nativeDoc(sourceDocBeforeRow), 1, 0);
    const docRevisionsPath = `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${document.id}/revisions`;
    const docBefore = await okJson(page.request.post(docRevisionsPath), idOf);
    // Text edit, phrase deletion, and a deleted then reinserted reference node.
    await edit(page, docScope, { kind: "replace", find: "수정 전", replacement: "수정 후" });
    await edit(page, docScope, { kind: "replace", find: " 삭제될 문장", replacement: "" });
    await edit(page, docScope, { kind: "deleteNode", nodeType: "mention" });
    await edit(page, docScope, {
      kind: "insertNodeAtStart",
      anchor: FIXTURE.taskOriginAnchor,
      node: { type: "mention", attrs: { entity: "task", id: task.id, label: FIXTURE.taskTitle } },
    });
    await persist(page, docScope);
    const sourceDocRow = await waitDurable(
      sourceApp,
      ws,
      "document",
      document.id,
      expectedDocAfter,
      sourceUser.userId,
    );
    const docAfter = await okJson(page.request.post(docRevisionsPath), idOf);
    const sourceDoc = nativeDoc(sourceDocRow);
    const mentionAfter = itemAt(sourceDoc, 1, 0);
    expect(mentionAfter).not.toEqual(mentionBefore);
    expect([itemState(sourceDoc, mentionBefore), itemState(sourceDoc, mentionAfter)]).toEqual([
      "deleted",
      "live",
    ]);
    // Public relative position at "한글🙂" inside the anchored paragraph text.
    const anchoredText = (doc: Y.Doc): Y.XmlText => {
      const paragraph = doc.getXmlFragment("prosemirror").get(0);
      const text = paragraph instanceof Y.XmlElement ? paragraph.get(0) : null;
      if (!(text instanceof Y.XmlText)) throw new Error("anchored paragraph text missing");
      return text;
    };
    const markedStart = "수정 후 ".length;
    const relative = Buffer.from(
      Y.encodeRelativePosition(
        Y.createRelativePositionFromTypeIndex(anchoredText(sourceDoc), markedStart),
      ),
    ).toString("base64");
    const resolveAt = (doc: Y.Doc): number | null => {
      const absolute = Y.createAbsolutePositionFromRelativePosition(
        Y.decodeRelativePosition(Buffer.from(relative, "base64")),
        doc,
      );
      return absolute && absolute.type === anchoredText(doc) ? absolute.index : null;
    };
    expect(resolveAt(sourceDoc)).toBe(markedStart);
    // Task body: document mention, root embed, task file, before/after revisions.
    await openItem(page, sourceSlug, item(task.number), taskScope);
    await edit(page, taskScope, {
      kind: "set",
      content: {
        type: "doc",
        content: [
          { type: "paragraph", content: [{ type: "text", text: FIXTURE.taskBefore }] },
          {
            type: "paragraph",
            content: [
              {
                type: "mention",
                attrs: { entity: "document", id: document.id, label: FIXTURE.documentTitle },
              },
            ],
          },
          { type: "embed", attrs: { entity: "document", ref: root.id } },
          {
            type: "attachment",
            attrs: { id: taskFile, name: FIXTURE.files.task.name, image: false },
          },
        ],
      },
    });
    await persist(page, taskScope);
    await waitDurable(sourceApp, ws, "task", task.id, expectedTaskBefore, sourceUser.userId);
    const taskRevisionsPath = `/api/v1/workspaces/${ws}/tasks/${task.id}/revisions`;
    const taskBefore = await okJson(page.request.post(taskRevisionsPath), idOf);
    await edit(page, taskScope, { kind: "replace", find: " 삭제할 꼬리", replacement: " 🧪" });
    await persist(page, taskScope);
    const sourceTaskRow = await waitDurable(
      sourceApp,
      ws,
      "task",
      task.id,
      expectedTaskAfter,
      sourceUser.userId,
    );
    const taskAfter = await okJson(page.request.post(taskRevisionsPath), idOf);
    // All four source revisions: stored JSON and native history match the declared structures.
    const revisionPlan = [
      { id: docBefore.id, base: docRevisionsPath, doc: sourceDoc, expected: expectedDocBefore },
      { id: docAfter.id, base: docRevisionsPath, doc: sourceDoc, expected: expectedDocAfter },
      {
        id: taskBefore.id,
        base: taskRevisionsPath,
        doc: nativeDoc(sourceTaskRow),
        expected: expectedTaskBefore,
      },
      {
        id: taskAfter.id,
        base: taskRevisionsPath,
        doc: nativeDoc(sourceTaskRow),
        expected: expectedTaskAfter,
      },
    ];
    const sourceRevisionSnapshots: Record<string, string> = {};
    const sourceRevisionJson: Record<string, unknown> = {};
    for (const revision of revisionPlan) {
      const detail = await okJson(
        page.request.get(`${revision.base}/${revision.id}`),
        revisionDetail,
      );
      expect(jsonStructure(detail.contentJson)).toEqual(revision.expected);
      expect(historicalStructure(revision.doc, detail.ySnapshot)).toEqual(revision.expected);
      // Full fidelity: the stored revision JSON equals its native reconstruction.
      expect(fullHistorical(revision.doc, detail.ySnapshot)).toEqual(
        fullJsonStructure(detail.contentJson),
      );
      sourceRevisionSnapshots[revision.id] = detail.ySnapshot;
      sourceRevisionJson[revision.id] = detail.contentJson;
    }
    // Every revision the product holds for the closure, frozen from the ordinary
    // list APIs: the four manual ones above plus automatic ones (a room writes a
    // session revision on close when its body differs from the last revision;
    // here only the root, which had none). Automatic JSON must equal its native
    // historical reconstruction too.
    const rootRevisionsPath = `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${root.id}/revisions`;
    const sourceRootRow = nativeRow.parse(
      appRoleRead(sourceApp, ws, nativeSelect("document", root.id)),
    );
    const revisionTargets = [
      { id: root.id, base: rootRevisionsPath, doc: nativeDoc(sourceRootRow) },
      { id: document.id, base: docRevisionsPath, doc: sourceDoc },
      { id: task.id, base: taskRevisionsPath, doc: nativeDoc(sourceTaskRow) },
    ];
    const manualRevisionIds = new Set(revisionPlan.map((revision) => revision.id));
    const sourceRevisionLists: Record<string, string[]> = {};
    const automaticRevisions: { id: string; target: string }[] = [];
    for (const target of revisionTargets) {
      const list = await okJson(page.request.get(target.base), revisionList);
      sourceRevisionLists[target.id] = list.items.map((item) => item.id).sort();
      for (const item of list.items) {
        if (manualRevisionIds.has(item.id)) {
          expect(item.reason).toBe("manual");
          continue;
        }
        expect(["session", "scheduled"]).toContain(item.reason);
        const detail = await okJson(page.request.get(`${target.base}/${item.id}`), revisionDetail);
        expect(fullHistorical(target.doc, detail.ySnapshot)).toEqual(
          fullJsonStructure(detail.contentJson),
        );
        sourceRevisionSnapshots[item.id] = detail.ySnapshot;
        sourceRevisionJson[item.id] = detail.contentJson;
        automaticRevisions.push({ id: item.id, target: target.id });
      }
    }
    for (const id of manualRevisionIds)
      expect(Object.values(sourceRevisionLists).flat()).toContain(id);
    const allRevisionIds = Object.values(sourceRevisionLists).flat().sort();
    // ACK/seq/state binding was checked in waitDurable from one snapshot per target.
    const sourceReceipts = {
      document: receiptFacts(sourceDocRow),
      task: receiptFacts(sourceTaskRow),
    };
    const sourceRoots = {
      document: rootFacts(sourceDoc),
      task: rootFacts(nativeDoc(sourceTaskRow)),
    };
    // Trigger-created baseline collection/item identities through ordinary APIs.
    const sourceCollection = await okJson(
      page.request.get(`/api/v1/workspaces/${ws}/projects/${project.id}/collection`),
      collectionSchema,
    );
    const sourceItem = await okJson(
      page.request.get(`/api/v1/workspaces/${ws}/tasks/${task.id}/collection-item`),
      itemLookupSchema,
    );
    expect([
      sourceCollection.name,
      sourceCollection.version,
      sourceItem.item.collectionId,
      sourceItem.item.taskId,
    ]).toEqual([FIXTURE.projectName, 1, sourceCollection.id, task.id]);
    if (!UUID.test(project.id)) throw new Error("project id");
    const sourceBaseline = appRoleRead(sourceApp, ws, baselineSelect(project.id));
    const activity = await okJson(
      page.request.get(`/api/v1/workspaces/${ws}/tasks/${task.id}/activity`),
      activityList,
    );
    const createdActivity = activity.items.filter(
      (entry) => entry.type === "change" && entry.kind === "created",
    );
    expect(createdActivity).toHaveLength(1);
    const createdActivityId = createdActivity[0]?.id ?? "";
    // Independent expected changes, fixed before export from the patches above.
    // Stored form (tasks::activity snapshot): reference objects {id, label}
    // with the label's name kept as history and assignee labels null; the
    // activity API instead returns lists as {items, totalCount} with current
    // visible person names. Both contracts are asserted separately.
    const ref = (id: string, label: string | null) => ({ id, label });
    const list = (...items: { id: string; label: string | null }[]) => ({
      items,
      totalCount: items.length,
    });
    const actor = sourceUser.userId;
    const storedLabelChanges = [
      { field: "assigneeIds", from: [ref(actor, null)], to: [] },
      { field: "labelIds", from: [], to: [ref(label.id, FIXTURE.label.name)] },
    ];
    const storedReassignChanges = [{ field: "assigneeIds", from: [], to: [ref(actor, null)] }];
    const visibleLabelChanges = [
      { field: "assigneeIds", from: list(ref(actor, FIXTURE.sourcePersonName)), to: list() },
      { field: "labelIds", from: list(), to: list(ref(label.id, FIXTURE.label.name)) },
    ];
    const visibleReassignChanges = [
      { field: "assigneeIds", from: list(), to: list(ref(actor, FIXTURE.sourcePersonName)) },
    ];
    const changedActivity = activity.items.filter(
      (entry) => entry.type === "change" && entry.kind === "changed",
    );
    expect(changedActivity).toHaveLength(2);
    const labelActivity = changedActivity.find((entry) =>
      entry.changes?.some((change) => change.field === "labelIds"),
    );
    const reassignActivity = changedActivity.find((entry) => entry !== labelActivity);
    if (!labelActivity || !reassignActivity) throw new Error("changed activity missing");
    expect(labelActivity.changes).toEqual(visibleLabelChanges);
    expect(reassignActivity.changes).toEqual(visibleReassignChanges);
    expect(appRoleRead(sourceApp, ws, changedActivitySelect(task.id))).toEqual({
      [labelActivity.id]: storedLabelChanges,
      [reassignActivity.id]: storedReassignChanges,
    });
    expect(appRoleRead(sourceApp, ws, commentsSelect(document.id, task.id))).toEqual(
      storedComments(sourceUser.userId),
    );
    const sourceSnapshot = {
      document: Y.snapshot(sourceDoc),
      task: Y.snapshot(nativeDoc(sourceTaskRow)),
    };
    // Frozen expected facts BEFORE the archive writer runs.
    const expected = {
      sourceWorkspaceId: ws,
      sourceActorId: sourceUser.userId,
      projectId: project.id,
      rootDocumentId: root.id,
      documentId: document.id,
      documentNumber: document.number,
      taskId: task.id,
      taskNumber: task.number,
      statusId: task.statusId,
      workflowId: workflow.id,
      createdActivityId,
      label: { id: label.id, ...FIXTURE.label },
      comments: storedComments(sourceUser.userId),
      changedActivity: {
        label: { id: labelActivity.id, stored: storedLabelChanges, visible: visibleLabelChanges },
        reassign: {
          id: reassignActivity.id,
          stored: storedReassignChanges,
          visible: visibleReassignChanges,
        },
      },
      revisions: {
        docBefore: docBefore.id,
        docAfter: docAfter.id,
        taskBefore: taskBefore.id,
        taskAfter: taskAfter.id,
      },
      revisionSnapshots: sourceRevisionSnapshots,
      revisionLists: sourceRevisionLists,
      automaticRevisions,
      structures: { expectedDocBefore, expectedDocAfter, expectedTaskBefore, expectedTaskAfter },
      mentionItems: { before: mentionBefore, after: mentionAfter },
      relativePosition: { base64: relative, index: markedStart },
      receipts: sourceReceipts,
      roots: sourceRoots,
      revisionJson: sourceRevisionJson,
      currentJson: { document: sourceDocRow.content, task: sourceTaskRow.content },
      files: { document: docFile, task: taskFile, metadata: sourceFiles },
      sourceServerEnvNames,
      baseline: { collection: sourceCollection.id, item: sourceItem.item.id, rows: sourceBaseline },
      sourceRoleWitness: roleWitness(sourceApp, ws),
    };
    record(out, "expected-before-export.json", expected);

    // ---- Settings export through the ordinary UI.
    await page.goto(`/w/${sourceSlug}/settings`);
    await page.getByLabel("보관할 프로젝트").selectOption(project.id);
    const exportSection = page
      .locator(".native-archive")
      .filter({ has: page.getByRole("button", { name: "네이티브 보관 파일 다운로드" }) });
    await expect(
      exportSection.getByText(/같은 데이터베이스의 빈 워크스페이스만으로는 충분하지 않습니다/),
    ).toBeVisible();
    const exportResponse = page.waitForResponse((response) =>
      response.url().endsWith(`/projects/${project.id}/native-archive`),
    );
    const downloaded = page.waitForEvent("download");
    // A refused export never downloads; keep that pending wait from rejecting unobserved.
    downloaded.catch(() => undefined);
    await exportSection.getByRole("button", { name: "네이티브 보관 파일 다운로드" }).click();
    const exported = await exportResponse;
    if (exported.status() !== 200)
      throw new Error(
        `native export refused ${String(exported.status())}: ${await exported.text()}`,
      );
    const download = await downloaded;
    const archivePath = path.join(out, "fvoci-native-project.zip");
    await download.saveAs(archivePath);
    const archive = readFileSync(archivePath);
    const archiveHash = sha256(archive);
    expect(csp).toEqual([]);

    // ---- Independent ZIP reader: exact entries, manifest digests, no credential data.
    const zip = await JSZip.loadAsync(archive);
    const names = Object.keys(zip.files).sort();
    for (const name of names) expect(zip.files[name]?.dir).toBe(false);
    const manifest = z
      .object({
        kind: z.literal("fvoci-native-user-archive"),
        format_version: z.literal(1),
        complete: z.literal(true),
        entries: z.record(z.string(), z.object({ size: z.number(), sha256: z.string() })),
      })
      .passthrough()
      .parse(JSON.parse(await entryText(zip, "manifest.json")));
    // The manifest digests every entry except itself (graph.json included).
    expect(names).toEqual(["manifest.json", ...Object.keys(manifest.entries)].sort());
    for (const [name, digest] of Object.entries(manifest.entries)) {
      const bytes = await entryBytes(zip, name);
      expect([name, bytes.length, sha256(bytes)]).toEqual([name, digest.size, digest.sha256]);
      expect(name).toMatch(
        /^(?:graph\.json|native\/(?:document|task)\/[a-f0-9-]{36}\/(?:state|[1-9]\d*)\.v1|revisions\/[a-f0-9-]{36}\.snapshot\.v1|attachments\/[a-f0-9-]{36}\/payload)$/,
      );
    }
    expect(sha256(await entryBytes(zip, `attachments/${docFile}/payload`))).toBe(
      FIXTURE.files.document.sha256,
    );
    expect(sha256(await entryBytes(zip, `attachments/${taskFile}/payload`))).toBe(
      FIXTURE.files.task.sha256,
    );
    const graphText = await entryText(zip, "graph.json");
    const keys = new Set<string>();
    const collect = (value: unknown): void => {
      if (Array.isArray(value)) value.forEach(collect);
      else if (value && typeof value === "object")
        for (const [key, child] of Object.entries(value)) {
          keys.add(key);
          collect(child);
        }
    };
    collect(JSON.parse(graphText));
    // Predicate controls: only the exact portable names are exempt.
    expect(
      forbiddenArchiveKeys([
        ...PORTABLE_ZOTERO_KEYS,
        "zotero_credentials",
        "zotero_sync_state",
        "zotero_connector",
        "credential",
        "session_id",
        "sealed_key",
        "api_key",
        "token",
      ]),
    ).toEqual([
      "zotero_credentials",
      "zotero_sync_state",
      "zotero_connector",
      "credential",
      "session_id",
      "sealed_key",
      "api_key",
      "token",
    ]);
    expect(forbiddenArchiveKeys(keys)).toEqual([]);
    const graphValue = JSON.parse(graphText) as Record<string, unknown>;
    for (const key of PORTABLE_ZOTERO_KEYS) expect(graphValue[key]).toEqual([]);
    const graph = z
      .object({
        source_workspace_id: z.string(),
        source_actor_id: z.string(),
        project: z.object({ id: z.string() }).passthrough(),
        documents: z.array(z.object({ id: z.string() }).passthrough()),
        tasks: z.array(z.object({ id: z.string() }).passthrough()),
        activity: z.array(z.object({ id: z.string() }).passthrough()),
        revisions: z.array(z.object({ id: z.string() }).passthrough()),
        collections: z.array(z.object({ id: z.string() }).passthrough()),
        collection_items: z.array(z.object({ id: z.string() }).passthrough()),
        labels: z.array(z.unknown()),
        task_labels: z.array(z.unknown()),
      })
      .passthrough()
      .parse(JSON.parse(graphText));
    expect(graph.source_workspace_id).toBe(expected.sourceWorkspaceId);
    expect(graph.source_actor_id).toBe(expected.sourceActorId);
    expect(graph.project.id).toBe(expected.projectId);
    expect(graph.documents.map((d) => d.id).sort()).toEqual([root.id, document.id].sort());
    expect(graph.tasks.map((t) => t.id)).toEqual([task.id]);
    expect(graph.activity.map((a) => a.id).sort()).toEqual(
      [createdActivityId, labelActivity.id, reassignActivity.id].sort(),
    );
    expect(graph.collections.map((c) => c.id)).toEqual([sourceCollection.id]);
    expect(graph.collection_items.map((i) => i.id)).toEqual([sourceItem.item.id]);
    expect(graph.revisions.map((r) => r.id).sort()).toEqual(allRevisionIds);
    expect(graph.labels).toEqual([
      {
        id: label.id,
        project_id: project.id,
        name: FIXTURE.label.name,
        color: FIXTURE.label.color,
        created_at: expect.any(String),
        updated_at: expect.any(String),
      },
    ]);
    expect(graph.task_labels).toEqual([{ task_id: task.id, label_id: label.id }]);
    expect(
      (graph as unknown as { comments: Record<string, unknown>[] }).comments.map((c) => ({
        id: c.id,
        parent: c.parent_id,
        by: c.created_by,
        body: c.body,
        chosung: c.chosung,
        resolved: c.resolved_at !== null,
        reactions: c.reactions,
      })),
    ).toEqual(storedComments(sourceUser.userId));
    const archivedChanges = Object.fromEntries(
      graph.activity
        .filter((a) => a.id !== createdActivityId)
        .map((a) => [a.id, (a as { changes?: unknown }).changes]),
    );
    expect(archivedChanges).toEqual({
      [labelActivity.id]: storedLabelChanges,
      [reassignActivity.id]: storedReassignChanges,
    });

    // ---- Same-source-database empty personal workspace is NOT a safe target:
    // a second ordinary source user's fresh personal workspace still collides.
    const COLLIDE = { email: "native-collide@example.com", password: "nativecollide123" };
    createE2eUser(COLLIDE.email, COLLIDE.password, "충돌");
    const collide = await browser.newContext({ baseURL });
    try {
      const cpage = await collide.newPage();
      await login(cpage, COLLIDE.email, COLLIDE.password);
      const collideUser = await okJson(cpage.request.get("/api/v1/auth/me"), meSchema);
      const collidePersonal = await okJson(
        cpage.request.post("/api/v1/me/personal-workspace"),
        workspaceMeta,
      );
      const sourcePreflight = await okJson(
        cpage.request.post(`/api/v1/workspaces/${collidePersonal.id}/native-archive/preflight`, {
          data: { archiveBase64: archive.toString("base64") },
        }),
        preflightSchema,
      );
      expect(sourcePreflight.archiveHash).toBe(archiveHash);
      const sameDb = await cpage.request.post(
        `/api/v1/workspaces/${collidePersonal.id}/native-archive/restore`,
        {
          data: {
            archiveBase64: archive.toString("base64"),
            archiveHash,
            requestId: randomUUID(),
            destinationActorId: collideUser.userId,
            confirm: true,
          },
        },
      );
      let sameDbStatus = sameDb.status();
      if (sameDbStatus === 202) {
        const job = jobSchema.parse(await sameDb.json());
        await expect
          .poll(
            async () =>
              (
                await cpage.request.get(
                  `/api/v1/workspaces/${collidePersonal.id}/native-archive/jobs/${job.id}`,
                )
              ).status(),
            { timeout: 60000 },
          )
          .toBe(409);
        sameDbStatus = 409;
      }
      expect(sameDbStatus).toBe(409);
      const collideProjects = await okJson(
        cpage.request.get(`/api/v1/workspaces/${collidePersonal.id}/projects`),
        z.object({ items: z.array(idOf) }),
      );
      expect(collideProjects.items).toEqual([]);
    } finally {
      await collide.close();
    }
    // The original project is untouched by the refused restore.
    const sourceAfterRefusal = nativeRow.parse(
      appRoleRead(sourceApp, ws, nativeSelect("document", document.id)),
    );
    expect(
      Y.equalSnapshots(Y.snapshot(nativeDoc(sourceAfterRefusal)), sourceSnapshot.document),
    ).toBe(true);

    // ---- Destination installation B.
    await destinationInstall.start();
    checkServerEnvNames(destinationInstall.envNames);
    const destinationApp = destinationInstall.appUrl;
    const destination = await browser.newContext({ baseURL: destinationInstall.url });
    try {
      const dpage = await destination.newPage();
      const dcsp = watchCspViolations(dpage);
      await setupThroughUi(dpage, DESTINATION_OWNER, {
        name: "복원 대상 팀",
        slug: "native-destination",
      });
      const destinationUser = await okJson(dpage.request.get("/api/v1/auth/me"), meSchema);
      expect(destinationUser.userId).not.toBe(sourceUser.userId);
      const personal = await okJson(
        dpage.request.post("/api/v1/me/personal-workspace"),
        workspaceMeta,
      );

      // Negative controls with a separate destination actor (own rate budget).
      destinationInstall.createUser(DESTINATION_PROBE);
      const probe = await browser.newContext({ baseURL: destinationInstall.url });
      try {
        const ppage = await probe.newPage();
        await login(ppage, DESTINATION_PROBE.email, DESTINATION_PROBE.password);
        const probeUser = await okJson(ppage.request.get("/api/v1/auth/me"), meSchema);
        const probePersonal = await okJson(
          ppage.request.post("/api/v1/me/personal-workspace"),
          workspaceMeta,
        );
        const preflightAt = (bytes: Buffer) =>
          ppage.request.post(`/api/v1/workspaces/${probePersonal.id}/native-archive/preflight`, {
            data: { archiveBase64: bytes.toString("base64") },
          });
        // Re-packed with identical entries: discriminating positive control.
        // mode: none = identical entries; bytes = native state changed under the
        // old digest; rehash = changed native state with a recomputed digest.
        const stateName = `native/document/${document.id}/state.v1`;
        const repack = async (mode: "none" | "bytes" | "rehash"): Promise<Buffer> => {
          const copy = new JSZip();
          const original = await entryBytes(zip, stateName);
          const changed = Uint8Array.from(original);
          changed[changed.length - 1] = (changed[changed.length - 1] ?? 0) ^ 0x01;
          for (const name of names) {
            let bytes = await entryBytes(zip, name);
            if (mode !== "none" && name === stateName) bytes = changed;
            if (mode === "rehash" && name === "manifest.json") {
              const edited = z
                .object({
                  entries: z.record(z.string(), z.object({ size: z.number(), sha256: z.string() })),
                })
                .passthrough()
                .parse(JSON.parse(Buffer.from(bytes).toString("utf8")));
              edited.entries[stateName] = { size: changed.length, sha256: sha256(changed) };
              bytes = Buffer.from(JSON.stringify(edited), "utf8");
            }
            copy.file(name, bytes, { createFolders: false });
          }
          return copy.generateAsync({ type: "nodebuffer", compression: "DEFLATE" });
        };
        const repacked = await repack("none");
        const repackedPreflight = await preflightAt(repacked);
        const tampered = await preflightAt(await repack("bytes"));
        expect(tampered.status(), await tampered.text()).toBe(422);
        // SHA-256 is integrity, not authenticity: a re-hashed corrupt native state
        // must still be refused by native validation before any effect.
        const rehashed = await preflightAt(await repack("rehash"));
        expect(rehashed.status(), await rehashed.text()).toBe(422);
        const restoreAt = (body: Record<string, unknown>) =>
          ppage.request.post(`/api/v1/workspaces/${probePersonal.id}/native-archive/restore`, {
            data: body,
          });
        const wrongHash = await restoreAt({
          archiveBase64: archive.toString("base64"),
          archiveHash: "0".repeat(64),
          requestId: randomUUID(),
          destinationActorId: probeUser.userId,
          confirm: true,
        });
        expect(wrongHash.status()).toBe(422);
        const wrongActor = await restoreAt({
          archiveBase64: archive.toString("base64"),
          archiveHash,
          requestId: randomUUID(),
          destinationActorId: destinationUser.userId,
          confirm: true,
        });
        expect(wrongActor.status()).toBe(400);
        // Refused previews/confirmations created no project.
        const probeProjects = await okJson(
          ppage.request.get(`/api/v1/workspaces/${probePersonal.id}/projects`),
          z.object({ items: z.array(idOf) }),
        );
        expect(probeProjects.items).toEqual([]);
        record(out, "repacked-preflight.json", {
          status: repackedPreflight.status(),
          body: await repackedPreflight.text(),
        });
        const repackedHash = sha256(repacked);
        expect(repackedPreflight.status(), "identical re-packed entries stay valid").toBe(200);

        // ---- Preflight + explicit confirmation through the destination UI.
        await dpage.goto(`/w/${personal.slug}/settings`);
        const restoreSection = dpage
          .locator(".native-archive")
          .filter({ has: dpage.getByRole("button", { name: /복원할 네이티브 보관 파일 선택/ }) });
        await expect(restoreSection.getByText(/별도 설치의 빈 개인 워크스페이스/)).toBeVisible();
        await expect(restoreSection.getByText(/SHA-256 해시는 파일 변경 여부/)).toBeVisible();
        const chooser = dpage.waitForEvent("filechooser");
        await restoreSection
          .getByRole("button", { name: /복원할 네이티브 보관 파일 선택/ })
          .click();
        let restoreRequestId = "";
        dpage.on("request", (request) => {
          if (request.method() === "POST" && request.url().endsWith("/native-archive/restore")) {
            const body = z
              .object({ requestId: z.string() })
              .passthrough()
              .parse(request.postDataJSON());
            restoreRequestId = body.requestId;
          }
        });
        await (await chooser).setFiles(archivePath);
        await expect(restoreSection.getByText(archiveHash)).toBeVisible({ timeout: 60000 });
        await expect(restoreSection.getByText(destinationUser.userId)).toBeVisible();
        await expect(restoreSection.getByText(personal.id)).toBeVisible();
        await expect(restoreSection.getByText(ws)).toBeVisible();
        await restoreSection.getByRole("checkbox").check();
        await restoreSection.getByRole("button", { name: "확인한 내용 복원" }).click();
        await expect(restoreSection.getByRole("status")).toContainText("복원이 완료되었습니다", {
          timeout: 120000,
        });
        expect(restoreRequestId).toMatch(UUID);
        expect(dcsp).toEqual([]);

        // Response-loss replay: same command returns the same completed job, no second graph.
        const replay = await dpage.request.post(
          `/api/v1/workspaces/${personal.id}/native-archive/restore`,
          {
            data: {
              archiveBase64: archive.toString("base64"),
              archiveHash,
              requestId: restoreRequestId,
              destinationActorId: destinationUser.userId,
              confirm: true,
            },
          },
        );
        expect(replay.status(), await replay.text()).toBe(202);
        const replayJob = jobSchema.parse(await replay.json());
        expect([replayJob.status, replayJob.archiveHash, replayJob.projectId]).toEqual([
          "completed",
          archiveHash,
          project.id,
        ]);
        // Same command with different (valid) bytes is a generic conflict.
        const mismatch = await dpage.request.post(
          `/api/v1/workspaces/${personal.id}/native-archive/restore`,
          {
            data: {
              archiveBase64: repacked.toString("base64"),
              archiveHash: repackedHash,
              requestId: restoreRequestId,
              destinationActorId: destinationUser.userId,
              confirm: true,
            },
          },
        );
        expect(mismatch.status()).toBe(409);
        // A nonempty destination refuses another restore before effects.
        const again = await dpage.request.post(
          `/api/v1/workspaces/${personal.id}/native-archive/preflight`,
          {
            data: { archiveBase64: archive.toString("base64") },
          },
        );
        expect(again.status()).toBe(409);
        // Another actor cannot read this job.
        const foreignStatus = await ppage.request.get(
          `/api/v1/workspaces/${personal.id}/native-archive/jobs/${replayJob.id}`,
        );
        expect(foreignStatus.status()).toBe(403);
      } finally {
        await probe.close();
      }

      // ---- Fresh destination client: ordinary API/UI, files, revisions, native history.
      const fresh = await browser.newContext({ baseURL: destinationInstall.url });
      try {
        const fpage = await fresh.newPage();
        const fcsp = watchCspViolations(fpage);
        await login(fpage, DESTINATION_OWNER.email, DESTINATION_OWNER.password);
        const pws = personal.id;
        const restoredProject = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/projects/${project.id}`),
          projectSchema,
        );
        expect(restoredProject).toMatchObject({
          id: project.id,
          key: FIXTURE.projectKey,
          name: FIXTURE.projectName,
          visibility: "private",
          rootDocumentId: root.id,
          createdBy: destinationUser.userId,
        });
        const restoredDoc = await okJson(
          fpage.request.get(
            `/api/v1/workspaces/${pws}/projects/${project.id}/documents/${document.id}`,
          ),
          documentMeta,
        );
        expect(restoredDoc).toMatchObject({
          id: document.id,
          number: document.number,
          title: FIXTURE.documentTitle,
          parentId: root.id,
        });
        const restoredTask = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/tasks/${task.id}`),
          taskSchema,
        );
        expect(restoredTask).toMatchObject({
          id: task.id,
          number: task.number,
          title: FIXTURE.taskTitle,
          type: FIXTURE.taskType,
          priority: FIXTURE.taskPriority,
          statusId: task.statusId,
          startDate: FIXTURE.startDate,
          dueDate: FIXTURE.dueDate,
          createdBy: destinationUser.userId,
          assigneeIds: [destinationUser.userId],
          labelIds: [label.id],
        });
        const restoredLabels = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/projects/${project.id}/labels`),
          z.object({ items: z.array(labelSchema) }),
        );
        expect(
          restoredLabels.items.map((item) => ({ id: item.id, name: item.name, color: item.color })),
        ).toEqual([{ id: label.id, ...FIXTURE.label }]);
        // Comments keep IDs, bodies, reply and resolution; the author and the
        // reaction are the destination user's (reactedByMe for that client).
        const restoredDocComments = await okJson(
          fpage.request.get(
            `/api/v1/workspaces/${pws}/projects/${project.id}/documents/${document.id}/comments`,
          ),
          commentList,
        );
        const restoredTaskComments = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/tasks/${task.id}/comments`),
          commentList,
        );
        const visibleComment = (c: z.infer<typeof commentSchema>) => ({
          id: c.id,
          parentId: c.parentId,
          createdBy: c.createdBy,
          body: c.body,
          resolved: c.resolvedAt !== null,
          reactions: c.reactions,
        });
        expect(
          [...restoredDocComments.items, ...restoredTaskComments.items]
            .map(visibleComment)
            .sort((a, b) => a.id.localeCompare(b.id)),
        ).toEqual(
          [
            {
              id: topComment.id,
              parentId: null,
              createdBy: destinationUser.userId,
              body: FIXTURE.comments.top.body,
              resolved: true,
              reactions: { "👍": { count: 1, reactedByMe: true } },
            },
            {
              id: replyComment.id,
              parentId: topComment.id,
              createdBy: destinationUser.userId,
              body: FIXTURE.comments.reply.body,
              resolved: false,
              reactions: {},
            },
            {
              id: taskComment.id,
              parentId: null,
              createdBy: destinationUser.userId,
              body: FIXTURE.comments.task.body,
              resolved: false,
              reactions: {},
            },
          ].sort((a, b) => a.id.localeCompare(b.id)),
        );
        expect(appRoleRead(destinationApp, pws, commentsSelect(document.id, task.id))).toEqual(
          storedComments(destinationUser.userId),
        );
        const restoredWorkflow = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/projects/${project.id}/workflow`),
          workflowSchema,
        );
        expect(restoredWorkflow.id).toBe(workflow.id);
        expect(restoredWorkflow.statuses.map((s) => s.id)).toEqual(
          workflow.statuses.map((s) => s.id),
        );
        const origin = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/tasks/${task.id}/origin`),
          z.object({
            items: z.array(
              z
                .object({
                  taskId: z.string(),
                  documentId: z.string(),
                  anchor: z.string().nullable(),
                })
                .passthrough(),
            ),
          }),
        );
        expect(origin.items).toHaveLength(1);
        expect(origin.items[0]).toMatchObject({
          taskId: task.id,
          documentId: document.id,
          anchor: FIXTURE.taskOriginAnchor,
        });
        const restoredActivity = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/tasks/${task.id}/activity`),
          activityList,
        );
        const restoredCreated = restoredActivity.items.find(
          (item) => item.id === createdActivityId,
        );
        expect(restoredCreated).toMatchObject({ type: "change", kind: "created" });
        expect(restoredCreated?.actor ?? null).toBeNull();
        // The changed activities keep their IDs; stored assignee identities
        // are the destination actor (labels keep their historical name) and
        // attribution is NULL; the API shows the destination person's name.
        const destActor = destinationUser.userId;
        for (const [expectedChanged, visible] of [
          [
            labelActivity,
            [
              {
                field: "assigneeIds",
                from: list(ref(destActor, FIXTURE.destinationPersonName)),
                to: list(),
              },
              { field: "labelIds", from: list(), to: list(ref(label.id, FIXTURE.label.name)) },
            ],
          ],
          [
            reassignActivity,
            [
              {
                field: "assigneeIds",
                from: list(),
                to: list(ref(destActor, FIXTURE.destinationPersonName)),
              },
            ],
          ],
        ] as const) {
          const restoredChanged = restoredActivity.items.find(
            (item) => item.id === expectedChanged.id,
          );
          expect(restoredChanged).toMatchObject({ type: "change", kind: "changed" });
          expect(restoredChanged?.actor ?? null).toBeNull();
          expect(restoredChanged?.changes).toEqual(visible);
        }
        expect(appRoleRead(destinationApp, pws, changedActivitySelect(task.id))).toEqual({
          [labelActivity.id]: [
            { field: "assigneeIds", from: [ref(destActor, null)], to: [] },
            { field: "labelIds", from: [], to: [ref(label.id, FIXTURE.label.name)] },
          ],
          [reassignActivity.id]: [{ field: "assigneeIds", from: [], to: [ref(destActor, null)] }],
        });
        for (const [fileId, literal] of [
          [docFile, FIXTURE.files.document],
          [taskFile, FIXTURE.files.task],
        ] as const) {
          const download = await fpage.request.get(
            `/api/v1/workspaces/${pws}/attachments/${fileId}/download`,
          );
          expect(download.ok(), await download.text()).toBe(true);
          expect(sha256(await download.body())).toBe(literal.sha256);
        }
        const docRevisionBase = `/api/v1/workspaces/${pws}/projects/${project.id}/documents/${document.id}/revisions`;
        const taskRevisionBase = `/api/v1/workspaces/${pws}/tasks/${task.id}/revisions`;
        const rootRevisionBase = `/api/v1/workspaces/${pws}/projects/${project.id}/documents/${root.id}/revisions`;
        const destinationBases: Record<string, string> = {
          [root.id]: rootRevisionBase,
          [document.id]: docRevisionBase,
          [task.id]: taskRevisionBase,
        };
        for (const [target, base] of Object.entries(destinationBases)) {
          const list = await okJson(fpage.request.get(base), revisionList);
          expect(list.items.map((r) => r.id).sort()).toEqual(sourceRevisionLists[target]);
        }
        const revision = (target: string, id: string) =>
          okJson(fpage.request.get(`${target}/${id}`), revisionDetail);

        // Independent Yjs reader over the destination's persisted native store,
        // compared with facts frozen from the SOURCE before export.
        const destDocRow = nativeRow.parse(
          appRoleRead(destinationApp, pws, nativeSelect("document", document.id)),
        );
        const destTaskRow = nativeRow.parse(
          appRoleRead(destinationApp, pws, nativeSelect("task", task.id)),
        );
        // Binding re-proved at the destination, receipts remapped only in actor.
        checkBinding(destDocRow, destinationUser.userId);
        checkBinding(destTaskRow, destinationUser.userId);
        expect({ document: receiptFacts(destDocRow), task: receiptFacts(destTaskRow) }).toEqual(
          sourceReceipts,
        );
        expect([destDocRow.cutoff, destDocRow.tail, destTaskRow.cutoff, destTaskRow.tail]).toEqual([
          sourceDocRow.cutoff,
          sourceDocRow.tail,
          sourceTaskRow.cutoff,
          sourceTaskRow.tail,
        ]);
        expect([destDocRow.content, destTaskRow.content]).toEqual([
          sourceDocRow.content,
          sourceTaskRow.content,
        ]);
        const destDoc = nativeDoc(destDocRow);
        const destTask = nativeDoc(destTaskRow);
        expect({ document: rootFacts(destDoc), task: rootFacts(destTask) }).toEqual(sourceRoots);
        expect(fullNativeStructure(destDoc)).toEqual(fullJsonStructure(sourceDocRow.content));
        expect(fullNativeStructure(destTask)).toEqual(fullJsonStructure(sourceTaskRow.content));
        const destPlan = [
          { id: docBefore.id, base: docRevisionBase, doc: destDoc, expected: expectedDocBefore },
          { id: docAfter.id, base: docRevisionBase, doc: destDoc, expected: expectedDocAfter },
          {
            id: taskBefore.id,
            base: taskRevisionBase,
            doc: destTask,
            expected: expectedTaskBefore,
          },
          { id: taskAfter.id, base: taskRevisionBase, doc: destTask, expected: expectedTaskAfter },
        ];
        for (const entry of destPlan) {
          const detail = await revision(entry.base, entry.id);
          expect(detail.ySnapshot).toBe(sourceRevisionSnapshots[entry.id]);
          expect(detail.contentJson).toEqual(sourceRevisionJson[entry.id]);
          expect(jsonStructure(detail.contentJson)).toEqual(entry.expected);
          expect(historicalStructure(entry.doc, detail.ySnapshot)).toEqual(entry.expected);
          // Deleted items' payloads (phrase, old mention) via full historical structure.
          expect(fullHistorical(entry.doc, detail.ySnapshot)).toEqual(
            fullJsonStructure(sourceRevisionJson[entry.id]),
          );
        }
        const destRoot = nativeDoc(
          nativeRow.parse(appRoleRead(destinationApp, pws, nativeSelect("document", root.id))),
        );
        const destTargetDocs: Record<string, Y.Doc> = {
          [root.id]: destRoot,
          [document.id]: destDoc,
          [task.id]: destTask,
        };
        for (const automatic of automaticRevisions) {
          const base = destinationBases[automatic.target];
          const doc = destTargetDocs[automatic.target];
          if (!base || !doc) throw new Error("automatic revision target missing");
          const detail = await revision(base, automatic.id);
          expect(detail.ySnapshot).toBe(sourceRevisionSnapshots[automatic.id]);
          expect(detail.contentJson).toEqual(sourceRevisionJson[automatic.id]);
          expect(fullHistorical(doc, detail.ySnapshot)).toEqual(
            fullJsonStructure(sourceRevisionJson[automatic.id]),
          );
        }
        expect(nativeStructure(destDoc)).toEqual(expectedDocAfter);
        expect(nativeStructure(destTask)).toEqual(expectedTaskAfter);
        expect(Y.equalSnapshots(Y.snapshot(destDoc), sourceSnapshot.document)).toBe(true);
        expect(Y.equalSnapshots(Y.snapshot(destTask), sourceSnapshot.task)).toBe(true);
        expect(itemAt(destDoc, 1, 0)).toEqual(mentionAfter);
        expect([itemState(destDoc, mentionBefore), itemState(destDoc, mentionAfter)]).toEqual([
          "deleted",
          "live",
        ]);
        expect(resolveAt(destDoc)).toBe(markedStart);
        const destinationRoleWitness = roleWitness(destinationApp, pws);
        // Same baseline collection/item identities and stored rows after restore.
        const destCollection = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/projects/${project.id}/collection`),
          collectionSchema,
        );
        const destItem = await okJson(
          fpage.request.get(`/api/v1/workspaces/${pws}/tasks/${task.id}/collection-item`),
          itemLookupSchema,
        );
        expect([destCollection.id, destItem.item.id, destItem.item.collectionId]).toEqual([
          sourceCollection.id,
          sourceItem.item.id,
          sourceCollection.id,
        ]);
        expect(appRoleRead(destinationApp, pws, baselineSelect(project.id))).toEqual(
          sourceBaseline,
        );
        const restoredEvent = z
          .object({ actor: z.string(), count: z.number() })
          .parse(
            appRoleRead(
              destinationApp,
              pws,
              `SELECT jsonb_build_object('actor', max(actor_user_id::text), 'count', count(*)) FROM fvoci.events WHERE verb = 'native_archive.restored' AND target_id = '${project.id}'`,
            ),
          );
        expect(restoredEvent).toEqual({ actor: destinationUser.userId, count: 1 });

        // Ordinary UI shows the restored body. A peer inserts text inside the
        // anchored paragraph: references, attachment and all history stay, and
        // the frozen relative position moves with its character.
        await openItem(fpage, personal.slug, item(document.number), docScope);
        await expect(fpage.locator(`${docScope} .fvoci-editor .ProseMirror`)).toContainText(
          FIXTURE.documentAfter,
        );
        await edit(fpage, docScope, {
          kind: "insertAfter",
          find: "수정 후 ",
          text: FIXTURE.peerEdit,
        });
        await persist(fpage, docScope);
        const peerRow = await waitDurable(
          destinationApp,
          pws,
          "document",
          document.id,
          expectedDocPeer,
          destinationUser.userId,
        );
        const peerDoc = nativeDoc(peerRow);
        // Old ACK bindings survive the peer edit unchanged; only the edited
        // paragraph's text differs from the frozen source body.
        for (const old of sourceReceipts.document)
          expect(receiptFacts(peerRow)).toContainEqual(old);
        // Full expected peer body = frozen source body with exactly the declared
        // text change in the anchored paragraph (all marks/attrs retained).
        const sourceFull = fullJsonStructure(sourceDocRow.content);
        const expectedPeerFull = structuredClone(sourceFull);
        const firstRun = expectedPeerFull[0]?.content[0];
        if (!firstRun || !("text" in firstRun) || firstRun.text !== "수정 후 ")
          throw new Error("frozen source anchored paragraph changed");
        firstRun.text = `수정 후 ${FIXTURE.peerEdit}`;
        expect(fullJsonStructure(peerRow.content)).toEqual(expectedPeerFull);
        expect(fullNativeStructure(peerDoc)).toEqual(expectedPeerFull);
        expect(rootFacts(peerDoc)).toEqual(sourceRoots.document);
        const peerRevision = await okJson(fpage.request.post(docRevisionBase), idOf);
        const afterPeer = await okJson(fpage.request.get(docRevisionBase), revisionList);
        expect(afterPeer.items.map((r) => r.id).sort()).toEqual(
          [docBefore.id, docAfter.id, peerRevision.id].sort(),
        );
        for (const [id, structure] of [
          [docBefore.id, expectedDocBefore],
          [docAfter.id, expectedDocAfter],
        ] as const) {
          const old = await revision(docRevisionBase, id);
          expect(old.ySnapshot).toBe(sourceRevisionSnapshots[id]);
          expect(old.contentJson).toEqual(sourceRevisionJson[id]);
          expect(jsonStructure(old.contentJson)).toEqual(structure);
          expect(historicalStructure(peerDoc, old.ySnapshot)).toEqual(structure);
          expect(fullHistorical(peerDoc, old.ySnapshot)).toEqual(
            fullJsonStructure(sourceRevisionJson[id]),
          );
        }
        const newest = await revision(docRevisionBase, peerRevision.id);
        expect(jsonStructure(newest.contentJson)).toEqual(expectedDocPeer);
        expect(historicalStructure(peerDoc, newest.ySnapshot)).toEqual(expectedDocPeer);
        expect(fullJsonStructure(newest.contentJson)).toEqual(expectedPeerFull);
        expect(fullHistorical(peerDoc, newest.ySnapshot)).toEqual(expectedPeerFull);
        expect([itemState(peerDoc, mentionBefore), itemState(peerDoc, mentionAfter)]).toEqual([
          "deleted",
          "live",
        ]);
        expect(resolveAt(peerDoc)).toBe(markedStart + FIXTURE.peerEdit.length);
        const sourceVector = Y.decodeStateVector(Y.encodeStateVector(sourceDoc));
        const peerVector = Y.decodeStateVector(Y.encodeStateVector(peerDoc));
        for (const [client, clock] of sourceVector)
          expect(peerVector.get(client) ?? 0).toBeGreaterThanOrEqual(clock);
        expect([...peerVector.keys()].some((client) => !sourceVector.has(client))).toBe(true);
        expect(fcsp).toEqual([]);
        record(out, "observed-after-restore.json", {
          archiveHash,
          archiveBytes: archive.length,
          entries: names.length,
          destinationWorkspaceId: pws,
          destinationActorId: destinationUser.userId,
          peerRevision: peerRevision.id,
          peerRelativeIndex: resolveAt(peerDoc),
          destinationServerEnvNames: destinationInstall.envNames,
          destinationRoleWitness,
        });
      } finally {
        await fresh.close();
      }
    } finally {
      await destination.close();
    }
  } finally {
    await source.close();
    await destinationInstall.stop();
  }
});

// ---------------------------------------------------------------------------
// Z2: the real W6 producer. Source installation S runs the W6 Zotero CLI (fake
// upstream mode 22, no real network) as its server; the owner connects, syncs,
// captures a personal-input task and authors native bodies and two manual
// revisions; the selected connector is exported with the project. A separate
// destination installation D restores through the ordinary W7 server, which is
// then stopped while D's database and storage stay owned here; the W6 CLI
// serves D with that storage lent, a fresh client checks content/history/
// origin/receipt/privacy and the disconnected mirror, then reconnects since 0.
const Z2_SOURCE_OWNER = { email: "zotero-source@example.com", password: "zoterosource123" };
const Z2_DESTINATION_OWNER = { email: "zotero-dest@example.com", password: "zoterodest123" };
const Z2_OUTSIDER = { email: "zotero-outsider@example.com", password: "zoterooutsider123" };
const Z2 = {
  captureTitle: "Compare my authored evidence",
  referenceTitle: "Authored astronomy 개인 의견",
  origin: { block: "authored-commentary", text: "내가 쓴 의견과 비교 기록" },
  revisions: [
    { block: "owned-commentary", text: "My authored telescope notes. 개인 의견은 유지됩니다." },
    { block: "owned-commentary", text: "My revised telescope comparison. 복원 후에도 유지됩니다." },
  ],
} as const;
const singleParagraph = (block: string, text: string) => ({
  type: "doc",
  content: [{ type: "paragraph", attrs: { id: block }, content: [{ type: "text", text }] }],
});
const bodySchema = z.object({ contentJson: z.unknown() }).passthrough();
function expectSingleParagraph(body: unknown, block: string, text: string): void {
  const doc = z
    .object({
      type: z.literal("doc"),
      content: z.tuple([
        z
          .object({
            type: z.literal("paragraph"),
            attrs: z.object({ id: z.string() }).passthrough(),
            content: z.tuple([
              z.object({ type: z.literal("text"), text: z.string() }).passthrough(),
            ]),
          })
          .passthrough(),
      ]),
    })
    .passthrough()
    .parse(body);
  expect([doc.content[0].attrs.id, doc.content[0].content[0].text]).toEqual([block, text]);
}

/** Restored reference/origin documents and the captured task in the ordinary UI. */
async function expectRestoredUi(
  page: Page,
  slug: string,
  doc: (id: string, suffix?: string) => string,
  reference: string,
  origin: string,
  taskNumber: number,
): Promise<void> {
  const docScope = ".document-page";
  for (const [id, text] of [
    [reference, Z2.revisions[1].text],
    [origin, Z2.origin.text],
  ] as const) {
    const meta = await okJson(page.request.get(doc(id)), documentMeta);
    await openItem(page, slug, `WIKI-${String(meta.number)}`, docScope);
    await expect(page.locator(`${docScope} .fvoci-editor .ProseMirror`)).toContainText(text);
    // The title is an editable textarea (its value is not element text).
    if (id === reference)
      await expect(
        page.locator(docScope).getByRole("textbox", { name: "문서 제목", exact: true }),
      ).toHaveValue(Z2.referenceTitle);
  }
  const taskScope = '[data-testid="task-body"]';
  const navigation = await page.goto(`/w/${slug}/INBOX-${String(taskNumber)}`);
  expect(navigation?.status()).toBe(200);
  // The task title is an input value (data-testid task-edit-title).
  await expect(page.getByTestId("task-edit-title")).toHaveValue(Z2.captureTitle, {
    timeout: 20000,
  });
  await expect(page.locator(taskScope)).toBeVisible();
}

test("native archive restores a real W6 Zotero producer closure into a separate installation and reconnects since 0", async ({
  browser,
}, testInfo) => {
  test.setTimeout(420_000);
  const out = testInfo.outputPath("zotero-pair");
  mkdirSync(out, { recursive: true });
  const sourceInstall = new Installation(path.join(out, "source"), ZOTERO_CLI_PEPPER);
  const destinationInstall = new Installation(path.join(out, "destination"), ZOTERO_CLI_PEPPER);
  const clis: ZoteroCli[] = [];
  const cleanupErrors: unknown[] = [];
  try {
    // ---- Source S: W6 CLI producer.
    sourceInstall.provision();
    sourceInstall.createUser(Z2_SOURCE_OWNER);
    const sourceCli = await sourceInstall.serveZotero();
    clis.push(sourceCli);
    const source = await browser.newContext({ baseURL: sourceCli.origin });
    let archive: Buffer | undefined;
    let ids:
      | {
          connectorId: string;
          referenceId: string;
          taskId: string;
          originDocumentId: string;
          projectId: string;
          revisionIds: [string, string];
          requestId: string;
          sourceWorkspaceId: string;
        }
      | undefined;
    let sourceReceipt: unknown;
    let sourceNative: { reference: NativeRow; origin: NativeRow } | undefined;
    const sourceRevisionFacts: Record<string, { ySnapshot: string; contentJson: unknown }> = {};
    let sourceOrigin: unknown;
    try {
      const spage = await source.newPage();
      await login(spage, Z2_SOURCE_OWNER.email, Z2_SOURCE_OWNER.password);
      const workspace = await okJson(
        spage.request.post("/api/v1/me/personal-workspace"),
        workspaceMeta,
      );
      const ws = workspace.id;
      expect(await sourceCli.send({ command: "mode", mode: 22 })).toEqual({ ok: true });
      const connected = await okJson(
        spage.request.post(`/api/v1/workspaces/${ws}/zotero`, {
          data: { ...ZOTERO_USER_LIBRARY, apiKey: ZOTERO_FIXTURE_KEY },
        }),
        idOf,
      );
      const imported = await okJson(
        spage.request.post(`/api/v1/workspaces/${ws}/zotero/libraries/${connected.id}/sync`),
        z.record(z.string(), z.unknown()),
      );
      expectMode22Import(imported);
      const reference = z
        .object({
          references: z.tuple([
            z.object({ id: z.string().regex(UUID), localVersion: z.string() }).passthrough(),
          ]),
        })
        .passthrough()
        .parse(imported).references[0];
      const requestId = randomUUID();
      const captured = await okJson(
        spage.request.post(`/api/v1/workspaces/${ws}/personal-input`, {
          data: { requestId, intent: "task", title: Z2.captureTitle },
        }),
        z
          .object({
            documentId: z.string().regex(UUID),
            taskId: z.string().regex(UUID),
            projectId: z.string().regex(UUID),
            replayed: z.literal(false),
          })
          .passthrough(),
      );
      const doc = (id: string, suffix = "") => `/api/v1/workspaces/${ws}/documents/${id}${suffix}`;
      await okJson(
        spage.request.put(doc(captured.documentId, "/body"), {
          data: { contentJson: singleParagraph(Z2.origin.block, Z2.origin.text) },
        }),
        z.unknown(),
      );
      await okJson(
        spage.request.patch(doc(reference.id), { data: { title: Z2.referenceTitle } }),
        z.unknown(),
      );
      const revisionIds: string[] = [];
      for (const revision of Z2.revisions) {
        await okJson(
          spage.request.put(doc(reference.id, "/body"), {
            data: { contentJson: singleParagraph(revision.block, revision.text) },
          }),
          z.unknown(),
        );
        revisionIds.push(
          (await okJson(spage.request.post(doc(reference.id, "/revisions")), idOf)).id,
        );
      }
      const [firstRevision, secondRevision] = revisionIds;
      if (!firstRevision || !secondRevision || firstRevision === secondRevision)
        throw new Error("two distinct manual revisions required");
      await okJson(
        spage.request.post(`/api/v1/workspaces/${ws}/zotero/references/${reference.id}/links`, {
          data: {
            documentId: null,
            taskId: captured.taskId,
            anchor: null,
            expectedVersion: reference.localVersion,
          },
        }),
        z.unknown(),
      );
      ids = {
        connectorId: connected.id,
        referenceId: reference.id,
        taskId: captured.taskId,
        originDocumentId: captured.documentId,
        projectId: captured.projectId,
        revisionIds: [firstRevision, secondRevision],
        requestId,
        sourceWorkspaceId: ws,
      };
      sourceReceipt = appRoleRead(
        sourceInstall.appUrl,
        ws,
        `SELECT jsonb_build_object('hash',request_hash,'intent',intent,'document',document_id,'task',task_id,'project',project_id,'created',created_at) FROM fvoci.personal_input_commands WHERE request_id='${requestId}'`,
      );
      sourceOrigin = appRoleRead(
        sourceInstall.appUrl,
        ws,
        `SELECT jsonb_build_object('task',task_id,'document',document_id,'request',request_id,'hash',request_hash,'anchor',anchor,'created',created_at) FROM fvoci.task_origins WHERE task_id='${captured.taskId}'`,
      );
      expect(sourceReceipt).toMatchObject({
        intent: "task",
        document: captured.documentId,
        task: captured.taskId,
        project: captured.projectId,
      });
      // Canonical native reader over S's persisted store (independent Yjs),
      // frozen before export: state/tail/receipts/body and both retained
      // revision snapshots reconstruct exactly their stored JSON.
      const sourceUser = await okJson(spage.request.get("/api/v1/auth/me"), meSchema);
      const readSource = (id: string) =>
        nativeRow.parse(appRoleRead(sourceInstall.appUrl, ws, nativeSelect("document", id)));
      await expect
        .poll(() => readSource(reference.id).text, { timeout: 20000 })
        .toContain(Z2.revisions[1].text);
      await expect
        .poll(() => readSource(captured.documentId).text, { timeout: 20000 })
        .toContain(Z2.origin.text);
      sourceNative = {
        reference: readSource(reference.id),
        origin: readSource(captured.documentId),
      };
      for (const row of [sourceNative.reference, sourceNative.origin]) {
        checkBinding(row, sourceUser.userId);
        expect(fullNativeStructure(nativeDoc(row))).toEqual(fullJsonStructure(row.content));
      }
      expectSingleParagraph(
        sourceNative.reference.content,
        Z2.revisions[1].block,
        Z2.revisions[1].text,
      );
      expectSingleParagraph(sourceNative.origin.content, Z2.origin.block, Z2.origin.text);
      for (const [index, id] of [firstRevision, secondRevision].entries()) {
        const detail = await okJson(
          spage.request.get(doc(reference.id, `/revisions/${id}`)),
          revisionDetail,
        );
        const literal = Z2.revisions[index];
        if (!literal) throw new Error("revision literal missing");
        expectSingleParagraph(detail.contentJson, literal.block, literal.text);
        expect(fullHistorical(nativeDoc(sourceNative.reference), detail.ySnapshot)).toEqual(
          fullJsonStructure(detail.contentJson),
        );
        sourceRevisionFacts[id] = { ySnapshot: detail.ySnapshot, contentJson: detail.contentJson };
      }
      // Restricted-role witness of the selected bodies that have no native
      // state yet (never opened): they travel body-only.
      const stateless = z.array(z.object({ kind: z.string(), id: z.string() })).parse(
        appRoleRead(
          sourceInstall.appUrl,
          ws,
          `SELECT coalesce(jsonb_agg(jsonb_build_object('kind',k,'id',i) ORDER BY k,i),'[]'::jsonb) FROM (
              SELECT 'document' AS k,d.id AS i FROM fvoci.documents d WHERE (d.project_id='${captured.projectId}' OR d.id IN ('${captured.documentId}','${reference.id}'))
                AND NOT EXISTS(SELECT 1 FROM fvoci.document_states s WHERE s.document_id=d.id)
              UNION ALL SELECT 'task',t.id FROM fvoci.tasks t WHERE t.project_id='${captured.projectId}'
                AND NOT EXISTS(SELECT 1 FROM fvoci.task_states s WHERE s.task_id=t.id)) q`,
        ),
      );
      expect(stateless).toContainEqual({ kind: "task", id: captured.taskId });
      record(out, "source-stateless-targets.json", stateless);
    } finally {
      await source.close();
    }
    await sourceCli.stop();
    clis.pop();
    // Export through the ordinary W7 server on S (the CLI binary is not the
    // product's office/native container helper; the server is).
    await sourceInstall.startServer();
    const exporter = await browser.newContext({ baseURL: sourceInstall.url });
    try {
      const xpage = await exporter.newPage();
      await login(xpage, Z2_SOURCE_OWNER.email, Z2_SOURCE_OWNER.password);
      const exported = await xpage.request.get(
        `/api/v1/workspaces/${ids.sourceWorkspaceId}/projects/${ids.projectId}/native-archive?zoteroConnector=${ids.connectorId}`,
      );
      expect(exported.status(), await exported.text().catch(() => "")).toBe(200);
      archive = Buffer.from(await exported.body());
    } finally {
      await exporter.close();
    }
    expect(await sourceInstall.stopServer()).toEqual({ code: 0, signal: null });
    const zip = await JSZip.loadAsync(archive);
    const graphText = await entryText(zip, "graph.json");
    // Neither the synthetic key nor any credential/sync field is archived.
    expect(graphText.includes(ZOTERO_FIXTURE_KEY)).toBe(false);
    const graph = z
      .object({
        documents: z.array(
          z.object({ id: z.string(), project_id: z.string().nullable() }).passthrough(),
        ),
        personal_input_commands: z.array(z.record(z.string(), z.unknown())),
        origins: z.array(z.record(z.string(), z.unknown())),
      })
      .passthrough()
      .parse(JSON.parse(graphText));
    expect(
      graph.documents
        .filter((d) => d.project_id === null)
        .map((d) => d.id)
        .sort(),
    ).toEqual([ids.originDocumentId, ids.referenceId].sort());
    const graphKeys = new Set<string>();
    const collectKeys = (value: unknown): void => {
      if (Array.isArray(value)) value.forEach(collectKeys);
      else if (value && typeof value === "object")
        for (const [key, child] of Object.entries(value)) {
          graphKeys.add(key);
          collectKeys(child);
        }
    };
    collectKeys(graph);
    expect(forbiddenArchiveKeys(graphKeys)).toEqual([]);

    // ---- Destination D: ordinary W7 server restore.
    destinationInstall.provision();
    const destinationUserId = destinationInstall.createUser(Z2_DESTINATION_OWNER);
    destinationInstall.createUser(Z2_OUTSIDER);
    await destinationInstall.startServer();
    const restore = await browser.newContext({ baseURL: destinationInstall.url });
    let dws = "";
    let dslug = "";
    try {
      const rpage = await restore.newPage();
      await login(rpage, Z2_DESTINATION_OWNER.email, Z2_DESTINATION_OWNER.password);
      const me = await okJson(rpage.request.get("/api/v1/auth/me"), meSchema);
      const personal = await okJson(
        rpage.request.post("/api/v1/me/personal-workspace"),
        workspaceMeta,
      );
      dws = personal.id;
      dslug = personal.slug;
      const preflight = await okJson(
        rpage.request.post(`/api/v1/workspaces/${dws}/native-archive/preflight`, {
          data: { archiveBase64: archive.toString("base64") },
        }),
        preflightSchema,
      );
      expect(preflight.projectId).toBe(ids.projectId);
      const job = jobSchema.parse(
        await (
          await rpage.request.post(`/api/v1/workspaces/${dws}/native-archive/restore`, {
            data: {
              archiveBase64: archive.toString("base64"),
              archiveHash: preflight.archiveHash,
              requestId: randomUUID(),
              destinationActorId: me.userId,
              confirm: true,
            },
          })
        ).json(),
      );
      await expect
        .poll(
          async () =>
            jobSchema.parse(
              await (
                await rpage.request.get(`/api/v1/workspaces/${dws}/native-archive/jobs/${job.id}`)
              ).json(),
            ).status,
          { timeout: 120000 },
        )
        .toBe("completed");
      expect(me.userId).toBe(destinationUserId);
    } finally {
      await restore.close();
    }
    // Clean stop witness of the W7 server (graceful drain, exit 0) before
    // the CLI reuses D's database and storage.
    expect(await destinationInstall.stopServer()).toEqual({ code: 0, signal: null });

    // ---- D served by the W6 CLI with D's storage lent; fresh client.
    const destinationCli = await destinationInstall.serveZotero();
    clis.push(destinationCli);
    const fresh = await browser.newContext({ baseURL: destinationCli.origin });
    try {
      const fpage = await fresh.newPage();
      await login(fpage, Z2_DESTINATION_OWNER.email, Z2_DESTINATION_OWNER.password);
      const restoredIds = { ...ids, destinationUserId, destinationWorkspaceId: dws };
      // The whole fresh upstream log is empty (startup and login included).
      const requestsBefore = expectNoUpstreamGet(
        await destinationCli.send({ command: "requests" }),
      );
      const observe = () =>
        destinationCli.send({
          command: "observe",
          userId: destinationUserId,
          workspaceId: dws,
          connectorId: ids.connectorId,
        });
      expectRestoredDisconnected(await observe(), restoredIds);
      const doc = (id: string, suffix = "") => `/api/v1/workspaces/${dws}/documents/${id}${suffix}`;
      // Canonical native reader over D's persisted store, compared with the
      // facts frozen from S before export (receipts remapped only in actor).
      const readDestination = (id: string) =>
        nativeRow.parse(appRoleRead(destinationInstall.appUrl, dws, nativeSelect("document", id)));
      const destinationReference = readDestination(ids.referenceId);
      for (const [row, source] of [
        [destinationReference, sourceNative.reference],
        [readDestination(ids.originDocumentId), sourceNative.origin],
      ] as const) {
        checkBinding(row, destinationUserId);
        expect(receiptFacts(row)).toEqual(receiptFacts(source));
        expect([row.cutoff, row.tail, row.state]).toEqual([
          source.cutoff,
          source.tail,
          source.state,
        ]);
        expect(row.updates).toEqual(source.updates);
        expect(row.content).toEqual(source.content);
        expect(fullNativeStructure(nativeDoc(row))).toEqual(fullJsonStructure(source.content));
      }
      for (const id of ids.revisionIds) {
        const detail = await okJson(
          fpage.request.get(doc(ids.referenceId, `/revisions/${id}`)),
          revisionDetail,
        );
        const source = sourceRevisionFacts[id];
        if (!source) throw new Error("source revision facts missing");
        expect([detail.ySnapshot, detail.contentJson]).toEqual([
          source.ySnapshot,
          source.contentJson,
        ]);
        expect(fullHistorical(nativeDoc(destinationReference), detail.ySnapshot)).toEqual(
          fullJsonStructure(source.contentJson),
        );
      }
      expect((await okJson(fpage.request.get(doc(ids.referenceId)), documentMeta)).title).toBe(
        Z2.referenceTitle,
      );
      expectSingleParagraph(
        (await okJson(fpage.request.get(doc(ids.referenceId, "/body")), bodySchema)).contentJson,
        Z2.revisions[1].block,
        Z2.revisions[1].text,
      );
      expectSingleParagraph(
        (await okJson(fpage.request.get(doc(ids.originDocumentId, "/body")), bodySchema))
          .contentJson,
        Z2.origin.block,
        Z2.origin.text,
      );
      const listed = await okJson(
        fpage.request.get(doc(ids.referenceId, "/revisions")),
        revisionList,
      );
      expect(
        listed.items.filter((r) => ids.revisionIds.includes(r.id)).map((r) => [r.id, r.reason]),
      ).toEqual(expect.arrayContaining(ids.revisionIds.map((id) => [id, "manual"])));
      for (const [index, id] of ids.revisionIds.entries()) {
        const detail = await okJson(
          fpage.request.get(doc(ids.referenceId, `/revisions/${id}`)),
          revisionDetail,
        );
        const expected = Z2.revisions[index];
        if (!expected) throw new Error("revision literal missing");
        expectSingleParagraph(detail.contentJson, expected.block, expected.text);
        expect(detail.ySnapshot.length).toBeGreaterThan(0);
      }
      expectAuthoredContent(
        {
          reference: await okJson(fpage.request.get(doc(ids.referenceId)), z.unknown()),
          referenceBody: await okJson(
            fpage.request.get(doc(ids.referenceId, "/body")),
            z.unknown(),
          ),
          originBody: await okJson(
            fpage.request.get(doc(ids.originDocumentId, "/body")),
            z.unknown(),
          ),
          revisions: await Promise.all(
            ids.revisionIds.map((id) =>
              okJson(fpage.request.get(doc(ids.referenceId, `/revisions/${id}`)), z.unknown()),
            ),
          ),
          origins: await okJson(
            fpage.request.get(doc(ids.originDocumentId, "/task-origins")),
            z.unknown(),
          ),
        },
        restoredIds,
      );
      const task = await okJson(
        fpage.request.get(`/api/v1/workspaces/${dws}/tasks/${ids.taskId}`),
        taskSchema,
      );
      expect([task.title, task.createdBy]).toEqual([Z2.captureTitle, destinationUserId]);
      // Origin relation unchanged; receipt retired (destination actor, all targets NULL).
      const destinationOrigin = appRoleRead(
        destinationInstall.appUrl,
        dws,
        `SELECT jsonb_build_object('task',task_id,'document',document_id,'request',request_id,'hash',request_hash,'anchor',anchor,'created',created_at) FROM fvoci.task_origins WHERE task_id='${ids.taskId}'`,
      );
      expect(destinationOrigin).toEqual(sourceOrigin);
      const receiptSql = `SELECT jsonb_build_object('actor',actor_user_id,'hash',request_hash,'intent',intent,'document',document_id,'task',task_id,'project',project_id,'created',created_at) FROM fvoci.personal_input_commands WHERE request_id='${ids.requestId}'`;
      const retired = z
        .record(z.string(), z.unknown())
        .parse(appRoleRead(destinationInstall.appUrl, dws, receiptSql));
      const sourceFacts = z.record(z.string(), z.unknown()).parse(sourceReceipt);
      expect(retired).toEqual({
        actor: destinationUserId,
        hash: sourceFacts.hash,
        intent: "task",
        document: null,
        task: null,
        project: null,
        created: sourceFacts.created,
      });
      // The imported receipt is never a live replay target: 409 and the whole
      // affected restricted-role graph (rows, native tails, history, origin,
      // receipts, events) is unchanged.
      const graphSql = `SELECT jsonb_build_object(
        'documents',(SELECT jsonb_agg(to_jsonb(d)-'workspace_id' ORDER BY id) FROM fvoci.documents d),
        'tasks',(SELECT jsonb_agg((to_jsonb(t)-'workspace_id')||jsonb_build_object('estimate',t.estimate::text) ORDER BY id) FROM fvoci.tasks t),
        'states',(SELECT jsonb_agg((to_jsonb(s)-'workspace_id'-'state')||jsonb_build_object('stateSha',encode(sha256(s.state),'hex')) ORDER BY document_id) FROM fvoci.document_states s),
        'taskStates',(SELECT jsonb_agg((to_jsonb(s)-'workspace_id'-'state')||jsonb_build_object('stateSha',encode(sha256(s.state),'hex')) ORDER BY task_id) FROM fvoci.task_states s),
        'updates',(SELECT jsonb_agg(jsonb_build_object('target',document_id,'seq',seq,'op',op_id,'sha',encode(sha256(payload),'hex')) ORDER BY document_id,seq) FROM fvoci.document_collab_updates),
        'taskUpdates',(SELECT jsonb_agg(jsonb_build_object('target',task_id,'seq',seq,'op',op_id,'sha',encode(sha256(payload),'hex')) ORDER BY task_id,seq) FROM fvoci.task_collab_updates),
        'opReceipts',(SELECT jsonb_agg((to_jsonb(r)-'workspace_id'-'payload_sha256')||jsonb_build_object('sha',encode(r.payload_sha256,'hex')) ORDER BY document_id,seq,op_id) FROM fvoci.document_collab_op_receipts r),
        'taskOpReceipts',(SELECT jsonb_agg((to_jsonb(r)-'workspace_id'-'payload_sha256')||jsonb_build_object('sha',encode(r.payload_sha256,'hex')) ORDER BY task_id,seq,op_id) FROM fvoci.task_collab_op_receipts r),
        'revisions',(SELECT jsonb_agg((to_jsonb(r)-'workspace_id'-'y_snapshot')||jsonb_build_object('snapshotSha',encode(sha256(r.y_snapshot),'hex')) ORDER BY id) FROM fvoci.revisions r),
        'origins',(SELECT jsonb_agg(to_jsonb(o)-'workspace_id' ORDER BY task_id) FROM fvoci.task_origins o),
        'receipts',(SELECT jsonb_agg(to_jsonb(c)-'workspace_id' ORDER BY request_id) FROM fvoci.personal_input_commands c),
        'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY e.seq) FROM fvoci.events e))`;
      const graphBefore = appRoleRead(destinationInstall.appUrl, dws, graphSql);
      const replay = await fpage.request.post(`/api/v1/workspaces/${dws}/personal-input`, {
        data: { requestId: ids.requestId, intent: "task", title: Z2.captureTitle },
      });
      expectRetiredReceiptReplay(
        replay.status(),
        graphBefore,
        appRoleRead(destinationInstall.appUrl, dws, graphSql),
      );
      // Owner-private: another destination account sees no mirror.
      const outsider = await browser.newContext({ baseURL: destinationCli.origin });
      try {
        const opage = await outsider.newPage();
        await login(opage, Z2_OUTSIDER.email, Z2_OUTSIDER.password);
        const me = await opage.request.get("/api/v1/auth/me");
        const reads = [
          (await opage.request.get(`/api/v1/workspaces/${dws}/zotero`)).status(),
          (
            await opage.request.get(`/api/v1/workspaces/${dws}/zotero/libraries/${ids.connectorId}`)
          ).status(),
        ] as const;
        expectPrivateZoteroDenied(me.status(), await me.json(), reads, restoredIds);
      } finally {
        await outsider.close();
      }
      // Archive metadata is present before any reconnect could repair it.
      expectRestoredLibrary(
        await okJson(
          fpage.request.get(`/api/v1/workspaces/${dws}/zotero/libraries/${ids.connectorId}`),
          z.unknown(),
        ),
        restoredIds,
      );
      // The ordinary product UI shows the restored reference, origin and task
      // (after the replay snapshot, so live editor rooms cannot touch it).
      await expectRestoredUi(fpage, dslug, doc, ids.referenceId, ids.originDocumentId, task.number);
      // Nothing contacted the upstream before the explicit reconnect.
      const beforeReconnect = expectNoUpstreamGet(
        await destinationCli.send({ command: "requests" }),
        requestsBefore,
      );
      // Explicit reconnect (normal mode) and since-0 reconciliation.
      expect(await destinationCli.send({ command: "mode", mode: 0 })).toEqual({ ok: true });
      const reconnected = await okJson(
        fpage.request.post(`/api/v1/workspaces/${dws}/zotero`, {
          data: { ...ZOTERO_USER_LIBRARY, apiKey: ZOTERO_FIXTURE_KEY },
        }),
        idOf,
      );
      expect(reconnected.id).toBe(ids.connectorId);
      const synced = await okJson(
        fpage.request.post(`/api/v1/workspaces/${dws}/zotero/libraries/${ids.connectorId}/sync`),
        z.record(z.string(), z.unknown()),
      );
      expectReconnectedSince0(
        synced,
        await observe(),
        await destinationCli.send({ command: "requests" }),
        restoredIds,
        beforeReconnect,
      );
      // A genuinely NEW client after since-0: authored content, history, origin,
      // task link and the ordinary UI are unchanged by the reconnect.
      const after = await browser.newContext({ baseURL: destinationCli.origin });
      try {
        const apage = await after.newPage();
        await login(apage, Z2_DESTINATION_OWNER.email, Z2_DESTINATION_OWNER.password);
        expectAuthoredContent(
          {
            reference: await okJson(apage.request.get(doc(ids.referenceId)), z.unknown()),
            referenceBody: await okJson(
              apage.request.get(doc(ids.referenceId, "/body")),
              z.unknown(),
            ),
            originBody: await okJson(
              apage.request.get(doc(ids.originDocumentId, "/body")),
              z.unknown(),
            ),
            revisions: await Promise.all(
              ids.revisionIds.map((id) =>
                okJson(apage.request.get(doc(ids.referenceId, `/revisions/${id}`)), z.unknown()),
              ),
            ),
            origins: await okJson(
              apage.request.get(doc(ids.originDocumentId, "/task-origins")),
              z.unknown(),
            ),
          },
          restoredIds,
        );
        const library = z
          .object({
            references: z.array(
              z
                .object({
                  id: z.string(),
                  links: z.array(z.object({ taskId: z.string().nullable() }).passthrough()),
                })
                .passthrough(),
            ),
          })
          .passthrough()
          .parse(
            await okJson(
              apage.request.get(`/api/v1/workspaces/${dws}/zotero/libraries/${ids.connectorId}`),
              z.unknown(),
            ),
          );
        expect(
          library.references.find((r) => r.id === ids.referenceId)?.links.map((l) => l.taskId),
        ).toEqual([ids.taskId]);
        const afterTask = await okJson(
          apage.request.get(`/api/v1/workspaces/${dws}/tasks/${ids.taskId}`),
          taskSchema,
        );
        await expectRestoredUi(
          apage,
          dslug,
          doc,
          ids.referenceId,
          ids.originDocumentId,
          afterTask.number,
        );
      } finally {
        await after.close();
      }
      record(out, "observed-zotero-pair.json", {
        archiveBytes: archive.length,
        ids: restoredIds,
        sourceCliEnvNames: sourceCli.envNames,
        destinationCliEnvNames: destinationCli.envNames,
        destinationServerEnvNames: destinationInstall.envNames,
      });
    } finally {
      await fresh.close();
    }
    await destinationCli.stop();
    clis.pop();
    // The lent store survived the CLI; only this test removes it.
    expect(statSync(destinationInstall.storageDir).isDirectory()).toBe(true);
  } finally {
    // Every owned child is reaped before any disposal; every cleanup step is
    // attempted. The first test failure stays the reported one; cleanup
    // failures are recorded and only fail an otherwise passing test (below).
    const attempt = async (step: () => unknown): Promise<void> => {
      try {
        await step();
      } catch (error) {
        cleanupErrors.push(error);
      }
    };
    for (const cli of clis) await attempt(() => cli.kill());
    await attempt(() => sourceInstall.stopServer());
    await attempt(() => destinationInstall.stopServer());
    await attempt(() => {
      destinationInstall.dispose();
    });
    await attempt(() => {
      sourceInstall.dispose();
    });
    if (cleanupErrors.length)
      testInfo.annotations.push({
        type: "cleanup-failure",
        description: cleanupErrors.map(String).join("; "),
      });
  }
  if (cleanupErrors.length) throw new AggregateError(cleanupErrors, "Z2 cleanup failed");
});

// Harness-only checks for the Z2 lifetime helpers (no product server or DB
// effect): a reply timeout poisons the CLI line protocol so a late line can
// never answer a later command, a fake child is always reaped (also after a
// startup or assertion failure), and installation cleanup attempts every
// resource it created, keeps a failed one for retry and touches nothing it
// did not create.

/**
 * Spawns a fake CLI child and always reaps it, also when startup or `body`
 * fails. The first failure is the one thrown; a reap failure after a body
 * failure is recorded in `cleanupErrors`, and a reap failure alone throws.
 */
async function withFakeCli(
  script: string,
  log: string,
  onSpawn: (child: ChildProcess) => void,
  body: (cli: ZoteroCli) => Promise<void>,
  cleanupErrors: unknown[],
  reap: (child: ChildProcess) => Promise<void> = reapChild,
): Promise<void> {
  const child = spawn("sh", ["-c", script], { detached: true, stdio: ["pipe", "pipe", "pipe"] });
  onSpawn(child);
  let failure: { error: unknown } | null = null;
  try {
    await body(await ZoteroCli.start(child, log, ["PATH"]));
  } catch (error) {
    failure = { error };
  }
  try {
    await reap(child);
  } catch (error) {
    if (!failure) throw error;
    cleanupErrors.push(error);
  }
  if (failure) throw failure.error;
}

/** Removes each owned directory independently; returns every failure. */
function removeOwnedDirs(
  dirs: string[],
  remove: (dir: string) => void = (dir) => {
    rmSync(dir, { recursive: true, force: true });
  },
): unknown[] {
  const errors: unknown[] = [];
  for (const dir of dirs) {
    try {
      remove(dir);
    } catch (error) {
      errors.push(error);
    }
  }
  return errors;
}
const exitedChild = (child: ChildProcess | undefined): boolean =>
  child !== undefined && (child.exitCode !== null || child.signalCode !== null);

test("Z2 harness: a late CLI reply poisons the protocol and cleanup attempts every owned resource", async ({
  playwright,
}, testInfo) => {
  expect(playwright.request).toBeDefined(); // no product server is used here
  const ready = `printf '%s\\n' '{"origin":"http://127.0.0.1:9"}'`;
  const script = [
    ready,
    "read line",
    "sleep 2",
    `printf '%s\\n' '{"late":true}'`,
    "read line",
    `printf '%s\\n' '{"second":true}'`,
    "sleep 30",
  ].join("; ");
  const cleanupErrors: unknown[] = [];
  let child: ChildProcess | undefined;
  await withFakeCli(
    script,
    testInfo.outputPath("fake-cli.log"),
    (spawned) => (child = spawned),
    async (cli) => {
      await expect(cli.send({ command: "slow" }, 500)).rejects.toThrow("reply deadline");
      await expect(cli.send({ command: "next" })).rejects.toThrow("protocol retired");
    },
    cleanupErrors,
  );
  expect(exitedChild(child)).toBe(true);
  // Failure paths: an assertion failure inside the body and an invalid
  // readiness line both still reap the live child before the error surfaces.
  let failing: ChildProcess | undefined;
  await expect(
    withFakeCli(
      `${ready}; sleep 30`,
      testInfo.outputPath("fake-cli-assert.log"),
      (spawned) => (failing = spawned),
      () => Promise.reject(new Error("intentional assertion failure")),
      cleanupErrors,
    ),
  ).rejects.toThrow("intentional assertion failure");
  expect(exitedChild(failing)).toBe(true);
  // A reap failure after the child actually exited: the body's failure is
  // still the one thrown and the cleanup failure is recorded; alone, the
  // cleanup failure fails.
  const failingReap = async (spawned: ChildProcess): Promise<void> => {
    await reapChild(spawned);
    throw new Error("injected cleanup failure");
  };
  const recorded: unknown[] = [];
  let reaped: ChildProcess | undefined;
  await expect(
    withFakeCli(
      `${ready}; sleep 30`,
      testInfo.outputPath("fake-cli-reap.log"),
      (spawned) => (reaped = spawned),
      () => Promise.reject(new Error("original body failure")),
      recorded,
      failingReap,
    ),
  ).rejects.toThrow("original body failure");
  expect(exitedChild(reaped)).toBe(true);
  expect(recorded.map(String)).toEqual(["Error: injected cleanup failure"]);
  let reapedAlone: ChildProcess | undefined;
  await expect(
    withFakeCli(
      `${ready}; sleep 30`,
      testInfo.outputPath("fake-cli-reap-alone.log"),
      (spawned) => (reapedAlone = spawned),
      () => Promise.resolve(),
      recorded,
      failingReap,
    ),
  ).rejects.toThrow("injected cleanup failure");
  expect(exitedChild(reapedAlone)).toBe(true);
  let unready: ChildProcess | undefined;
  await expect(
    withFakeCli(
      "printf 'not json\\n'; sleep 30",
      testInfo.outputPath("fake-cli-start.log"),
      (spawned) => (unready = spawned),
      () => Promise.resolve(),
      cleanupErrors,
    ),
  ).rejects.toThrow();
  expect(exitedChild(unready)).toBe(true);
  // Owned directories: every one is attempted even when an earlier removal
  // fails after removing it; all failures are returned.
  const owned = [testInfo.outputPath("owned-a"), testInfo.outputPath("owned-b")];
  for (const dir of owned) mkdirSync(dir, { recursive: true });
  const removalErrors = removeOwnedDirs(owned, (dir) => {
    rmSync(dir, { recursive: true, force: true });
    if (dir === owned[0]) throw new Error("injected removal failure");
  });
  expect(removalErrors.map(String)).toEqual(["Error: injected removal failure"]);
  expect(owned.map((dir) => existsSync(dir))).toEqual([false, false]);

  class FlakyInstallation extends Installation {
    calls: string[] = [];
    failDatabase = true;
    constructor(dir: string) {
      super(dir);
      mkdirSync(this.storageDir, { recursive: true });
      this.created.storage = true;
      this.created.database = true;
      this.created.role = true;
    }
    protected override psql(_database: string, sql: string): void {
      this.calls.push(sql.split(" ").slice(0, 2).join(" "));
      if (sql.startsWith("DROP DATABASE") && this.failDatabase)
        throw new Error("injected drop failure");
    }
  }
  // This test owns the fake stores; they are removed in finally even if an
  // assertion fails (the first failure still surfaces).
  const flakyDir = testInfo.outputPath("flaky");
  const preexistingDir = testInfo.outputPath("preexisting");
  try {
    const flaky = new FlakyInstallation(flakyDir);
    expect(() => {
      flaky.dispose();
    }).toThrow("installation cleanup failed");
    // The role and the store were still attempted after the database failed.
    expect(flaky.calls).toEqual(["DROP DATABASE", "DROP ROLE"]);
    expect(existsSync(flaky.storageDir)).toBe(false);
    // A retry resumes only the failed resource; a third call has nothing left.
    flaky.failDatabase = false;
    flaky.dispose();
    flaky.dispose();
    expect(flaky.calls).toEqual(["DROP DATABASE", "DROP ROLE", "DROP DATABASE"]);

    // A pre-existing store is refused before anything is created and its
    // sentinel survives until this test removes its own directory.
    const sentinel = path.join(preexistingDir, "storage", "sentinel");
    mkdirSync(path.dirname(sentinel), { recursive: true });
    writeFileSync(sentinel, "foreign");
    const preexisting = new Installation(preexistingDir);
    expect(() => {
      preexisting.provision();
    }).toThrow("installation storage already exists");
    preexisting.dispose();
    expect(readFileSync(sentinel, "utf8")).toBe("foreign");
  } finally {
    cleanupErrors.push(...removeOwnedDirs([flakyDir, preexistingDir]));
  }
  // Only reached when the test body passed: cleanup failures still fail it.
  if (cleanupErrors.length) throw new AggregateError(cleanupErrors, "harness cleanup failed");
});

// Proposed test-only delta; NOTRUN until Root source-GO and frozen runtime START.
test("native archive restores document tag search filters into a separate installation", async ({
  browser,
  baseURL,
}, testInfo) => {
  const meiliUrl = process.env.FVOCI_MEILI_URL;
  const sourceIndex = process.env.FVOCI_MEILI_INDEX ?? "fvoci";
  const observerKey = process.env.MEILI_MASTER_KEY;
  const sourceApp = process.env.DATABASE_APP_URL;
  if (!baseURL || !meiliUrl || !observerKey || !sourceApp)
    throw new Error("real source app-role/Meili fixture missing");
  const out = testInfo.outputPath("native-tags");
  mkdirSync(out, { recursive: true });
  const destinationIndex = `native_tags_${randomBytes(8).toString("hex")}`;
  const destinationInstall = new Installation(out, undefined, {
    url: meiliUrl,
    index: destinationIndex,
  });
  const token = `w8tag${randomBytes(8).toString("hex")}`;
  const body = `${token} 실제 보존 본문 한글🙂`;
  const titles = [`${token} 태그 문서`, `${token} 태그 없는 문서`];
  const tagSchema = z
    .object({
      id: z.string().uuid(),
      workspaceId: z.string().uuid(),
      name: z.string(),
      color: z.string(),
    })
    .passthrough();
  const tagsSchema = z.object({ items: z.array(tagSchema) });
  const searchSchema = z.object({
    items: z.array(
      z
        .object({
          id: z.string().uuid(),
          type: z.string(),
          title: z.string(),
          workspaceId: z.string().uuid(),
          snippet: z.array(z.object({ text: z.string(), match: z.boolean() })).nullable(),
        })
        .passthrough(),
    ),
    nextCursor: z.string().nullable(),
  });
  const candidatesSchema = z.object({
    hits: z.array(
      z
        .object({
          id: z.string(),
          kind: z.literal("document"),
          documentId: z.string().uuid(),
          workspaceId: z.string().uuid(),
          projectId: z.string().uuid(),
        })
        .passthrough(),
    ),
  });
  const source = await browser.newContext({ baseURL });
  const errors: unknown[] = [];
  try {
    const page = await source.newPage();
    await login(page, SOURCE_OWNER.email, SOURCE_OWNER.password);
    const actor = await okJson(page.request.get("/api/v1/auth/me"), meSchema);
    const workspace = (
      await okJson(page.request.get("/api/v1/me/workspaces"), workspaces)
    ).items.find((item) => item.slug === "native-source");
    if (!workspace) throw new Error("ordinary source workspace missing");
    const ws = workspace.id;
    const project = await okJson(
      page.request.post(`/api/v1/workspaces/${ws}/projects`, {
        data: { key: "W8TAG", name: "태그 검색 복원 🧪", visibility: "private" },
      }),
      projectSchema,
    );
    const documents = [];
    for (const title of titles)
      documents.push(
        await okJson(
          page.request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/documents`, {
            data: { title, parentId: project.rootDocumentId },
          }),
          documentMeta,
        ),
      );
    const tagged = documents[0];
    const untagged = documents[1];
    if (!tagged || !untagged) throw new Error("two ordinary documents missing");
    const tag = await okJson(
      page.request.post(`/api/v1/workspaces/${ws}/document-tags`, {
        data: { name: "파란 복원 태그 🧪", color: "blue" },
      }),
      tagSchema,
    );
    const assignment = await page.request.post(
      `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${tagged.id}/tags`,
      {
        data: { tagId: tag.id },
      },
    );
    expect(assignment.status(), await assignment.text()).toBe(200);
    await openItem(page, workspace.slug, `W8TAG-${String(tagged.number)}`, ".document-page");
    await edit(page, ".document-page", {
      kind: "set",
      content: {
        type: "doc",
        content: [{ type: "paragraph", content: [{ type: "text", text: body }] }],
      },
    });
    await persist(page, ".document-page");
    const stored = await waitDurable(
      sourceApp,
      ws,
      "document",
      tagged.id,
      [
        {
          type: "paragraph",
          runs: [
            { text: `${token} 실제 보존 본문 한글`, marks: [] },
            { emoji: "slightly_smiling_face" },
          ],
        },
      ],
      actor.userId,
    );
    const expectedIds = [tagged.id, untagged.id].sort();
    const candidates = async (index: string, tenant: string) => {
      // Test observer uses fetch, so its synthetic preparation key is not written
      // into Playwright request traces or any evidence; normal servers use scoped keyFILE.
      const response = await fetch(`${meiliUrl}/indexes/${index}/search`, {
        method: "POST",
        headers: { "content-type": "application/json", Authorization: `Bearer ${observerKey}` },
        body: JSON.stringify({
          q: token,
          filter: `workspaceId = "${tenant}" AND kind = "document"`,
          limit: 20,
        }),
      });
      expect(response.status, "real Meili candidate observer HTTP").toBe(200);
      const hits = candidatesSchema.parse(await response.json()).hits;
      const indexedSchema = candidatesSchema.shape.hits.element.extend({
        title: z.string(),
        body: z.string(),
      });
      return Promise.all(
        hits.map(async (hit) => {
          // Product search returns identity candidates only; SQL hydrates content/ACL.
          // The private test observer separately verifies the indexed document bytes.
          expect(hit.workspaceId).toBe(tenant);
          expect(hit.projectId).toBe(project.id);
          expect(Object.hasOwn(hit, "title")).toBe(false);
          expect(Object.hasOwn(hit, "body")).toBe(false);
          const indexedResponse = await fetch(
            `${meiliUrl}/indexes/${index}/documents/${encodeURIComponent(hit.id)}`,
            { headers: { Authorization: `Bearer ${observerKey}` } },
          );
          expect(indexedResponse.status, "real Meili indexed document observer HTTP").toBe(200);
          const indexed = indexedSchema.parse(await indexedResponse.json());
          for (const key of ["id", "kind", "documentId", "workspaceId", "projectId"] as const)
            expect(indexed[key], `indexed identity matches search candidate ${key}`).toBe(hit[key]);
          return { ...hit, title: indexed.title, body: indexed.body };
        }),
      );
    };
    const search = async (request: APIRequestContext, tenant: string, tagId?: string) => {
      const query = `q=${token}&type=document${tagId ? `&tag=${tagId}` : ""}`;
      const replies = [];
      for (const route of [`/api/v1/workspaces/${tenant}/search`, "/api/v1/search"]) {
        const response = await request.get(`${route}?${query}`);
        expect(response.status(), await response.text()).toBe(200);
        replies.push(searchSchema.parse(await response.json()));
      }
      return replies;
    };
    await expect
      .poll(async () => (await candidates(sourceIndex, ws)).map((item) => item.documentId).sort())
      .toEqual(expectedIds);
    const sourceCandidates = await candidates(sourceIndex, ws);
    expect(
      sourceCandidates
        .map((item) => ({ id: item.id, title: item.title, workspaceId: item.workspaceId }))
        .sort((a, b) => a.id.localeCompare(b.id)),
    ).toEqual(
      documents
        .map((item) => ({ id: `document_${item.id}`, title: item.title, workspaceId: ws }))
        .sort((a, b) => a.id.localeCompare(b.id)),
    );
    expect(sourceCandidates.find((item) => item.documentId === tagged.id)?.body).toBe(body);
    for (const reply of await search(page.request, ws))
      expect(reply.items.map((item) => item.id).sort()).toEqual(expectedIds);
    const sourceFiltered = await search(page.request, ws, tag.id);
    for (const reply of sourceFiltered) {
      expect(reply.nextCursor).toBeNull();
      expect(
        reply.items.map((item) => ({
          id: item.id,
          type: item.type,
          title: item.title,
          workspaceId: item.workspaceId,
        })),
      ).toEqual([{ id: tagged.id, type: "document", title: titles[0], workspaceId: ws }]);
      expect(reply.items[0]?.snippet?.map((piece) => piece.text).join("")).toBe(body);
    }
    const probe = { email: "native-tags-probe@example.com", password: "nativetagsprobe123" };
    createE2eUser(probe.email, probe.password, "태그 권한 검증", {
      workspaceSlug: "native-source",
      membershipRole: "member",
    });
    const unauthorized = await browser.newContext({ baseURL });
    try {
      const ppage = await unauthorized.newPage();
      await login(ppage, probe.email, probe.password);
      const denied = await search(ppage.request, ws, tag.id);
      for (const reply of denied) {
        expect(reply.items).toEqual([]);
        expect(JSON.stringify(reply)).not.toContain(token);
      }
      const privateRead = await ppage.request.get(
        `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${tagged.id}`,
      );
      expect(privateRead.status()).toBe(404);
      expect(await privateRead.text()).not.toContain(token);
    } finally {
      try {
        await unauthorized.close();
      } catch (error) {
        errors.push(error);
      }
    }
    record(out, "expected-before-export.json", {
      ws,
      project: project.id,
      documents,
      tag,
      body,
      sourceCandidates,
      sourceFiltered,
    });
    await page.goto(`/w/${workspace.slug}/settings`);
    await page.getByLabel("보관할 프로젝트").selectOption(project.id);
    const exporter = page
      .locator(".native-archive")
      .filter({ has: page.getByRole("button", { name: "네이티브 보관 파일 다운로드" }) });
    const response = page.waitForResponse((res) =>
      res.url().endsWith(`/projects/${project.id}/native-archive`),
    );
    const download = page.waitForEvent("download");
    download.catch(() => undefined);
    await exporter.getByRole("button", { name: "네이티브 보관 파일 다운로드" }).click();
    expect((await response).status()).toBe(200);
    const archivePath = path.join(out, "native-tags.zip");
    await (await download).saveAs(archivePath);
    const archive = readFileSync(archivePath);
    const archiveHash = sha256(archive);
    await destinationInstall.start();
    checkServerEnvNames(destinationInstall.envNames);
    expect(destinationInstall.envNames).toEqual(
      expect.arrayContaining(["FVOCI_MEILI_URL", "FVOCI_MEILI_INDEX", "FVOCI_MEILI_KEY_FILE"]),
    );
    const destination = await browser.newContext({ baseURL: destinationInstall.url });
    let restoredWorkspace = "";
    let restoredSlug = "";
    try {
      const dpage = await destination.newPage();
      await setupThroughUi(dpage, DESTINATION_OWNER, {
        name: "태그 검색 설치",
        slug: "native-tags-destination",
      });
      const personal = await okJson(
        dpage.request.post("/api/v1/me/personal-workspace"),
        workspaceMeta,
      );
      restoredWorkspace = personal.id;
      restoredSlug = personal.slug;
      await dpage.goto(`/w/${personal.slug}/settings`);
      const importer = dpage
        .locator(".native-archive")
        .filter({ has: dpage.getByRole("button", { name: /복원할 네이티브 보관 파일 선택/ }) });
      const chooser = dpage.waitForEvent("filechooser");
      await importer.getByRole("button", { name: /복원할 네이티브 보관 파일 선택/ }).click();
      await (await chooser).setFiles(archivePath);
      await expect(importer.getByText(archiveHash)).toBeVisible();
      await importer.getByRole("checkbox").check();
      await importer.getByRole("button", { name: "확인한 내용 복원" }).click();
      await expect(importer.getByRole("status")).toContainText("복원이 완료되었습니다");
    } finally {
      try {
        await destination.close();
      } catch (error) {
        errors.push(error);
      }
    }
    const fresh = await browser.newContext({ baseURL: destinationInstall.url });
    try {
      const fpage = await fresh.newPage();
      await login(fpage, DESTINATION_OWNER.email, DESTINATION_OWNER.password);
      const restoredActor = await okJson(fpage.request.get("/api/v1/auth/me"), meSchema);
      expect(restoredWorkspace).not.toBe(ws);
      expect(restoredActor.userId).not.toBe(actor.userId);
      const current = nativeRow.parse(
        appRoleRead(
          destinationInstall.appUrl,
          restoredWorkspace,
          nativeSelect("document", tagged.id),
        ),
      );
      expect(current.content).toEqual(stored.content);
      expect(current.text).toBe(body);
      expect([
        current.state,
        current.cutoff,
        current.tail,
        current.updates,
        receiptFacts(current),
      ]).toEqual([stored.state, stored.cutoff, stored.tail, stored.updates, receiptFacts(stored)]);
      checkBinding(current, restoredActor.userId);
      expect(fullNativeStructure(nativeDoc(current))).toEqual(fullJsonStructure(stored.content));
      const restoredTags = await okJson(
        fpage.request.get(
          `/api/v1/workspaces/${restoredWorkspace}/projects/${project.id}/documents/${tagged.id}/tags`,
        ),
        tagsSchema,
      );
      expect(
        restoredTags.items.map(({ id, workspaceId, name, color }) => ({
          id,
          workspaceId,
          name,
          color,
        })),
      ).toEqual([{ id: tag.id, workspaceId: restoredWorkspace, name: tag.name, color: tag.color }]);
      expect(
        (
          await okJson(
            fpage.request.get(
              `/api/v1/workspaces/${restoredWorkspace}/projects/${project.id}/documents/${untagged.id}/tags`,
            ),
            tagsSchema,
          )
        ).items,
      ).toEqual([]);
      await expect
        .poll(async () =>
          (await candidates(destinationIndex, restoredWorkspace))
            .map((item) => item.documentId)
            .sort(),
        )
        .toEqual(expectedIds);
      const restoredCandidates = await candidates(destinationIndex, restoredWorkspace);
      expect(
        restoredCandidates
          .map((item) => ({ id: item.id, title: item.title, body: item.body }))
          .sort((a, b) => a.id.localeCompare(b.id)),
      ).toEqual(
        sourceCandidates
          .map((item) => ({ id: item.id, title: item.title, body: item.body }))
          .sort((a, b) => a.id.localeCompare(b.id)),
      );
      for (const reply of await search(fpage.request, restoredWorkspace))
        expect(reply.items.map((item) => item.id).sort()).toEqual(expectedIds);
      const filtered = await search(fpage.request, restoredWorkspace, tag.id);
      expect(filtered).toEqual(
        sourceFiltered.map((reply) => ({
          ...reply,
          items: reply.items.map((item) => ({ ...item, workspaceId: restoredWorkspace })),
        })),
      );
      await openItem(fpage, restoredSlug, `W8TAG-${String(tagged.number)}`, ".document-page");
      await expect(fpage.locator(".document-page .fvoci-editor .ProseMirror")).toContainText(body);
      record(out, "observed-current54-tags.json", {
        sourceIndex,
        destinationIndex,
        archiveHash,
        ws,
        restoredWorkspace,
        tagged: tagged.id,
        untagged: untagged.id,
        tag: tag.id,
        sourceCandidates,
        restoredCandidates,
        filtered,
        sourceRoles: roleWitness(sourceApp, ws),
        destinationRoles: roleWitness(destinationInstall.appUrl, restoredWorkspace),
        normalServerEnvNames: destinationInstall.envNames,
      });
    } finally {
      try {
        await fresh.close();
      } catch (error) {
        errors.push(error);
      }
    }
    destinationInstall.createUser(probe);
    const outsider = await browser.newContext({ baseURL: destinationInstall.url });
    try {
      const ppage = await outsider.newPage();
      await login(ppage, probe.email, probe.password);
      const global = await ppage.request.get(
        `/api/v1/search?q=${token}&type=document&tag=${tag.id}`,
      );
      expect(global.status(), await global.text()).toBe(200);
      const denied = searchSchema.parse(await global.json());
      expect(denied.items).toEqual([]);
      expect(JSON.stringify(denied)).not.toContain(token);
      const workspaceRead = await ppage.request.get(
        `/api/v1/workspaces/${restoredWorkspace}/search?q=${token}&type=document&tag=${tag.id}`,
      );
      expect(workspaceRead.status()).toBe(404);
      expect(await workspaceRead.text()).not.toContain(token);
    } finally {
      try {
        await outsider.close();
      } catch (error) {
        errors.push(error);
      }
    }
  } catch (error) {
    errors.unshift(error);
  }
  // Keep the primary assertion first and attempt both owned cleanup steps even
  // if either fails; throw only after every attempt, outside a finally block.
  try {
    await source.close();
  } catch (error) {
    errors.push(error);
  }
  try {
    await destinationInstall.stop();
  } catch (error) {
    errors.push(error);
  }
  if (errors.length) throw new AggregateError(errors, "native tag oracle or cleanup failed");
});
