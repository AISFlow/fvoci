import { randomUUID } from "node:crypto";
import { chmodSync, lstatSync, readFileSync, writeFileSync } from "node:fs";
import { expect, test, type Page, type TestInfo } from "@playwright/test";
import { MessageType } from "@hocuspocus/provider";
import * as decoding from "lib0/decoding";
import { messageYjsSyncStep2, messageYjsUpdate } from "y-protocols/sync";
import { z } from "zod";
import type { Editor } from "@tiptap/core";
import type { components } from "../src/generated/api";
import { observeRestoreCounts } from "../../../tools/turso/ui-restore-observe";
import { restoreStateDigest, verifyRestoreStage } from "../../../tools/turso/ui-restore";
import { frameBytes, persistParts } from "./collab-wire";
import {
  admin,
  attachCollabWire,
  clearEditor,
  closeCollabContext,
  ensureInstanceSetup,
  expectMatchingPersistAck,
  login,
  newCollabContext,
  openEditor,
  persistBody,
  type CollabWireLog,
} from "./collab-helpers";
import {
  LITERALS,
  archive,
  collectionValues,
  decodeArchive,
  documentBase,
  documentUrl,
  hash,
  identitySchema,
  jobSchema,
  json,
  jsonObject,
  metaSchema,
  projectSchema,
  readCheckpoint,
  reboundArchive,
  remappedGraph,
  restoredReadback,
  rows,
  upload,
  workspaceSchema,
  writeCheckpoint,
  type Checkpoint,
} from "./turso-restore-helpers";

type Schema = components["schemas"];
type NativeArchive = Awaited<ReturnType<typeof archive>>;

// This companion is separate from the immutable restore-response checkpoint.
// Only native content archives and allowlisted save witnesses enter it.
function stagePath(): string {
  const path = process.env.FVOCI_E2E_RESTORE_INPUT;
  if (!path?.startsWith("/")) throw new Error("RESTORE_STAGE_INPUT_MISSING");
  return path + ".stage.private.json";
}
async function previousStage(): Promise<NativeArchive | null> {
  try {
    const facts = lstatSync(stagePath());
    expect(
      facts.isFile() &&
        !facts.isSymbolicLink() &&
        facts.size <= 64 * 1024 * 1024 &&
        (facts.mode & 0o777) === 0o600,
    ).toBe(true);
    const saved = z
      .object({ base64: z.string() })
      .strict()
      .parse(JSON.parse(readFileSync(stagePath(), "utf8")));
    return await decodeArchive(Buffer.from(z.string().parse(saved.base64), "base64"));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw new Error("RESTORE_STAGE_INPUT_INVALID", { cause: error });
  }
}

async function archiveRow(captured: NativeArchive, document: string) {
  const native = rows(captured.graph, "states").filter(
    (row) => row.target_kind === "document" && row.target_id === document,
  );
  expect(native).toHaveLength(1);
  const state = native[0];
  if (!state) throw new Error("RESTORE_STAGE_STATE_MISSING");
  const entry = captured.zip.file(z.string().parse(state.state_entry));
  if (!entry) throw new Error("RESTORE_STAGE_STATE_MISSING");
  expect(state.encoding).toBe(1);
  const updates = await Promise.all(
    rows(state, "updates").map(async (update) => {
      const payload = captured.zip.file(z.string().parse(update.payload_entry));
      if (!payload) throw new Error("RESTORE_STAGE_UPDATE_MISSING");
      return {
        seq: update.seq,
        op: update.op_id,
        hex: (await payload.async("nodebuffer")).toString("hex"),
      };
    }),
  );
  const body = rows(captured.graph, "documents").find((row) => row.id === document);
  if (!body) throw new Error("RESTORE_STAGE_DOCUMENT_MISSING");
  return {
    state: (await entry.async("nodebuffer")).toString("hex"),
    cutoff: state.snapshot_cutoff_seq,
    tail: state.tail_seq,
    content: body.content_json,
    updates,
    receipts: rows(state, "receipts").map((receipt) => ({
      seq: receipt.seq,
      op: receipt.op_id,
      len: receipt.payload_len,
      sha: receipt.payload_sha256,
      actor: receipt.actor_user_id,
    })),
  };
}

/** Installed lib0/Yjs codecs read only document sync writes and SyncStatus.
 * Auth, awareness and unknown frame bodies are never read or retained here;
 * stateless capture retains only the existing allowlisted persist UUIDs. */
function freshSyncWitness(page: Page, routingKey: string, before: Record<string, unknown>) {
  const sync: Array<{
    sha: string;
    len: number;
    empty: boolean;
    applied: boolean;
    fullStateUnchanged: boolean;
  }> = [];
  let invalid = false;
  let connections = 0;
  let order = 0;
  const syncOrder: number[] = [];
  const persistOrder = new Map<string, number>();
  const wire: CollabWireLog = { sent: [], received: [] };
  page.on("websocket", (ws) => {
    connections += 1;
    const pending: typeof sync = [];
    const read = (payload: string | Buffer, sent: boolean) => {
      const frameOrder = order++;
      const bytes = frameBytes(payload);
      if (bytes.length <= 1) return;
      try {
        const decoder = decoding.createDecoder(bytes);
        const routing = decoding.readVarString(decoder);
        const type = MessageType[decoding.readVarUint(decoder)];
        if (sent && type === "Sync") {
          const step = decoding.readVarUint(decoder);
          if (step !== messageYjsSyncStep2 && step !== messageYjsUpdate) return;
          if (routing !== routingKey) {
            invalid = true;
            return;
          }
          const update = decoding.readVarUint8Array(decoder);
          if (decoding.hasContent(decoder) || update.length === 0) {
            invalid = true;
            return;
          }
          const record = {
            sha: hash(update),
            len: update.length,
            empty: update.length === 2 && update[0] === 0 && update[1] === 0,
            applied: false,
            fullStateUnchanged: restoreStateDigest(before, [update]) === restoreStateDigest(before),
          };
          sync.push(record);
          syncOrder.push(frameOrder);
          pending.push(record);
        } else if (!sent && type === "SyncStatus") {
          if (routing !== routingKey) {
            invalid = true;
            return;
          }
          const applied = decoding.readVarInt(decoder);
          const write = pending.shift();
          if (!write || applied !== 1 || decoding.hasContent(decoder)) {
            invalid = true;
            return;
          }
          write.applied = true;
        } else if (type === "Stateless" && routing === routingKey) {
          const payload = decoding.readVarString(decoder);
          const parts = persistParts(payload);
          if (parts && !decoding.hasContent(decoder)) {
            (sent ? wire.sent : wire.received).push({ kind: "stateless", routingKey, payload });
            if (sent && parts.kind === "request") persistOrder.set(parts.id, frameOrder);
          }
        }
      } catch {
        invalid = true;
      }
    };
    ws.on("framesent", (frame) => {
      read(frame.payload, true);
    });
    ws.on("framereceived", (frame) => {
      read(frame.payload, false);
    });
  });
  return {
    sync,
    wire,
    facts: () => ({ invalid, connections }),
    valid: (requestId: string) =>
      !invalid &&
      connections === 1 &&
      sync.length > 0 &&
      sync.every((item) => item.applied) &&
      syncOrder.every((sent) => sent < (persistOrder.get(requestId) ?? -1)),
  };
}

async function freshStage(
  page: Page,
  state: Checkpoint,
  beforeArchive: NativeArchive,
  info: TestInfo,
  destination = false,
): Promise<void> {
  const workspace = destination ? state.destination?.workspace : state.source.workspace;
  const actor = destination ? state.destination?.actor : state.source.actor;
  if (!workspace || !actor) throw new Error("RESTORE_STAGE_IDENTITY_MISSING");
  const before = await archiveRow(beforeArchive, state.source.document.id);
  const capture = freshSyncWitness(
    page,
    `${workspace.id}:document:${state.source.document.id}`,
    before,
  );
  try {
    const editor = await openEditor(page, documentUrl(state, workspace.slug));
    await expect(editor).toContainText(LITERALS.before);
    await expect(editor).not.toContainText(LITERALS.after);
    await persistBody(page);
    const requestId = await expectMatchingPersistAck(page, capture.wire);
    expect(capture.valid(requestId), "RESTORE_STAGE_SYNC_UNCONFIRMED").toBe(true);
    const base = documentBase(state, workspace.id);
    const preview = await jsonObject(
      page.request.get(`${base}/revisions/${state.source.sourceRevision}/restore-preview`),
    );
    const finalTail = z.string().parse(preview.currentTailSeq);
    // Closing the editor before capture prevents a later client write from
    // being folded into this independently observed durable barrier.
    await page.close();
    expect(capture.valid(requestId), "RESTORE_STAGE_SYNC_UNCONFIRMED").toBe(true);
    const afterArchive = await archive(page.request, workspace.id, state.source.project.id);
    const after = await archiveRow(afterArchive, state.source.document.id);
    const priorState = rows(beforeArchive.graph, "states").find(
      (row) => row.target_id === state.source.document.id,
    );
    const finalState = rows(afterArchive.graph, "states").find(
      (row) => row.target_id === state.source.document.id,
    );
    if (!priorState || !finalState) throw new Error("RESTORE_STAGE_STATE_MISSING");
    expect(finalState.created_at).toBe(priorState.created_at);
    expect(finalState.state_entry).toBe(priorState.state_entry);
    for (const receipt of rows(priorState, "receipts"))
      expect(rows(finalState, "receipts")).toContainEqual(receipt);
    const witness = {
      document: state.source.document.id,
      actor,
      beforeTail: String(before.tail),
      finalTail,
      fullStateSha256: restoreStateDigest(before),
      ack: { requestId, matched: true },
      sync: capture.sync,
    };
    verifyRestoreStage(before, after, witness, state.source.document.id, actor);
    // The acknowledged sync response may compact this one native state. All
    // other content, IDs, precision, revisions and archive members stay exact.
    const graph = structuredClone(beforeArchive.graph);
    const states = rows(graph, "states");
    const target = states.findIndex(
      (row) => row.target_id === state.source.document.id && row.target_kind === "document",
    );
    if (target < 0) throw new Error("RESTORE_STAGE_STATE_MISSING");
    states[target] = finalState;
    graph.states = states;
    graph.captured_at = afterArchive.graph.captured_at;
    const inventory = rows(afterArchive.graph, "native_inventory");
    graph.native_inventory = rows(graph, "native_inventory").map((row, index) => {
      const current = inventory[index];
      if (!current) throw new Error("RESTORE_STAGE_INVENTORY_MISSING");
      if (index === target) {
        expect(current).toEqual({ ...row, binding: current.binding, work: current.work });
        return current;
      }
      return { ...row, binding: current.binding };
    });
    expect(afterArchive.graph).toEqual(graph);
    const nativePrefix = `native/document/${state.source.document.id}/`;
    const retained = (name: string) =>
      !name.startsWith(nativePrefix) && name !== "graph.json" && name !== "manifest.json";
    expect(Object.keys(afterArchive.zip.files).filter(retained).sort()).toEqual(
      Object.keys(beforeArchive.zip.files).filter(retained).sort(),
    );
    for (const name of Object.keys(beforeArchive.zip.files).filter(retained)) {
      const old = beforeArchive.zip.file(name),
        current = afterArchive.zip.file(name);
      if (!old || !current) throw new Error("RESTORE_ARCHIVE_MEMBER_MISSING");
      expect(await current.async("nodebuffer")).toEqual(await old.async("nodebuffer"));
    }
    await info.attach("restore-stage.private.json", {
      body: JSON.stringify({ before, witness }),
      contentType: "application/json",
    });
    writeFileSync(stagePath(), JSON.stringify({ base64: afterArchive.bytes.toString("base64") }), {
      mode: 0o600,
    });
    chmodSync(stagePath(), 0o600);
  } finally {
    await info.attach("restore-sync.private.json", {
      body: JSON.stringify({
        document: state.source.document.id,
        ...capture.facts(),
        sync: capture.sync,
        persist: capture.wire,
      }),
      contentType: "application/json",
    });
  }
}

async function exactArchive(
  actual: NativeArchive,
  expected: NativeArchive,
  destination?: { actor: string; workspace: string },
) {
  expect(
    Date.parse(z.iso.datetime({ offset: true }).parse(actual.graph.captured_at)),
  ).toBeGreaterThanOrEqual(
    Date.parse(z.iso.datetime({ offset: true }).parse(expected.graph.captured_at)),
  );
  let graph: Record<string, unknown>;
  if (destination)
    graph = remappedGraph(expected.graph, actual.graph, destination.actor, destination.workspace);
  else {
    graph = structuredClone(expected.graph);
    graph.captured_at = actual.graph.captured_at;
    const inventory = rows(actual.graph, "native_inventory");
    expect(inventory).toHaveLength(rows(graph, "native_inventory").length);
    graph.native_inventory = rows(graph, "native_inventory").map((row, index) => ({
      ...row,
      binding: z
        .string()
        .regex(/^[a-f0-9]{64}$/)
        .parse(inventory[index]?.binding),
    }));
  }
  expect(actual.graph).toEqual(graph);
  expect(actual.manifest).toEqual({
    ...expected.manifest,
    captured_at: actual.graph.captured_at,
    entries: { ...expected.manifest.entries, "graph.json": actual.manifest.entries["graph.json"] },
  });
  expect(new Set(Object.keys(actual.zip.files))).toEqual(new Set(Object.keys(expected.zip.files)));
  for (const [name, entry] of Object.entries(expected.zip.files)) {
    if (name === "graph.json" || name === "manifest.json") continue;
    const current = actual.zip.file(name);
    if (!current) throw new Error("RESTORE_ARCHIVE_MEMBER_MISSING");
    expect(await current.async("nodebuffer")).toEqual(await entry.async("nodebuffer"));
  }
}

test("Turso restore U6: UI confirm has one forward revision and matching save ACK", async ({
  browser,
  baseURL,
}, info) => {
  if (!baseURL) throw new Error("Allocated source server required");
  expect(process.env.FVOCI_E2E_SELECTED_BACKEND).toBe("libsql-remote");
  const context = await newCollabContext(browser, baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    const wire = attachCollabWire(page);
    if (process.env.FVOCI_E2E_RESTORE_SETUP === "1") await ensureInstanceSetup(page);
    else await login(page, admin.email, admin.password);
    const actor = await json(page.request.get("/api/v1/auth/me"), identitySchema);
    const spaces = await json(
      page.request.get("/api/v1/me/workspaces"),
      z.object({ items: z.array(workspaceSchema) }),
    );
    const team = spaces.items.find((item) => item.slug === admin.workspaceSlug);
    if (!team) throw new Error("Allocated source team workspace required");
    const workspace = await json(
      page.request.post("/api/v1/me/personal-workspace"),
      workspaceSchema,
    );
    const project = await json(
      page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
        data: { key: "RST", name: "별도 설치 복구 🧪", visibility: "private" },
      }),
      projectSchema,
      201,
    );
    const document = await json(
      page.request.post(`/api/v1/workspaces/${workspace.id}/projects/${project.id}/documents`, {
        data: { title: "복원 이력 문서", parentId: project.rootDocumentId },
      }),
      metaSchema,
      201,
    );
    const base = `/api/v1/workspaces/${workspace.id}/projects/${project.id}/documents/${document.id}`;
    const url = `/w/${workspace.slug}/${project.key}-${String(document.number)}`;
    const editor = await openEditor(page, url);
    await editor.fill(LITERALS.before);
    await persistBody(page);
    const firstAck = await expectMatchingPersistAck(page, wire);
    const source = await json(
      page.request.post(base + "/revisions"),
      z.object({ id: z.guid() }),
      201,
    );
    const sourceDetail = await jsonObject(page.request.get(`${base}/revisions/${source.id}`));
    await clearEditor(page);
    await editor.fill(LITERALS.after);
    await persistBody(page);
    const finalAck = await expectMatchingPersistAck(page, wire);
    expect(firstAck).not.toBe(finalAck);
    await page.getByTestId("revision-history").click();
    const history = (await jsonObject(page.request.get(base + "/revisions"))).items;
    const index = z
      .array(z.object({ id: z.guid() }))
      .parse(history)
      .findIndex((item) => item.id === source.id);
    expect(index).toBeGreaterThanOrEqual(0);
    const previewResponse = page.waitForResponse((response) =>
      response.url().endsWith(`/revisions/${source.id}/restore-preview`),
    );
    await page.getByTestId("revision-item").nth(index).getByTestId("revision-restore").click();
    const previewHttp = await previewResponse;
    expect(previewHttp.status()).toBe(200);
    const preview = (await previewHttp.json()) as Schema["RevisionRestorePreviewResponse"];
    await expect(page.getByTestId("revision-restore-source")).toContainText(LITERALS.before);
    await expect(page.getByTestId("revision-restore-current")).toContainText(LITERALS.after);
    const response = page.waitForResponse(
      (r) => r.request().method() === "POST" && r.url().endsWith(`/revisions/${source.id}/restore`),
    );
    await page.getByTestId("revision-restore-confirm").click();
    const confirmed = await response;
    expect(confirmed.status()).toBe(200);
    const intent = confirmed.request().postDataJSON() as Schema["RevisionRestoreBody"];
    const restored = (await confirmed.json()) as Schema["RevisionRestoreResponse"];
    expect(intent.expectedTailSeq).toBe(preview.currentTailSeq);
    expect(restored.restored).toBe(true);
    expect(restored.revisionId).not.toBe(source.id);
    const committed = await jsonObject(
      page.request.get(`${base}/revisions/${source.id}/restore-preview`),
    );
    const tail = z.string().parse(committed.currentTailSeq);
    expect(BigInt(tail)).toBe(BigInt(intent.expectedTailSeq) + 1n);
    expect(committed.source).toEqual(sourceDetail);
    expect(
      await jsonObject(page.request.get(`${base}/revisions/${restored.revisionId}`)),
    ).toMatchObject({
      id: restored.revisionId,
      reason: "restore",
      restoredFromId: source.id,
      contentJson: sourceDetail.contentJson,
    });
    const replay = await page.request.post(`${base}/revisions/${source.id}/restore`, {
      data: intent,
    });
    expect(replay.status()).toBe(200);
    expect(await replay.json()).toEqual(restored);
    const finalHistory = await jsonObject(page.request.get(base + "/revisions"));
    expect(
      z
        .array(z.object({ reason: z.string() }))
        .parse(finalHistory.items)
        .filter((item) => item.reason === "restore"),
    ).toHaveLength(1);
    const state: Checkpoint = {
      source: {
        actor: actor.userId,
        team,
        workspace,
        project,
        document,
        sourceRevision: source.id,
        restoredRevision: restored.revisionId,
        restoredBody: sourceDetail.contentJson,
        restoredTail: tail,
        history: finalHistory,
        firstAck,
        finalAck,
        correlation: intent.correlationId,
        expectedTail: intent.expectedTailSeq,
      },
    };
    await page.close();
    const restorePoint = await archive(context.request, workspace.id, project.id);
    expect(
      BigInt(
        z
          .number()
          .int()
          .parse((await archiveRow(restorePoint, document.id)).tail),
      ),
    ).toBe(BigInt(tail));
    const fresh = await newCollabContext(browser, baseURL);
    let freshFailed = true;
    try {
      const client = await fresh.newPage();
      await login(client, admin.email, admin.password);
      expect((await jsonObject(client.request.get(base + "/body"))).contentJson).toEqual(
        sourceDetail.contentJson,
      );
      await freshStage(client, state, restorePoint, info);
      freshFailed = false;
    } finally {
      await closeCollabContext(fresh, freshFailed);
    }
    writeCheckpoint(state);
    await info.attach("restore-u6.private.json", {
      body: JSON.stringify(state.source),
      contentType: "application/json",
    });
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
});

test("Turso restore source restart: fresh client retains U6 body history and tail", async ({
  browser,
  baseURL,
}, info) => {
  if (!baseURL) throw new Error("Allocated restarted source server required");
  const state = readCheckpoint();
  const context = await newCollabContext(browser, baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    await login(page, admin.email, admin.password);
    const before = await archive(page.request, state.source.workspace.id, state.source.project.id);
    const previous = await previousStage();
    if (!previous) throw new Error("RESTORE_STAGE_INPUT_MISSING");
    await exactArchive(before, previous);
    const base = documentBase(state);
    expect((await jsonObject(page.request.get(base + "/body"))).contentJson).toEqual(
      state.source.restoredBody,
    );
    const preview = await jsonObject(
      page.request.get(`${base}/revisions/${state.source.sourceRevision}/restore-preview`),
    );
    expect(preview.currentTailSeq).toBe(
      String((await archiveRow(before, state.source.document.id)).tail),
    );
    const history = await jsonObject(page.request.get(base + "/revisions"));
    const items = z.array(z.record(z.string(), z.unknown())).parse(history.items);
    for (const old of z
      .array(z.record(z.string(), z.unknown()))
      .parse(z.record(z.string(), z.unknown()).parse(state.source.history).items))
      expect(items).toContainEqual(old);
    expect(items.filter((item) => item.reason === "restore")).toHaveLength(1);
    await freshStage(page, state, before, info);
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
});

test("Turso restore export: current native archive carries IDs references attachments and precision", async ({
  browser,
  baseURL,
}, info) => {
  if (!baseURL) throw new Error("Allocated source server required");
  const state = readCheckpoint();
  const context = await newCollabContext(browser, baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    const wire = attachCollabWire(page);
    await login(page, admin.email, admin.password);
    const { workspace, project, document } = state.source;
    const workspaceBase = `/api/v1/workspaces/${workspace.id}`;
    const base = documentBase(state);
    const taskCreated = await json(
      page.request.post(`${workspaceBase}/documents/${document.id}/tasks`, {
        data: {
          projectId: project.id,
          requestId: randomUUID(),
          anchor: null,
          selfAssign: true,
          task: {
            title: "원본에서 추출한 태스크",
            type: "task",
            priority: "medium",
            startDate: LITERALS.date,
          },
        },
      }),
      z.object({ taskId: z.guid() }),
      201,
    );
    const taskPath = `${workspaceBase}/tasks/${taskCreated.taskId}`;
    expect(
      (await page.request.patch(taskPath, { data: { dueAt: LITERALS.instant } })).status(),
    ).toBe(200);
    const file = await upload(page.request, base, workspace.id);
    const editor = await openEditor(page, documentUrl(state));
    await editor.evaluate(
      (element, input) => {
        const mounted = (element as HTMLElement & { editor?: Editor }).editor;
        if (!mounted) throw new Error("Actual mounted editor required");
        mounted.commands.insertContent([
          {
            type: "paragraph",
            content: [
              {
                type: "mention",
                attrs: { entity: "task", id: input.task, label: "원본에서 추출한 태스크" },
              },
            ],
          },
          { type: "attachment", attrs: { id: input.file, name: "복구-자료.txt", image: false } },
        ]);
      },
      { task: taskCreated.taskId, file },
    );
    await persistBody(page);
    await expectMatchingPersistAck(page, wire);
    expect((await page.request.post(base + "/revisions")).status()).toBe(201);
    const collection = await json(
      page.request.get(`${workspaceBase}/projects/${project.id}/collection`),
      z.object({ id: z.guid() }),
    );
    for (const [key, type, value] of [
      ["exact_number", "number", { number: LITERALS.number }],
      ["exact_date", "date", { date: LITERALS.date }],
      ["exact_time", "datetime", { datetime: LITERALS.instant }],
    ] as const) {
      const field = await json(
        page.request.post(`${workspaceBase}/collections/${collection.id}/fields`, {
          data: { name: key, key, type },
        }),
        z.object({ id: z.guid(), version: z.number() }),
        201,
      );
      const before = await collectionValues(page.request, workspace.id, collection.id);
      expect(before.items).toHaveLength(1);
      const row = before.items[0];
      if (!row) throw new Error("Actual collection item required");
      const updated = await page.request.put(
        `${workspaceBase}/collections/${collection.id}/items/${row.id}/values`,
        {
          data: {
            fieldId: field.id,
            expectedVersion: row.version,
            expectedFieldVersion: field.version,
            value,
          },
        },
      );
      expect(updated.status()).toBe(200);
      const after = await collectionValues(page.request, workspace.id, collection.id);
      expect(after.items[0]?.values[field.id]).toEqual(
        type === "datetime" ? { datetime: LITERALS.instantApi } : value,
      );
    }
    const body = await jsonObject(page.request.get(base + "/body"));
    const task = await jsonObject(page.request.get(taskPath));
    const origin = await jsonObject(page.request.get(taskPath + "/origin"));
    const values = await collectionValues(page.request, workspace.id, collection.id);
    const expectedValues = values.items[0]?.values;
    if (!expectedValues) throw new Error("Actual source collection values required");
    state.source.history = await jsonObject(page.request.get(base + "/revisions"));
    const exported = await archive(page.request, workspace.id, project.id);
    expect(exported.graph.source_actor_id).toBe(state.source.actor);
    expect(exported.graph.source_workspace_id).toBe(workspace.id);
    const scalars = rows(exported.graph, "collection_values");
    for (const [type, column, literal] of [
      ["number", "value_number", String(LITERALS.number)],
      ["date", "value_date", LITERALS.date],
      ["datetime", "value_ts", LITERALS.instantArchive],
    ]) {
      const selected = scalars.filter((row) => row.field_type === type);
      expect(selected).toHaveLength(1);
      if (!column) throw new Error("Fixed native scalar column required");
      expect(selected[0]?.[column]).toBe(literal);
    }
    expect(rows(exported.graph, "documents").map((item) => item.id)).toContain(document.id);
    expect(rows(exported.graph, "tasks").map((item) => item.id)).toEqual([taskCreated.taskId]);
    expect(rows(exported.graph, "attachments").map((item) => item.id)).toEqual([file]);
    expect(rows(exported.graph, "origins")).toHaveLength(1);
    expect(rows(exported.graph, "origins")[0]).toMatchObject({
      document_id: document.id,
      task_id: taskCreated.taskId,
    });
    const restored = rows(exported.graph, "revisions").filter((item) => item.reason === "restore");
    expect(restored).toHaveLength(1);
    expect(restored[0]).toMatchObject({
      id: state.source.restoredRevision,
      restored_from_id: state.source.sourceRevision,
      restore_correlation_id: state.source.correlation,
    });
    expect(BigInt(z.number().int().parse(restored[0]?.restore_base_tail_seq))).toBe(
      BigInt(state.source.expectedTail),
    );
    expect(BigInt(z.number().int().parse(restored[0]?.restore_committed_tail_seq))).toBe(
      BigInt(state.source.restoredTail),
    );
    const payload = exported.zip.file(`attachments/${file}/payload`);
    if (!payload) throw new Error("Actual attachment payload required");
    expect(await payload.async("nodebuffer")).toEqual(Buffer.from(LITERALS.attachment));
    state.archive = {
      base64: exported.bytes.toString("base64"),
      sha256: hash(exported.bytes),
      graph: exported.graph,
      body,
      task,
      origin,
      attachment: file,
      collection: collection.id,
      values: expectedValues,
    };
    writeCheckpoint(state);
    await info.attach("restore-export.private.json", {
      body: JSON.stringify({
        source: state.source,
        archiveHash: state.archive.sha256,
        documentCount: rows(exported.graph, "documents").length,
        revisionCount: rows(exported.graph, "revisions").length,
      }),
      contentType: "application/json",
    });
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
});

test("Turso restore destination: malformed input publishes nothing and identical replay has one job", async ({
  browser,
  baseURL,
}, info) => {
  if (!baseURL) throw new Error("Separate allocated SQLite installation required");
  expect(process.env.FVOCI_E2E_SELECTED_BACKEND).toBe("sqlite");
  const state = readCheckpoint();
  const exported = state.archive;
  if (!exported) throw new Error("Authenticated source archive required");
  const context = await newCollabContext(browser, baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    await ensureInstanceSetup(page);
    const actor = await json(page.request.get("/api/v1/auth/me"), identitySchema);
    expect(actor.userId).not.toBe(state.source.actor);
    const workspace = await json(
      page.request.post("/api/v1/me/personal-workspace"),
      workspaceSchema,
    );
    expect(workspace.id).not.toBe(state.source.workspace.id);
    const base = `/api/v1/workspaces/${workspace.id}`;
    // Invalid-input checks belong to a second genuine personal owner. Keep
    // the restore owner's five native-archive calls for preflight, restore,
    // replay, the pre-client archive and the post-ACK stage archive.
    const rejectionContext = await newCollabContext(browser, baseURL);
    let rejectionFailed = true;
    let beforeCounts: ReturnType<typeof observeRestoreCounts>;
    try {
      const rejectionPage = await rejectionContext.newPage();
      const spaces = await json(
        page.request.get("/api/v1/me/workspaces"),
        z.object({
          items: z.array(workspaceSchema.extend({ role: z.string(), kind: z.string() })),
        }),
      );
      const team = spaces.items.find((item) => item.slug === admin.workspaceSlug);
      expect(team?.role).toBe("owner");
      expect(team?.kind).toBe("team");
      if (!team) throw new Error("RESTORE_REJECTION_TEAM_MISSING");
      let rejectionActor: z.infer<typeof identitySchema>;
      let rejectionWorkspace: z.infer<typeof workspaceSchema>;
      try {
        // Token-bearing responses and request failures must never enter the
        // reporter; use the normal invitation session without a DB fixture.
        const email = `restore-rejection-${randomUUID()}@example.invalid`;
        const invitation = await json(
          page.request.post(`/api/v1/workspaces/${team.id}/invitations`, {
            data: { email, role: "member" },
          }),
          z.object({ acceptUrl: z.string() }),
          201,
        );
        const token = z
          .string()
          .min(1)
          .parse(new URL(invitation.acceptUrl).pathname.split("/invite/")[1]);
        const invitationPath = `/api/v1/invitations/${encodeURIComponent(token)}`;
        const preview = await json(
          rejectionPage.request.get(invitationPath),
          z.object({
            requiredLegal: z.array(z.object({ kind: z.string(), version: z.number().int() })),
          }),
        );
        expect(
          (
            await rejectionPage.request.post(invitationPath + "/accept", {
              data: {
                email,
                givenName: "복구거부검증",
                password: randomUUID() + "Aa1!",
                consents: preview.requiredLegal,
              },
            })
          ).status(),
        ).toBe(200);
        const current = await jsonObject(rejectionPage.request.get("/api/v1/auth/me"));
        expect(current.isInstanceAdmin).toBe(false);
        rejectionActor = identitySchema.parse(current);
        rejectionWorkspace = await json(
          rejectionPage.request.post("/api/v1/me/personal-workspace"),
          workspaceSchema,
        );
      } catch {
        throw new Error("RESTORE_REJECTION_OWNER_SETUP_FAILED");
      }
      expect(rejectionActor.userId).not.toBe(actor.userId);
      expect(rejectionActor.sessionId).not.toBe(actor.sessionId);
      expect(rejectionWorkspace.id).not.toBe(workspace.id);
      const owned = await json(
        rejectionPage.request.get("/api/v1/me/workspaces"),
        z.object({
          items: z.array(workspaceSchema.extend({ role: z.string(), kind: z.string() })),
        }),
      );
      const personal = owned.items.find((item) => item.id === rejectionWorkspace.id);
      expect(personal).toMatchObject({ role: "owner", kind: "personal" });
      const rejectionBase = `/api/v1/workspaces/${rejectionWorkspace.id}`;
      // All account/workspace setup precedes the global no-publication
      // baseline. Both owners' targets must stay unchanged on rejection.
      const before = await jsonObject(page.request.get(base + "/projects"));
      const beforeDocuments = await jsonObject(page.request.get(base + "/tree"));
      const rejectedProjects = await jsonObject(
        rejectionPage.request.get(rejectionBase + "/projects"),
      );
      const rejectedDocuments = await jsonObject(
        rejectionPage.request.get(rejectionBase + "/tree"),
      );
      beforeCounts = observeRestoreCounts();
      expect(beforeCounts.counts.nativeJobs).toBe(0);
      const invalid = Buffer.from("not a native archive");
      const rebound = await reboundArchive(Buffer.from(exported.base64, "base64"));
      const malformedPreflight = await rejectionPage.request.post(
        rejectionBase + "/native-archive/preflight",
        {
          data: { archiveBase64: rebound.toString("base64") },
        },
      );
      expect(malformedPreflight.status()).toBe(422);
      expect(await malformedPreflight.json()).toMatchObject({
        params: { code: "native_archive_invalid" },
      });
      const malformedRestore = await rejectionPage.request.post(
        rejectionBase + "/native-archive/restore",
        {
          data: {
            archiveBase64: invalid.toString("base64"),
            archiveHash: hash(invalid),
            requestId: randomUUID(),
            destinationActorId: rejectionActor.userId,
            confirm: true,
          },
        },
      );
      expect(malformedRestore.status()).toBe(422);
      expect(await jsonObject(page.request.get(base + "/projects"))).toEqual(before);
      expect(await jsonObject(page.request.get(base + "/tree"))).toEqual(beforeDocuments);
      expect(await jsonObject(rejectionPage.request.get(rejectionBase + "/projects"))).toEqual(
        rejectedProjects,
      );
      expect(await jsonObject(rejectionPage.request.get(rejectionBase + "/tree"))).toEqual(
        rejectedDocuments,
      );
      const after = observeRestoreCounts();
      expect(after).toEqual(beforeCounts);
      await info.attach("restore-rejection-owner.private.json", {
        body: JSON.stringify({
          restoreActor: actor.userId,
          restoreWorkspace: workspace.id,
          rejectionActor: rejectionActor.userId,
          rejectionWorkspace: rejectionWorkspace.id,
          scope: personal,
          statuses: [malformedPreflight.status(), malformedRestore.status()],
          before: beforeCounts,
          after,
        }),
        contentType: "application/json",
      });
      rejectionFailed = false;
    } finally {
      await closeCollabContext(rejectionContext, rejectionFailed);
    }
    const preflight = await jsonObject(
      page.request.post(base + "/native-archive/preflight", {
        data: { archiveBase64: exported.base64 },
      }),
    );
    expect(preflight).toMatchObject({
      complete: true,
      preservedContentIds: true,
      requiresCollisionFreeInstallation: true,
      archiveHash: exported.sha256,
      sourceWorkspaceId: state.source.workspace.id,
      destinationWorkspaceId: workspace.id,
      destinationActorId: actor.userId,
      projectId: state.source.project.id,
      documentCount: rows(exported.graph, "documents").length,
      taskCount: 1,
      attachmentCount: 1,
      revisionCount: rows(exported.graph, "revisions").length,
    });
    const intent = {
      archiveBase64: exported.base64,
      archiveHash: exported.sha256,
      requestId: randomUUID(),
      destinationActorId: actor.userId,
      confirm: true,
    };
    const queued = await json(
      page.request.post(base + "/native-archive/restore", { data: intent }),
      jobSchema,
      202,
    );
    await expect
      .poll(
        async () =>
          (await json(page.request.get(base + "/native-archive/jobs/" + queued.id), jobSchema))
            .status,
      )
      .toBe("completed");
    const publishedCounts = observeRestoreCounts();
    expect(publishedCounts.counts).toMatchObject({
      projects: beforeCounts.counts.projects + 1,
      documents: beforeCounts.counts.documents + rows(exported.graph, "documents").length,
      tasks: beforeCounts.counts.tasks + rows(exported.graph, "tasks").length,
      attachments: beforeCounts.counts.attachments + rows(exported.graph, "attachments").length,
      revisions: beforeCounts.counts.revisions + rows(exported.graph, "revisions").length,
      jobs: beforeCounts.counts.jobs + 1,
      nativeJobs: 1,
    });
    const replay = await json(
      page.request.post(base + "/native-archive/restore", { data: intent }),
      jobSchema,
      202,
    );
    expect(replay.id).toBe(queued.id);
    const afterCounts = observeRestoreCounts();
    expect(afterCounts.counts).toEqual(publishedCounts.counts);
    expect(afterCounts.counts.nativeJobs).toBe(1);
    expect(afterCounts.counts.jobs).toBe(beforeCounts.counts.jobs + 1);
    await info.attach("restore-destination-counts.private.json", {
      body: JSON.stringify({
        before: beforeCounts,
        published: publishedCounts,
        after: afterCounts,
        request: intent.requestId,
        job: queued.id,
      }),
      contentType: "application/json",
    });
    state.destination = {
      actor: actor.userId,
      workspace,
      job: { ...queued, status: "completed", projectId: state.source.project.id },
    };
    writeCheckpoint(state);
    // Import is checked byte for byte before any fresh client can append its
    // accepted sync response. Later stages retain their own durable witness.
    const imported = await archive(page.request, workspace.id, state.source.project.id);
    await exactArchive(imported, await decodeArchive(Buffer.from(exported.base64, "base64")), {
      actor: actor.userId,
      workspace: workspace.id,
    });
    const fresh = await newCollabContext(browser, baseURL);
    let freshFailed = true;
    try {
      const client = await fresh.newPage();
      await login(client, admin.email, admin.password);
      await restoredReadback(client, state);
      await freshStage(client, state, imported, info, true);
      freshFailed = false;
    } finally {
      await closeCollabContext(fresh, freshFailed);
    }
    const anonymous = await browser.newContext({ baseURL });
    try {
      expect(
        (await anonymous.request.get(documentBase(state, workspace.id) + "/body")).status(),
      ).toBe(401);
      expect(
        (
          await anonymous.request.get(`${base}/attachments/${exported.attachment}/download`)
        ).status(),
      ).toBe(401);
    } finally {
      await anonymous.close();
    }
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
});

test("Turso restore destination restart: fresh client reads exact archive data and permissions", async ({
  browser,
  baseURL,
}, info) => {
  if (!baseURL) throw new Error("Allocated restarted destination required");
  const state = readCheckpoint();
  if (!state.destination) throw new Error("Actual destination required");
  const context = await newCollabContext(browser, baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    await login(page, admin.email, admin.password);
    const actor = await json(page.request.get("/api/v1/auth/me"), identitySchema);
    expect(actor.userId).toBe(state.destination.actor);
    const before = await archive(
      page.request,
      state.destination.workspace.id,
      state.source.project.id,
    );
    const previous = await previousStage();
    if (!previous) throw new Error("RESTORE_STAGE_INPUT_MISSING");
    await exactArchive(before, previous);
    await restoredReadback(page, state);
    await freshStage(page, state, before, info, true);
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
});

test("Turso restore archive comparison: re-export preserves native bytes with explicit actor workspace remap", async ({
  browser,
  baseURL,
}) => {
  if (!baseURL) throw new Error("Allocated destination required");
  const state = readCheckpoint();
  if (!state.destination || !state.archive)
    throw new Error("Actual source and destination archive required");
  const context = await newCollabContext(browser, baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    await login(page, admin.email, admin.password);
    const actual = await archive(
      page.request,
      state.destination.workspace.id,
      state.source.project.id,
    );
    const previous = await previousStage();
    if (!previous) throw new Error("RESTORE_STAGE_INPUT_MISSING");
    await exactArchive(actual, previous);
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
});
