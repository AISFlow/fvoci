import { expect, test } from "bun:test";
import { ProblemError } from "@/lib/api";
import type { CommandStorage } from "./capture-command";
import {
  audienceKey,
  buildSelection,
  classifyTransferFailure,
  forgetTransfer,
  recoverTransfer,
  rememberTransfer,
  resultPaths,
  hostTransferPrepare,
  OUTCOME_KEYS,
  taskTransferDocument,
  taskTransferPrepare,
  retainedTaskProject,
  transferFailureKeys,
  unscopedSourceKeys,
  type HostDraftSnapshot,
  type PendingTransferCommand,
  type TransferBlocker,
  type TaskHostSnapshot,
  type TransferSource,
} from "./personal-transfer-command";
const ACTOR = "00000000-0000-4000-8000-000000000001",
  SESSION = "00000000-0000-4000-8000-000000000002",
  NEXT_SESSION = "00000000-0000-4000-8000-000000000003",
  SOURCE = "00000000-0000-4000-8000-000000000004",
  TEAM = "00000000-0000-4000-8000-000000000005",
  PROJECT = "00000000-0000-4000-8000-000000000006",
  STATUS = "00000000-0000-4000-8000-000000000007";
function storage(): CommandStorage & { size(): number } {
  const data = new Map<string, string>();
  return {
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => {
      data.set(k, v);
    },
    removeItem: (k) => {
      data.delete(k);
    },
    size: () => data.size,
  };
}
const source = (taskId: string | null = crypto.randomUUID()): TransferSource => ({
  workspaceId: SOURCE,
  documentId: crypto.randomUUID(),
  documentVersion: 3,
  taskId,
  taskVersion: taskId ? 2 : null,
  taskProjectId: taskId ? PROJECT : null,
});
const command = (sessionId = SESSION): PendingTransferCommand => ({
  actorId: ACTOR,
  sessionId,
  sourceWorkspaceId: SOURCE,
  sourceTaskProjectId: PROJECT,
  destinationSlug: "Team-한글",
  destinationProjectKey: "PUB",
  body: {
    requestId: crypto.randomUUID(),
    confirmed: true,
    previewDigest: "a".repeat(64),
    selection: buildSelection(source(), "move", {
      workspaceId: TEAM,
      projectId: PROJECT,
      statusId: STATUS,
    }),
  },
});

test("selection carries host versions and drops task-only fields for a note", () => {
  const pair = source();
  const selected = buildSelection(pair, "copy", {
    workspaceId: TEAM,
    projectId: PROJECT,
    statusId: STATUS,
  });
  expect(selected).toEqual({
    action: "copy",
    documentId: pair.documentId,
    expectedDocumentVersion: 3,
    taskId: pair.taskId,
    expectedTaskVersion: 2,
    destinationWorkspaceId: TEAM,
    destinationProjectId: PROJECT,
    destinationStatusId: STATUS,
  });
  const note = buildSelection(source(null), "move", {
    workspaceId: TEAM,
    projectId: PROJECT,
    statusId: STATUS,
  });
  expect(note.taskId).toBeNull();
  expect(note.expectedTaskVersion).toBeNull();
  expect(note.destinationStatusId).toBeNull();
});

test("lost success replays the identical immutable command for the same credential only", () => {
  const s = storage(),
    first = command();
  rememberTransfer(s, first);
  const retry = recoverTransfer(s, ACTOR, SESSION);
  expect(retry).toEqual(first);
  expect(retry?.body.requestId).toBe(first.body.requestId);
  expect(recoverTransfer(s, ACTOR, NEXT_SESSION)).toBeNull();
  expect(recoverTransfer(s, SESSION, SESSION)).toBeNull();
  // The same exact command may be persisted again before a retry dispatch.
  rememberTransfer(s, first);
  expect(() => {
    rememberTransfer(s, command());
  }).toThrow();
  expect(recoverTransfer(s, ACTOR, SESSION)).toEqual(first);
});

test("a retired session's orphan cannot block the new credential's command", () => {
  const s = storage();
  rememberTransfer(s, command(SESSION));
  const fresh = command(NEXT_SESSION);
  rememberTransfer(s, fresh);
  expect(recoverTransfer(s, ACTOR, NEXT_SESSION)).toEqual(fresh);
  forgetTransfer(s, fresh);
  expect(recoverTransfer(s, ACTOR, NEXT_SESSION)).toBeNull();
  expect(s.size()).toBe(1);
});

test("old acknowledgement cannot erase a newer pending command", () => {
  const s = storage(),
    old = command();
  rememberTransfer(s, old);
  forgetTransfer(s, old);
  const next = command();
  rememberTransfer(s, next);
  forgetTransfer(s, old);
  expect(recoverTransfer(s, ACTOR, SESSION)).toEqual(next);
});

test("malformed or unconfirmed storage is never replayed", () => {
  const s = storage();
  const bad = command();
  s.setItem(
    `fvoci:personal-transfer:${ACTOR}:${SESSION}`,
    JSON.stringify({ ...bad, body: { ...bad.body, confirmed: false } }),
  );
  expect(recoverTransfer(s, ACTOR, SESSION)).toBeNull();
  s.setItem(`fvoci:personal-transfer:${ACTOR}:${SESSION}`, "{");
  expect(recoverTransfer(s, ACTOR, SESSION)).toBeNull();
  expect(() => {
    rememberTransfer(s, { ...bad, body: { ...bad.body, previewDigest: "A".repeat(64) } });
  }).toThrow();
});

test("only definitive server answers are classified; transport and 5xx stay unknown", () => {
  expect(classifyTransferFailure(new TypeError("fetch failed"))).toBe("unknown");
  expect(classifyTransferFailure(new ProblemError(500))).toBe("unknown");
  expect(classifyTransferFailure(new ProblemError(503))).toBe("unknown");
  expect(classifyTransferFailure(new ProblemError(409, "personal_transfer_incomplete"))).toBe(
    "incomplete",
  );
  expect(classifyTransferFailure(new ProblemError(409, "personal_transfer_conflict"))).toBe(
    "conflict",
  );
  expect(classifyTransferFailure(new ProblemError(404, "not_found"))).toBe("unavailable");
  expect(classifyTransferFailure(new ProblemError(403))).toBe("unavailable");
});

test("audience text never widens an unknown visibility", () => {
  expect(audienceKey("workspace")).toBe("personalTransfer.audienceWorkspace");
  expect(audienceKey("private")).toBe("personalTransfer.audiencePrivate");
  expect(audienceKey("")).toBe("personalTransfer.audiencePrivate");
});

test("result routes use the committed destination numbers", () => {
  const routes = resultPaths(
    { destinationSlug: "Team-한글", destinationProjectKey: "PUB" },
    {
      workspaceId: TEAM,
      projectId: PROJECT,
      documentId: crypto.randomUUID(),
      documentNumber: 7,
      taskId: crypto.randomUUID(),
      taskNumber: 8,
      replayed: false,
    },
  );
  expect(routes).toEqual({ document: "/w/team-한글/PUB-7", task: "/w/team-한글/PUB-8" });
  expect(
    resultPaths(
      { destinationSlug: "team", destinationProjectKey: "PUB" },
      {
        workspaceId: TEAM,
        projectId: PROJECT,
        documentId: crypto.randomUUID(),
        documentNumber: 9,
        taskId: null,
        taskNumber: null,
        replayed: true,
      },
    ).task,
  ).toBeNull();
});

test("a command stored before sourceTaskProjectId existed stays recoverable unchanged", () => {
  const s = storage();
  const legacy = command();
  delete legacy.sourceTaskProjectId;
  const raw = JSON.stringify(legacy);
  s.setItem(`fvoci:personal-transfer:${ACTOR}:${SESSION}`, raw);
  const recovered = recoverTransfer(s, ACTOR, SESSION);
  expect(recovered).toEqual(legacy);
  expect(recovered?.body.requestId).toBe(legacy.body.requestId);
  expect(recovered?.sourceTaskProjectId).toBeUndefined();
  // Re-persisting before an identical replay neither throws nor rewrites it.
  if (!recovered) throw new Error("legacy command lost");
  rememberTransfer(s, recovered);
  expect(s.getItem(`fvoci:personal-transfer:${ACTOR}:${SESSION}`)).toBe(raw);
});

const typed = (code: string, reason: string, title = "diagnostic title, never shown") => {
  const error = new ProblemError(409, code, undefined, reason);
  Object.defineProperty(error, "title", { value: title });
  return error;
};

test("every typed blocker maps to its own reason before the generic refusal", () => {
  const blockers: TransferBlocker[] = [
    "native_history",
    "outgoing_reference",
    "incoming_reference",
    "file",
    "hierarchy",
    "assignee",
    "dependent_graph",
    "wip_reservation",
    "inventory_budget",
    "block_identity",
    "body_encoding",
    "native_state_missing",
  ];
  for (const blocker of blockers) {
    const keys = transferFailureKeys(typed("personal_transfer_incomplete", blocker));
    expect(keys).toHaveLength(2);
    expect(keys?.[0]?.startsWith("personalTransfer.blocker.")).toBe(true);
    expect(keys?.[1]).toBe("personalTransfer.incomplete");
  }
  expect(transferFailureKeys(typed("personal_transfer_incomplete", "native_history"))).toEqual([
    "personalTransfer.blocker.nativeHistory",
    "personalTransfer.incomplete",
  ]);
});

test("unknown reasons and diagnostic titles never select a message", () => {
  expect(
    transferFailureKeys(typed("personal_transfer_incomplete", "future_model", "file attachment")),
  ).toEqual(["personalTransfer.incomplete"]);
  expect(transferFailureKeys(typed("personal_transfer_incomplete", "toString"))).toEqual([
    "personalTransfer.incomplete",
  ]);
  expect(transferFailureKeys(typed("personal_transfer_conflict", "command_changed"))).toEqual([
    "personalTransfer.conflictCommand",
  ]);
  expect(transferFailureKeys(typed("personal_transfer_conflict", "preview_stale"))).toEqual([
    "personalTransfer.conflict",
  ]);
  expect(transferFailureKeys(new TypeError("lost"))).toEqual(["personalTransfer.unknown"]);
  expect(transferFailureKeys(new ProblemError(404, "not_found"))).toBeNull();
});

test("every disposition outcome has distinct wording", () => {
  expect(new Set(Object.values(OUTCOME_KEYS)).size).toBe(Object.keys(OUTCOME_KEYS).length);
});

const committedHost = (): HostDraftSnapshot => ({
  committed: { title: "원본 제목", icon: null, status: "draft" },
  title: "원본 제목",
  icon: "",
  status: "draft",
  saving: false,
  sourceDrafts: [null, { dirty: false, composing: false }],
});

test("host prepare requires a clean committed host around the real save barrier", async () => {
  let persisted = 0;
  const persist = () => {
    persisted++;
    return Promise.resolve(true);
  };
  expect(await hostTransferPrepare(committedHost, persist)).toBe(true);
  expect(persisted).toBe(1);
  // Whitespace the host's own title save would not send is not a draft.
  expect(
    await hostTransferPrepare(() => ({ ...committedHost(), title: " 원본 제목 " }), persist),
  ).toBe(true);
  const unsaved: [string, Partial<HostDraftSnapshot>][] = [
    ["markdown dirty", { sourceDrafts: [{ dirty: true, composing: false }] }],
    ["markdown composing", { sourceDrafts: [null, { dirty: false, composing: true }] }],
    ["failed or dirty title", { title: "실패한 새 제목" }],
    ["dirty icon", { icon: "🧪" }],
    ["dirty status", { status: "published" }],
    ["metadata save pending", { saving: true }],
    ["metadata loading", { committed: undefined }],
  ];
  for (const [label, change] of unsaved) {
    persisted = 0;
    const result = await hostTransferPrepare(() => ({ ...committedHost(), ...change }), persist);
    expect([label, result, persisted]).toEqual([label, false, 0]);
  }
  expect(await hostTransferPrepare(committedHost, () => Promise.resolve(false))).toBe(false);
  // A draft that appears while the save is awaited refuses too.
  let state = committedHost();
  const result = hostTransferPrepare(
    () => state,
    () => {
      state = { ...state, sourceDrafts: [{ dirty: true, composing: false }] };
      return Promise.resolve(true);
    },
  );
  expect(await result).toBe(false);
});

test("a legacy source project is recovered only from positively matching retained data", () => {
  const task = "00000000-0000-4000-8000-0000000000aa";
  const lists = [
    { queryKey: ["tasks", SOURCE, PROJECT, ""], data: { items: [{ id: task, title: "x" }] } },
    { queryKey: ["tasks", SOURCE, "other-project", ""], data: { items: [{ id: "someone" }] } },
    { queryKey: ["tasks", TEAM, "team-project", ""], data: { items: [{ id: task }] } },
  ];
  expect(retainedTaskProject(lists, SOURCE, task)).toBe(PROJECT);
  const mine = [
    {
      queryKey: ["workspace-tasks", SOURCE, "q"],
      data: { pages: [{ items: [{ id: task, projectId: PROJECT }] }] },
    },
  ];
  expect(retainedTaskProject(mine, SOURCE, task)).toBe(PROJECT);
  expect(retainedTaskProject([], SOURCE, task)).toBeNull();
  const ambiguous = [
    ...lists,
    { queryKey: ["task-layout", SOURCE, "other-project"], data: { tasks: [{ taskId: task }] } },
  ];
  expect(retainedTaskProject(ambiguous, SOURCE, task)).toBeNull();
});

test("project-independent source keys stay inside the source workspace and task", () => {
  const task = "00000000-0000-4000-8000-0000000000aa";
  const document = "00000000-0000-4000-8000-0000000000bb";
  const other = "00000000-0000-4000-8000-0000000000cc";
  const queries = [
    ["task", SOURCE, task],
    ["task-activity", SOURCE, task, "all"],
    ["task-time-entries", SOURCE, task],
    ["collection-item", SOURCE, "task", task],
    ["task-origins", SOURCE, task, null],
    ["task-origins", SOURCE, document, null],
    ["workspace-tasks", SOURCE, "q"],
    ["workspace-tasks", SOURCE, "q", "preview", 8],
    ["projects", SOURCE],
    ["collection", SOURCE, "c"],
    ["search", SOURCE, "q", "task"],
    ["backlinks", "task", SOURCE, task],
    // Sentinels: another task, another workspace, documents-only search.
    ["task", SOURCE, other],
    ["task-time-entries", SOURCE, other],
    ["task-origins", SOURCE, other, null],
    ["workspace-tasks", TEAM, "q"],
    ["search", SOURCE, "q", "document"],
    ["backlinks", "task", TEAM, task],
  ].map((queryKey) => ({ queryKey, data: undefined }));
  const keys = unscopedSourceKeys(queries, SOURCE, task, document).map((key) => key.join("/"));
  expect(keys).toEqual(queries.slice(0, 12).map(({ queryKey }) => queryKey.join("/")));
});
const TASK = "00000000-0000-4000-8000-000000000008",
  OTHER_TASK = "00000000-0000-4000-8000-000000000009",
  ORIGIN_DOCUMENT = "00000000-0000-4000-8000-00000000000a";
type DirtyKind = "title" | "dueDate" | "hierarchy" | "assignees" | "labels";
// The mounted form getter's shape (TaskDetailForm getMetadataDraftState).
function formDraft(dirtyKind: DirtyKind | null = null, pending: boolean | undefined = false) {
  const dirty = Object.freeze({
    title: dirtyKind === "title",
    dueDate: dirtyKind === "dueDate",
    hierarchy: dirtyKind === "hierarchy",
    assignees: dirtyKind === "assignees",
    labels: dirtyKind === "labels",
  });
  return Object.freeze({
    workspaceId: SOURCE,
    taskId: TASK,
    actorId: ACTOR,
    dirty,
    hasUnsavedMetadata: Object.values(dirty).some(Boolean),
    pending,
    expectedDates: Object.freeze({ startDate: null, dueDate: null, dueAt: null }),
  });
}
function cleanTaskHost(): TaskHostSnapshot {
  return {
    workspaceId: SOURCE,
    taskId: TASK,
    actorId: ACTOR,
    sessionId: SESSION,
    generation: 4,
    busy: false,
    draft: formDraft(),
    bodyGeneration: 1,
    bodyPending: false,
  };
}
test("task transfer prepare refuses every unsaved, pending or foreign draft before the body save", async () => {
  let persisted = 0;
  const persist = () => {
    persisted += 1;
    return Promise.resolve(true);
  };
  expect(await taskTransferPrepare(cleanTaskHost, persist)).toBe(true);
  expect(persisted).toBe(1);
  // Unacknowledged body edits before the save are what the save flushes.
  let host: TaskHostSnapshot = { ...cleanTaskHost(), bodyPending: true };
  expect(
    await taskTransferPrepare(
      () => host,
      () => {
        host = { ...host, bodyPending: false };
        return Promise.resolve(true);
      },
    ),
  ).toBe(true);
  const refused: [string, Partial<TaskHostSnapshot>][] = [
    ...(["title", "dueDate", "hierarchy", "assignees", "labels"] as const).map(
      (kind): [string, Partial<TaskHostSnapshot>] => [`dirty ${kind}`, { draft: formDraft(kind) }],
    ),
    ["form metadata pending", { draft: formDraft(null, true) }],
    ["host busy", { busy: true }],
    ["form not mounted", { draft: null }],
    ["form of another workspace", { draft: { ...formDraft(), workspaceId: TEAM } }],
    ["form of another task", { draft: { ...formDraft(), taskId: OTHER_TASK } }],
    ["form of another actor", { draft: { ...formDraft(), actorId: NEXT_SESSION } }],
    ["no auth actor", { actorId: "" }],
    ["no auth session (retired)", { sessionId: "" }],
  ];
  for (const [label, change] of refused) {
    persisted = 0;
    const result = await taskTransferPrepare(() => ({ ...cleanTaskHost(), ...change }), persist);
    expect([label, result, persisted]).toEqual([label, false, 0]);
  }
});
test("task transfer prepare refuses a failed body save and any drift while it is awaited", async () => {
  expect(await taskTransferPrepare(cleanTaskHost, () => Promise.resolve(false))).toBe(false);
  const drift: [string, (host: TaskHostSnapshot) => TaskHostSnapshot][] = [
    ["metadata draft appears", (host) => ({ ...host, draft: formDraft("title") })],
    ["metadata save starts", (host) => ({ ...host, draft: formDraft(null, true) })],
    ["host becomes busy", (host) => ({ ...host, busy: true })],
    ["body edit unacknowledged", (host) => ({ ...host, bodyPending: true })],
    ["body room reconnects", (host) => ({ ...host, bodyGeneration: 2 })],
    ["auth session changes", (host) => ({ ...host, sessionId: NEXT_SESSION, generation: 5 })],
    // A to B and back to A: the same identity values, a later host generation.
    ["auth session ABA", (host) => ({ ...host, generation: host.generation + 2 })],
    ["task changes", (host) => ({ ...host, taskId: OTHER_TASK, generation: 5 })],
    ["workspace changes", (host) => ({ ...host, workspaceId: TEAM, generation: 5 })],
    ["actor changes", (host) => ({ ...host, actorId: NEXT_SESSION, generation: 5 })],
    ["form remounts", (host) => ({ ...host, draft: null })],
  ];
  for (const [label, change] of drift) {
    let state = cleanTaskHost();
    const result = await taskTransferPrepare(
      () => state,
      () => {
        state = change(state);
        return Promise.resolve(true);
      },
    );
    expect([label, result]).toEqual([label, false]);
  }
});
test("task transfer mounts only for a personal task with its single own origin", () => {
  const personal = [{ id: SOURCE, kind: "personal" }];
  const origin = (taskId = TASK) => ({
    count: 1,
    items: [{ documentId: ORIGIN_DOCUMENT, taskId }],
  });
  expect(taskTransferDocument(personal, SOURCE, TASK, origin())).toBe(ORIGIN_DOCUMENT);
  expect(taskTransferDocument([{ id: TEAM, kind: "team" }], TEAM, TASK, origin())).toBeNull();
  expect(taskTransferDocument([{ id: SOURCE, kind: "team" }], SOURCE, TASK, origin())).toBeNull();
  expect(taskTransferDocument(undefined, SOURCE, TASK, origin())).toBeNull();
  expect(taskTransferDocument(personal, SOURCE, TASK, undefined)).toBeNull();
  expect(taskTransferDocument(personal, SOURCE, TASK, { count: 0, items: [] })).toBeNull();
  expect(
    taskTransferDocument(personal, SOURCE, TASK, {
      count: 2,
      items: [...origin().items, { documentId: crypto.randomUUID(), taskId: TASK }],
    }),
  ).toBeNull();
  expect(taskTransferDocument(personal, SOURCE, TASK, origin(OTHER_TASK))).toBeNull();
});
