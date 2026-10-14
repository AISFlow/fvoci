import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";
import { z } from "zod";
import type { Editor } from "@tiptap/core";
import type { components } from "../src/generated/api";
import { observeRestoreCounts } from "../../../tools/turso/ui-restore-observe";
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
    const fresh = await newCollabContext(browser, baseURL);
    let freshFailed = true;
    try {
      const client = await fresh.newPage();
      await login(client, admin.email, admin.password);
      await expect(await openEditor(client, url)).toContainText(LITERALS.before);
      expect((await jsonObject(client.request.get(base + "/body"))).contentJson).toEqual(
        sourceDetail.contentJson,
      );
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
}) => {
  if (!baseURL) throw new Error("Allocated restarted source server required");
  const state = readCheckpoint();
  const context = await newCollabContext(browser, baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    await login(page, admin.email, admin.password);
    const editor = await openEditor(page, documentUrl(state));
    await expect(editor).toContainText(LITERALS.before);
    await expect(editor).not.toContainText(LITERALS.after);
    const base = documentBase(state);
    expect((await jsonObject(page.request.get(base + "/body"))).contentJson).toEqual(
      state.source.restoredBody,
    );
    const preview = await jsonObject(
      page.request.get(`${base}/revisions/${state.source.sourceRevision}/restore-preview`),
    );
    expect(preview.currentTailSeq).toBe(state.source.restoredTail);
    const history = await jsonObject(page.request.get(base + "/revisions"));
    const items = z.array(z.record(z.string(), z.unknown())).parse(history.items);
    for (const old of z
      .array(z.record(z.string(), z.unknown()))
      .parse(z.record(z.string(), z.unknown()).parse(state.source.history).items))
      expect(items).toContainEqual(old);
    expect(items.filter((item) => item.reason === "restore")).toHaveLength(1);
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
    const before = await jsonObject(page.request.get(base + "/projects"));
    const beforeCounts = observeRestoreCounts();
    expect(beforeCounts.counts.nativeJobs).toBe(0);
    const beforeDocuments = await jsonObject(page.request.get(base + "/documents"));
    const invalid = Buffer.from("not a native archive");
    const rebound = await reboundArchive(Buffer.from(exported.base64, "base64"));
    const malformedPreflight = await page.request.post(base + "/native-archive/preflight", {
      data: { archiveBase64: rebound.toString("base64") },
    });
    expect(malformedPreflight.status()).toBe(422);
    expect(await malformedPreflight.json()).toMatchObject({
      params: { code: "native_archive_invalid" },
    });
    const malformedRestore = await page.request.post(base + "/native-archive/restore", {
      data: {
        archiveBase64: invalid.toString("base64"),
        archiveHash: hash(invalid),
        requestId: randomUUID(),
        destinationActorId: actor.userId,
        confirm: true,
      },
    });
    expect(malformedRestore.status()).toBe(422);
    expect(await jsonObject(page.request.get(base + "/projects"))).toEqual(before);
    expect(await jsonObject(page.request.get(base + "/documents"))).toEqual(beforeDocuments);
    expect(observeRestoreCounts().counts).toEqual(beforeCounts.counts);
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
    const fresh = await newCollabContext(browser, baseURL);
    let freshFailed = true;
    try {
      const client = await fresh.newPage();
      await login(client, admin.email, admin.password);
      await restoredReadback(client, state);
      await expect(await openEditor(client, documentUrl(state, workspace.slug))).toContainText(
        LITERALS.before,
      );
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
}) => {
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
    await restoredReadback(page, state);
    await expect(
      await openEditor(page, documentUrl(state, state.destination.workspace.slug)),
    ).toContainText(LITERALS.before);
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
    expect(actual.graph).toEqual(
      remappedGraph(
        state.archive.graph,
        actual.graph,
        state.destination.actor,
        state.destination.workspace.id,
      ),
    );
    const sourceArchive = await decodeArchive(Buffer.from(state.archive.base64, "base64"));
    const sourceZip = sourceArchive.zip;
    expect(actual.manifest).toEqual({
      ...sourceArchive.manifest,
      captured_at: actual.graph.captured_at,
      entries: {
        ...sourceArchive.manifest.entries,
        "graph.json": actual.manifest.entries["graph.json"],
      },
    });
    expect(new Set(Object.keys(actual.zip.files))).toEqual(new Set(Object.keys(sourceZip.files)));
    for (const [name, entry] of Object.entries(sourceZip.files)) {
      if (name === "graph.json" || name === "manifest.json") continue;
      const current = actual.zip.file(name);
      if (!current) throw new Error("Native archive member missing");
      expect(await current.async("nodebuffer")).toEqual(await entry.async("nodebuffer"));
    }
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
});
