import { createHash } from "node:crypto";
import { chmodSync, lstatSync, readFileSync, writeFileSync } from "node:fs";
import { expect, type APIRequestContext, type APIResponse, type Page } from "@playwright/test";
import JSZip from "jszip";
import { z } from "zod";

export const LITERALS = {
  before: "복원할 한국어 본문 🙂",
  after: "교체된 최신 본문 🧪",
  attachment: "별도 설치 첨부 bytes\n한글🙂\n",
  number: 0.12345678901234566,
  date: "2026-10-03",
  instant: "2026-10-03T01:02:03.123456Z",
  instantApi: "2026-10-03T01:02:03.123Z",
  instantArchive: "2026-10-03T01:02:03.123456+00:00",
};
const object = z.record(z.string(), z.unknown());
export const workspaceSchema = z.object({ id: z.guid(), slug: z.string() });
export const metaSchema = z.object({ id: z.guid(), number: z.number().int() }).loose();
export const projectSchema = z
  .object({ id: z.guid(), key: z.string(), rootDocumentId: z.guid() })
  .loose();
export const identitySchema = z.object({ userId: z.guid(), sessionId: z.guid() });
export const jobSchema = z.object({
  id: z.guid(),
  status: z.string(),
  archiveHash: z.string(),
  projectId: z.guid().nullable(),
});
export const sourceSchema = z.object({
  actor: z.guid(),
  team: workspaceSchema,
  workspace: workspaceSchema,
  project: projectSchema,
  document: metaSchema,
  sourceRevision: z.guid(),
  restoredRevision: z.guid(),
  restoredBody: z.unknown(),
  restoredTail: z.string(),
  history: z.unknown(),
  firstAck: z.guid(),
  finalAck: z.guid(),
  correlation: z.guid(),
  expectedTail: z.string(),
});
export const checkpointSchema = z.object({
  source: sourceSchema,
  archive: z
    .object({
      base64: z.string(),
      sha256: z.string(),
      graph: object,
      body: z.unknown(),
      task: z.unknown(),
      origin: z.unknown(),
      attachment: z.guid(),
      collection: z.guid(),
      values: z.record(z.string(), z.unknown()),
    })
    .optional(),
  destination: z.object({ actor: z.guid(), workspace: workspaceSchema, job: jobSchema }).optional(),
});
export type Checkpoint = z.infer<typeof checkpointSchema>;
export const hash = (bytes: Uint8Array): string => createHash("sha256").update(bytes).digest("hex");

export function checkpointPath(): string {
  const path = process.env.FVOCI_E2E_RESTORE_INPUT;
  if (!path?.startsWith("/")) throw new Error("Allocated private restore input required");
  return path;
}
export function readCheckpoint(): Checkpoint {
  const path = checkpointPath();
  const facts = lstatSync(path);
  expect(facts.isFile() && !facts.isSymbolicLink()).toBe(true);
  expect(facts.mode & 0o777).toBe(0o600);
  return checkpointSchema.parse(JSON.parse(readFileSync(path, "utf8")));
}
export function writeCheckpoint(value: Checkpoint): void {
  const path = checkpointPath();
  writeFileSync(path, JSON.stringify(value), { mode: 0o600 });
  chmodSync(path, 0o600);
}
export async function json<T>(
  response: Promise<APIResponse>,
  schema: z.ZodType<T>,
  status = 200,
): Promise<T> {
  const received = await response;
  expect(received.status()).toBe(status);
  return schema.parse(await received.json());
}
export const jsonObject = (response: Promise<APIResponse>, status = 200) =>
  json(response, object, status);

export function documentBase(state: Checkpoint, workspace = state.source.workspace.id): string {
  return `/api/v1/workspaces/${workspace}/projects/${state.source.project.id}/documents/${state.source.document.id}`;
}
export function documentUrl(state: Checkpoint, slug = state.source.workspace.slug): string {
  return `/w/${slug}/${state.source.project.key}-${String(state.source.document.number)}`;
}
export async function archive(request: APIRequestContext, workspace: string, project: string) {
  const response = await request.get(
    `/api/v1/workspaces/${workspace}/projects/${project}/native-archive`,
  );
  expect(response.status()).toBe(200);
  const bytes = Buffer.from(await response.body());
  return decodeArchive(bytes);
}
export async function decodeArchive(bytes: Buffer) {
  const zip = await JSZip.loadAsync(bytes);
  const graphEntry = zip.file("graph.json");
  if (!graphEntry) throw new Error("Current native graph entry required");
  const graph = object.parse(JSON.parse(await graphEntry.async("text")));
  const manifestEntry = zip.file("manifest.json");
  if (!manifestEntry) throw new Error("Current native manifest entry required");
  const manifest = z
    .object({
      kind: z.literal("fvoci-native-user-archive"),
      format_version: z.literal(1),
      native_encoding: z.number().int(),
      complete: z.literal(true),
      required_models: z.array(z.string()),
      mapping_policy: z.string(),
      captured_at: z.string(),
      entries: z.record(
        z.string(),
        z.object({ size: z.number().int(), sha256: z.string().regex(/^[a-f0-9]{64}$/) }),
      ),
    })
    .strict()
    .parse(JSON.parse(await manifestEntry.async("text")));
  expect(manifest.captured_at).toBe(graph.captured_at);
  expect(new Set(Object.keys(zip.files))).toEqual(
    new Set(["manifest.json", ...Object.keys(manifest.entries)]),
  );
  for (const [name, expected] of Object.entries(manifest.entries)) {
    const entry = zip.file(name);
    if (!entry) throw new Error("Manifest member missing");
    const payload = await entry.async("nodebuffer");
    expect(payload.length).toBe(expected.size);
    expect(hash(payload)).toBe(expected.sha256);
  }
  return { bytes, graph, zip, manifest };
}

/** Alter only a capture association while keeping the ZIP manifest valid, so
 * the real product native validator must reject the rebound report. */
export async function reboundArchive(bytes: Buffer): Promise<Buffer> {
  const { zip, graph, manifest } = await decodeArchive(bytes);
  const inventory = rows(graph, "native_inventory");
  const first = inventory[0];
  if (!first) throw new Error("Actual native inventory required");
  const original = z
    .string()
    .regex(/^[a-f0-9]{64}$/)
    .parse(first.binding);
  first.binding = original === "0".repeat(64) ? "1".repeat(64) : "0".repeat(64);
  graph.native_inventory = inventory;
  const encoded = Buffer.from(JSON.stringify(graph));
  manifest.entries["graph.json"] = { size: encoded.length, sha256: hash(encoded) };
  zip.file("graph.json", encoded);
  zip.file("manifest.json", JSON.stringify(manifest));
  return zip.generateAsync({ type: "nodebuffer" });
}
export function rows(graph: Record<string, unknown>, name: string): Record<string, unknown>[] {
  return z.array(object).parse(graph[name]);
}

/** Explicit single-author identity mapping and capture metadata only.
 * Activity attribution is inert on restore. Authored JSON is never rewritten;
 * array order, numeric/time values, native bytes and content IDs stay exact. */
export function remappedGraph(
  source: Record<string, unknown>,
  destination: Record<string, unknown>,
  actor: string,
  workspace: string,
): Record<string, unknown> {
  const oldActor = z.guid().parse(source.source_actor_id);
  expect(destination.source_actor_id).toBe(actor);
  expect(destination.source_workspace_id).toBe(workspace);
  expect(actor).not.toBe(oldActor);
  expect(workspace).not.toBe(source.source_workspace_id);
  const captured = z.iso.datetime({ offset: true }).parse(destination.captured_at);
  expect(Date.parse(captured)).toBeGreaterThanOrEqual(
    Date.parse(z.string().parse(source.captured_at)),
  );
  const result = structuredClone(source);
  result.source_actor_id = actor;
  result.source_workspace_id = workspace;
  result.captured_at = captured;
  const identity = (row: Record<string, unknown>, field: string) => {
    if (row[field] === null) return;
    expect(row[field]).toBe(oldActor);
    row[field] = actor;
  };
  const project = object.parse(result.project);
  identity(project, "created_by");
  result.project = project;
  for (const [table, field] of [
    ["documents", "created_by"],
    ["tasks", "created_by"],
    ["revisions", "created_by"],
    ["attachments", "uploader_id"],
    ["assignees", "user_id"],
    ["views", "user_id"],
    ["comments", "created_by"],
    ["collection_people", "user_id"],
    ["collection_views", "owner_id"],
    ["time_entries", "user_id"],
    ["timer_runs", "user_id"],
    ["timer_segments", "user_id"],
    ["timer_legacy_open", "user_id"],
    ["timer_commands", "user_id"],
    ["timer_audit", "user_id"],
  ]) {
    if (!table || !field) throw new Error("Fixed identity path required");
    result[table] = rows(result, table).map((row) => {
      identity(row, field);
      return row;
    });
  }
  result.states = rows(result, "states").map((state) => ({
    ...state,
    receipts: z
      .array(object)
      .parse(state.receipts)
      .map((receipt) => {
        identity(receipt, "actor_user_id");
        return receipt;
      }),
  }));
  result.activity = rows(result, "activity").map((activity) => {
    expect(activity.actor_user_id === oldActor || activity.actor_user_id === null).toBe(true);
    activity.actor_user_id = null;
    activity.changes = z
      .array(object)
      .parse(activity.changes)
      .map((change) => {
        if (change.field === "assigneeIds")
          for (const field of ["from", "to"]) {
            change[field] = z
              .array(object)
              .parse(change[field])
              .map((ref) => {
                expect(ref.id).toBe(oldActor);
                return { ...ref, id: actor };
              });
          }
        return change;
      });
    return activity;
  });
  const current = rows(destination, "native_inventory");
  const previous = rows(result, "native_inventory");
  expect(current).toHaveLength(previous.length);
  expect(previous.length).toBeGreaterThan(0);
  result.native_inventory = previous.map((inventory, index) => {
    const binding = z
      .string()
      .regex(/^[a-f0-9]{64}$/)
      .parse(current[index]?.binding);
    expect(
      z
        .string()
        .regex(/^[a-f0-9]{64}$/)
        .parse(inventory.binding),
    ).not.toBe(binding);
    return { ...inventory, binding };
  });
  return result;
}

export async function upload(
  request: APIRequestContext,
  base: string,
  workspace: string,
): Promise<string> {
  const bytes = Buffer.from(LITERALS.attachment);
  const opened = await json(
    request.post(base + "/uploads", { data: { name: "복구-자료.txt", sizeBytes: bytes.length } }),
    z.object({
      attachmentId: z.guid(),
      partSizeBytes: z.number(),
      parts: z.array(z.object({ partNumber: z.number(), url: z.string() })),
    }),
    201,
  );
  const parts: { partNumber: number; etag: string }[] = [];
  for (const part of opened.parts) {
    const response = await request.put(part.url, {
      data: bytes.subarray(
        (part.partNumber - 1) * opened.partSizeBytes,
        part.partNumber * opened.partSizeBytes,
      ),
      headers: { "content-type": "application/octet-stream" },
    });
    expect(response.status()).toBe(200);
    const etag = response.headers().etag;
    if (!etag) throw new Error("Actual upload ETag required");
    parts.push({ partNumber: part.partNumber, etag });
  }
  const complete = await request.post(
    `/api/v1/workspaces/${workspace}/attachments/${opened.attachmentId}/complete`,
    { data: { parts } },
  );
  expect(complete.status()).toBe(200);
  return opened.attachmentId;
}

export async function collectionValues(
  request: APIRequestContext,
  workspace: string,
  collection: string,
) {
  return json(
    request.post(`/api/v1/workspaces/${workspace}/collections/${collection}/query`, {
      data: {
        config: { query: { filters: {}, sort: [] }, groupBy: null, dateBy: null },
        limit: 100,
      },
    }),
    z.object({
      items: z.array(
        z
          .object({
            id: z.guid(),
            taskId: z.guid().nullable().optional(),
            version: z.number(),
            values: z.record(z.string(), z.unknown()),
          })
          .loose(),
      ),
    }),
  );
}

export async function restoredReadback(page: Page, state: Checkpoint): Promise<void> {
  const { source, destination, archive: stored } = state;
  if (!destination || !stored) throw new Error("Actual restore checkpoint required");
  const workspace = destination.workspace.id;
  const base = documentBase(state, workspace);
  const body = await jsonObject(page.request.get(base + "/body"));
  expect(body.contentJson).toEqual(object.parse(stored.body).contentJson);
  const history = await jsonObject(page.request.get(base + "/revisions"));
  const previous = object.parse(source.history);
  expect(history).toEqual({
    ...previous,
    items: z
      .array(object)
      .parse(previous.items)
      .map((item) => ({
        ...item,
        createdBy: item.createdBy === source.actor ? destination.actor : item.createdBy,
      })),
  });
  const revision = await jsonObject(
    page.request.get(`${base}/revisions/${source.restoredRevision}`),
  );
  expect(revision).toMatchObject({
    id: source.restoredRevision,
    reason: "restore",
    restoredFromId: source.sourceRevision,
    contentJson: source.restoredBody,
  });
  const taskId = z.guid().parse(object.parse(stored.task).id);
  const task = await jsonObject(
    page.request.get(`/api/v1/workspaces/${workspace}/tasks/${taskId}`),
  );
  expect(task).toMatchObject({
    id: taskId,
    number: object.parse(stored.task).number,
    startDate: LITERALS.date,
    dueAt: LITERALS.instant,
  });
  const origin = await jsonObject(
    page.request.get(`/api/v1/workspaces/${workspace}/tasks/${taskId}/origin`),
  );
  expect(origin).toEqual(stored.origin);
  const file = await page.request.get(
    `/api/v1/workspaces/${workspace}/attachments/${stored.attachment}/download`,
  );
  expect(file.status()).toBe(200);
  expect(Buffer.from(await file.body())).toEqual(Buffer.from(LITERALS.attachment));
  const values = await collectionValues(page.request, workspace, stored.collection);
  expect(values.items).toHaveLength(1);
  expect(values.items[0]?.values).toEqual(stored.values);
  const job = await json(
    page.request.get(`/api/v1/workspaces/${workspace}/native-archive/jobs/${destination.job.id}`),
    jobSchema,
  );
  expect(job).toEqual({ ...destination.job, status: "completed", projectId: source.project.id });
}
