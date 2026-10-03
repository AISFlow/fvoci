import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import * as Query from "@tanstack/vue-query";
import ts from "typescript";
import * as Vue from "vue";
import { parse } from "vue/compiler-sfc";
import { loadErrorMessage, ProblemError } from "../../../lib/api";
import { inputScope } from "./capture-command";
import * as Command from "./personal-transfer-command";

const uuid = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const ACTOR = uuid(1),
  SESSION = uuid(2),
  OTHER_SESSION = uuid(3),
  SOURCE = uuid(4),
  DOCUMENT = uuid(5),
  TASK = uuid(6),
  PERSONAL_PROJECT = uuid(7),
  TEAM = uuid(8),
  OTHER_TEAM = uuid(9),
  PROJECT = uuid(10),
  BACKLOG = uuid(11),
  LIMITED = uuid(12),
  OTHER_DOCUMENT = uuid(13),
  OTHER_TASK = uuid(14),
  OTHER_PROJECT = uuid(15);
const DIGEST = "d".repeat(64);

function deferred() {
  let resolve!: (value?: unknown) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<unknown>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function record(value: unknown): Record<string, unknown> {
  assert.ok(typeof value === "object" && value !== null);
  return value as Record<string, unknown>;
}
const renderer = Vue.createRenderer({
  createElement: () => ({}),
  createText: () => ({}),
  createComment: () => ({}),
  setText() {},
  setElementText() {},
  patchProp() {},
  parentNode: () => null,
  nextSibling: () => null,
  insert() {},
  remove() {},
});
function memoryStorage(onRemove: (key: string) => void = () => undefined) {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      data.set(key, value);
    },
    removeItem: (key: string) => {
      onRemove(key);
      data.delete(key);
    },
  };
}
type Call = { method: string; args: unknown[] };

// Executes the retained SFC setup with real Vue reactivity/lifetime and real
// command storage helpers. HTTP and the host are controlled: this is not a
// Rust/DB or browser witness.
function harness(options: { prepare?: () => Promise<boolean> } = {}) {
  const host: { workspaceId: string; documentId: string | null; prepare?: () => Promise<boolean> } =
    { workspaceId: SOURCE, documentId: DOCUMENT, prepare: options.prepare };
  const props = Vue.reactive(host);
  const me = Vue.ref({ userId: ACTOR, sessionId: SESSION });
  const meError = Vue.ref<unknown>(null);
  const calls: Call[] = [];
  const previews: ReturnType<typeof deferred>[] = [];
  const confirms: ReturnType<typeof deferred>[] = [];
  const storage = memoryStorage((key) => calls.push({ method: "forget", args: [key] }));
  const listeners = new Set<() => void>();
  let confirmAnswer = true;
  const client = new Query.QueryClient();
  // Settlement can be held at a barrier or rejected once to test durability.
  let settlementGate: Promise<unknown> | null = null;
  let failNextSettlement = false;
  client.invalidateQueries = (filters?: Query.InvalidateQueryFilters) => {
    calls.push({ method: "invalidate", args: [...(filters?.queryKey ?? [])] });
    if (failNextSettlement) {
      failNextSettlement = false;
      return Promise.reject(new Error("settlement failed"));
    }
    return (settlementGate ?? Promise.resolve()).then(() => undefined);
  };
  // Options getters stay reactive; a disabled query (empty ID) has no data.
  const reactiveQuery = (input: unknown, data: (options: Record<string, unknown>) => unknown) => {
    const options = Vue.computed(() =>
      record(typeof input === "function" ? (input as () => unknown)() : input),
    );
    const value = Vue.computed(() => data(options.value));
    return {
      data: value,
      error: Vue.ref(null),
      isSuccess: Vue.computed(() => value.value !== undefined),
    };
  };
  const injected = {
    ...Vue,
    defineProps: () => props,
    t: (key: string) => key,
    useQueryClient: Query.useQueryClient,
    useRouter: () => ({
      push: (path: string) => {
        calls.push({ method: "navigate", args: [path] });
        return Promise.resolve();
      },
    }),
    useId: () => "transfer",
    useQuery: (input: unknown) => {
      const kind = record(typeof input === "function" ? (input as () => unknown)() : input).kind;
      if (kind === "me") return { data: me, error: meError, isSuccess: Vue.ref(true) };
      return reactiveQuery(input, (options) => {
        switch (options.kind) {
          case "documentMeta":
            return options.workspace && options.document ? { version: 1 } : undefined;
          // Document B's pair lives in a different personal project.
          case "origins":
            return options.workspace
              ? {
                  count: 1,
                  items: [{ taskId: options.document === OTHER_DOCUMENT ? OTHER_TASK : TASK }],
                }
              : undefined;
          case "task":
            return options.workspace && options.task
              ? {
                  id: options.task,
                  version: 1,
                  projectId: options.task === OTHER_TASK ? OTHER_PROJECT : PERSONAL_PROJECT,
                }
              : undefined;
          case "workspaces":
            return {
              items: [
                { id: SOURCE, kind: "personal", name: "개인", slug: "me", role: "owner" },
                { id: TEAM, kind: "team", name: "연구 팀", slug: "Lab", role: "member" },
                { id: OTHER_TEAM, kind: "team", name: "다른 팀", slug: "other", role: "member" },
              ],
            };
          case "projects":
            return options.workspace
              ? {
                  items: [
                    {
                      id: PROJECT,
                      key: "PUB",
                      name: "공개 프로젝트",
                      canEdit: true,
                      status: "active",
                      rootDocumentId: uuid(20),
                    },
                    {
                      id: uuid(21),
                      key: "RO",
                      name: "읽기 전용",
                      canEdit: false,
                      status: "active",
                      rootDocumentId: uuid(22),
                    },
                  ],
                }
              : undefined;
          default:
            return options.workspace && options.project
              ? {
                  statuses: [
                    { id: LIMITED, category: "in_progress", name: "WIP", wipLimit: 2 },
                    { id: BACKLOG, category: "backlog", name: "대기", wipLimit: null },
                  ],
                }
              : undefined;
        }
      });
    },
    documentMetaQuery: (workspace: string, document: string) => ({
      kind: "documentMeta",
      workspace,
      document,
    }),
    taskOriginsQuery: (workspace: string, target: { documentId?: string }) => ({
      kind: "origins",
      workspace,
      document: target.documentId,
    }),
    taskQuery: (workspace: string, task: string) => ({ kind: "task", workspace, task }),
    meQuery: { kind: "me" },
    workspacesQuery: { kind: "workspaces" },
    projectsQuery: (workspace: string) => ({ kind: "projects", workspace }),
    workflowQuery: (workspace: string, project: string) => ({
      kind: "workflow",
      workspace,
      project,
    }),
    invalidateTaskCaches: (_client: unknown, ...args: unknown[]) => {
      calls.push({ method: "taskCaches", args });
      return Promise.resolve();
    },
    inputScope,
    previewPersonalTransfer: (...args: unknown[]) => {
      calls.push({ method: "preview", args });
      const response = deferred();
      previews.push(response);
      return response.promise;
    },
    confirmPersonalTransfer: (...args: unknown[]) => {
      calls.push({ method: "confirm", args });
      const response = deferred();
      confirms.push(response);
      return response.promise;
    },
    ...Command,
    ProblemError,
    loadErrorMessage,
    crypto,
    window: {
      sessionStorage: storage,
      addEventListener: (_name: string, listener: () => void) => listeners.add(listener),
      removeEventListener: (_name: string, listener: () => void) => listeners.delete(listener),
      confirm: (message: string) => {
        calls.push({ method: "confirm-prompt", args: [message] });
        return confirmAnswer;
      },
    },
  };
  const { descriptor } = parse(
    readFileSync(new URL("PersonalTransferDialog.vue", import.meta.url), "utf8"),
  );
  assert.ok(descriptor.scriptSetup);
  let source = descriptor.scriptSetup.content;
  const parsed = ts.createSourceFile(
    "dialog.ts",
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  for (const statement of [...parsed.statements].reverse())
    if (ts.isImportDeclaration(statement))
      source = source.slice(0, statement.getFullStart()) + source.slice(statement.end);
  let dialog: Record<string, unknown> = {};
  let mounted = true;
  const app = renderer.createApp({
    setup() {
      const javascript = new Bun.Transpiler({ loader: "ts" }).transformSync(
        `(() => {${source}\nreturn {open,action,teamId,projectId,statusId,preview,pending,result,busy,error,ready,recoverable,source,review,confirm,retry,abandon,cancelReview,visit,paths};})()`,
      );
      dialog = record(runInNewContext(javascript, injected));
      return () => null;
    },
  });
  app.use(Query.VueQueryPlugin, { queryClient: client });
  app.mount({});
  const call = (name: string) => (dialog[name] as () => unknown)();
  const value = (name: string): unknown => {
    const ref = dialog[name];
    assert.ok(Vue.isRef(ref));
    return ref.value;
  };
  const set = (name: string, next: unknown) => {
    const ref = dialog[name];
    assert.ok(Vue.isRef(ref));
    ref.value = next;
  };
  return {
    client,
    props,
    me,
    meError,
    calls,
    storage,
    previews,
    confirms,
    call,
    value,
    set,
    count: (method: string) => calls.filter((entry) => entry.method === method).length,
    holdSettlement(gate: Promise<unknown> | null) {
      settlementGate = gate;
    },
    failSettlementOnce() {
      failNextSettlement = true;
    },
    answerConfirm(next: boolean) {
      confirmAnswer = next;
    },
    stop() {
      if (mounted) app.unmount();
      mounted = false;
      client.clear();
    },
  };
}
async function settle() {
  for (let step = 0; step < 10; step++) {
    await Vue.nextTick();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}
const previewValue = {
  digest: DIGEST,
  documentTitle: "원본 한글 🧑‍💻",
  taskTitle: "원본 한글 🧑‍💻",
  workspaceName: "연구 팀",
  projectName: "공개 프로젝트",
  projectVisibility: "workspace",
  sourceRetained: false,
  attachmentCount: 0,
  activityCount: 1,
};
const resultValue = {
  workspaceId: TEAM,
  projectId: PROJECT,
  documentId: DOCUMENT,
  documentNumber: 7,
  taskId: TASK,
  taskNumber: 8,
  replayed: false,
};
async function reviewed(h: ReturnType<typeof harness>, action: "copy" | "move" = "move") {
  h.set("open", true);
  h.set("action", action);
  h.set("teamId", TEAM);
  await settle();
  h.set("projectId", PROJECT);
  await settle();
  assert.equal(h.value("statusId"), BACKLOG, "WIP-limited column is never offered");
  assert.equal(h.value("ready"), true);
  const review = h.call("review") as Promise<void>;
  await settle();
  h.previews.at(-1)?.resolve(previewValue);
  await review;
}
function confirmBody(h: ReturnType<typeof harness>, index: number) {
  const call = h.calls.filter((entry) => entry.method === "confirm")[index];
  assert.ok(call);
  // Built inside the vm realm: compare content, not that realm's prototypes.
  return record(JSON.parse(JSON.stringify(call.args[1])));
}

await test("Cancel after the effect-free preview sends no command and stores nothing", async () => {
  const h = harness();
  try {
    await reviewed(h);
    const preview = record(h.value("preview"));
    assert.deepEqual(record(preview.selection), {
      action: "move",
      documentId: DOCUMENT,
      expectedDocumentVersion: 1,
      taskId: TASK,
      expectedTaskVersion: 1,
      destinationWorkspaceId: TEAM,
      destinationProjectId: PROJECT,
      destinationStatusId: BACKLOG,
    });
    h.call("cancelReview");
    await settle();
    assert.equal(h.value("preview"), null);
    assert.equal(h.count("confirm"), 0);
    assert.equal(h.storage.data.size, 0);
  } finally {
    h.stop();
  }
});

await test("a changed destination after review discards the reviewed digest", async () => {
  const h = harness();
  try {
    await reviewed(h);
    h.set("teamId", OTHER_TEAM);
    await settle();
    assert.equal(h.value("preview"), null);
    assert.equal(h.value("projectId"), "");
    await h.call("confirm");
    assert.equal(h.count("confirm"), 0);
  } finally {
    h.stop();
  }
});

await test("an unsaved host draft blocks review without any request", async () => {
  const h = harness({ prepare: () => Promise.resolve(false) });
  try {
    h.set("open", true);
    h.set("teamId", TEAM);
    await settle();
    h.set("projectId", PROJECT);
    await settle();
    await h.call("review");
    assert.equal(h.count("preview"), 0);
    assert.equal(h.value("error"), "personalTransfer.draft");
  } finally {
    h.stop();
  }
});

await test("lost success replays the identical stored command and settles once", async () => {
  const h = harness();
  try {
    await reviewed(h);
    const first = h.call("confirm") as Promise<void>;
    await settle();
    const stored = Command.recoverTransfer(h.storage, ACTOR, SESSION);
    assert.ok(stored, "command is durable before the response");
    assert.deepEqual(confirmBody(h, 0), stored.body);
    h.confirms[0]?.reject(new TypeError("network lost after commit"));
    await first;
    assert.equal(h.value("error"), "personalTransfer.unknown");
    assert.ok(h.value("pending"), "unknown outcome keeps the command");
    assert.equal(h.count("invalidate"), 0);
    const again = h.call("retry") as Promise<void>;
    await settle();
    assert.deepEqual(confirmBody(h, 1), confirmBody(h, 0), "same request UUID and payload");
    h.confirms[1]?.resolve({ ...resultValue, replayed: true });
    await again;
    await settle();
    assert.equal(Command.recoverTransfer(h.storage, ACTOR, SESSION), null);
    assert.equal(h.value("pending"), null);
    assert.ok(h.value("result"));
    const invalidated = h.calls
      .filter((entry) => entry.method === "invalidate")
      .map((entry) => entry.args.join("/"));
    assert.ok(invalidated.includes(`tree/${TEAM}`));
    assert.ok(invalidated.includes(`tree/${SOURCE}`), "MOVE removes the private source");
    assert.deepEqual(
      h.calls.filter((entry) => entry.method === "taskCaches").map((entry) => entry.args),
      [
        [TEAM, PROJECT, TASK, DOCUMENT],
        [SOURCE, PERSONAL_PROJECT, TASK, DOCUMENT],
      ],
    );
    assert.deepEqual(record(h.value("paths")), {
      document: "/w/lab/PUB-7",
      task: "/w/lab/PUB-8",
    });
  } finally {
    h.stop();
  }
});

await test("COPY settles destination caches only; the private source stays as it is", async () => {
  const h = harness();
  try {
    await reviewed(h, "copy");
    const pending = h.call("confirm") as Promise<void>;
    await settle();
    h.confirms[0]?.resolve({ ...resultValue, documentId: uuid(30), taskId: uuid(31) });
    await pending;
    const invalidated = h.calls
      .filter((entry) => entry.method === "invalidate")
      .map((entry) => entry.args.join("/"));
    assert.ok(!invalidated.some((key) => key.endsWith(SOURCE)));
    assert.equal(h.count("taskCaches"), 1);
  } finally {
    h.stop();
  }
});

await test("a definitive conflict keeps the command until explicit abandonment", async () => {
  const h = harness();
  try {
    await reviewed(h);
    const pending = h.call("confirm") as Promise<void>;
    await settle();
    h.confirms[0]?.reject(new ProblemError(409, "personal_transfer_conflict"));
    await pending;
    assert.equal(h.value("error"), "personalTransfer.conflict");
    assert.ok(Command.recoverTransfer(h.storage, ACTOR, SESSION));
    h.answerConfirm(false);
    h.call("abandon");
    assert.ok(Command.recoverTransfer(h.storage, ACTOR, SESSION), "declined prompt keeps it");
    h.answerConfirm(true);
    h.call("abandon");
    assert.equal(h.count("confirm-prompt"), 2);
    assert.equal(Command.recoverTransfer(h.storage, ACTOR, SESSION), null);
    assert.equal(h.value("pending"), null);
  } finally {
    h.stop();
  }
});

for (const change of ["session", "signed-out", "unmount"] as const) {
  await test(`${change}: a late success cannot settle caches or erase the command`, async () => {
    const h = harness();
    try {
      await reviewed(h);
      const pending = h.call("confirm") as Promise<void>;
      await settle();
      if (change === "session") h.me.value = { ...h.me.value, sessionId: OTHER_SESSION };
      if (change === "signed-out") h.meError.value = new ProblemError(401);
      if (change === "unmount") h.stop();
      h.confirms[0]?.resolve(resultValue);
      await pending;
      await settle();
      assert.equal(h.count("invalidate"), 0);
      assert.equal(h.count("taskCaches"), 0);
      assert.ok(
        Command.recoverTransfer(h.storage, ACTOR, SESSION),
        "retired lifetime leaves the stored command for its own credential",
      );
      if (change === "session") {
        assert.equal(h.value("result"), null);
        assert.equal(h.value("pending"), null, "new credential never sees the old command");
      }
    } finally {
      h.stop();
    }
  });
}

await test("target A-B-A settles the same actor's caches but publishes no result", async () => {
  const h = harness();
  try {
    await reviewed(h);
    const pending = h.call("confirm") as Promise<void>;
    await settle();
    h.props.documentId = OTHER_DOCUMENT;
    h.props.documentId = DOCUMENT;
    h.confirms[0]?.resolve(resultValue);
    await pending;
    await settle();
    assert.equal(Command.recoverTransfer(h.storage, ACTOR, SESSION), null);
    assert.ok(h.count("invalidate") > 0);
    assert.equal(h.value("result"), null);
  } finally {
    h.stop();
  }
});

await test("recovery-only mount offers the stored command without a source route", async () => {
  const h = harness();
  try {
    await reviewed(h);
    const pending = h.call("confirm") as Promise<void>;
    await settle();
    h.confirms[0]?.reject(new TypeError("lost"));
    await pending;
    h.set("open", false);
    h.props.documentId = null;
    await settle();
    assert.equal(h.value("recoverable"), true);
    h.set("open", true);
    await settle();
    assert.ok(h.value("pending"));
    const replay = h.call("retry") as Promise<void>;
    await settle();
    assert.deepEqual(confirmBody(h, 1), confirmBody(h, 0));
    h.confirms[1]?.resolve({ ...resultValue, replayed: true });
    await replay;
    assert.equal(Command.recoverTransfer(h.storage, ACTOR, SESSION), null);
    assert.deepEqual(
      h.calls.filter((entry) => entry.method === "taskCaches").map((entry) => entry.args),
      [
        [TEAM, PROJECT, TASK, DOCUMENT],
        [SOURCE, PERSONAL_PROJECT, TASK, DOCUMENT],
      ],
      "the stored command settles the moved source task without a source route",
    );
  } finally {
    h.stop();
  }
});

await test("a non-personal workspace offers no transfer source; the mount is recovery-only", async () => {
  const h = harness();
  try {
    assert.deepEqual(JSON.parse(JSON.stringify(h.value("source"))), {
      workspaceId: SOURCE,
      documentId: DOCUMENT,
      documentVersion: 1,
      taskId: TASK,
      taskVersion: 1,
      taskProjectId: PERSONAL_PROJECT,
    });
    h.props.workspaceId = TEAM;
    await settle();
    assert.equal(h.value("source"), null);
    h.set("open", true);
    h.set("teamId", TEAM);
    await settle();
    h.set("projectId", PROJECT);
    await settle();
    assert.equal(h.value("ready"), false);
    await h.call("review");
    assert.equal(h.count("preview"), 0);
  } finally {
    h.stop();
  }
});

/** Every cache key or task scope this run touched, flattened for sentinel checks. */
function touched(h: ReturnType<typeof harness>): string[] {
  return h.calls
    .filter((entry) => entry.method === "invalidate" || entry.method === "taskCaches")
    .map((entry) => entry.args.join("/"));
}
function forgottenAfterSettlement(h: ReturnType<typeof harness>): boolean {
  const forget = h.calls.findIndex((entry) => entry.method === "forget");
  let settled = -1;
  h.calls.forEach((entry, index) => {
    if (entry.method === "invalidate" || entry.method === "taskCaches") settled = index;
  });
  return forget > settled && settled >= 0;
}

await test("A-project -> B-project during a pending MOVE settles only A's captured scope and publishes nothing into B", async () => {
  const h = harness();
  try {
    await reviewed(h, "move");
    const pending = h.call("confirm") as Promise<void>;
    await settle();
    h.props.documentId = OTHER_DOCUMENT;
    await settle();
    assert.equal(record(h.value("source")).taskId, OTHER_TASK, "B is a different project's pair");
    h.confirms[0]?.resolve(resultValue);
    await pending;
    await settle();
    assert.deepEqual(
      h.calls.filter((entry) => entry.method === "taskCaches").map((entry) => entry.args),
      [
        [TEAM, PROJECT, TASK, DOCUMENT],
        [SOURCE, PERSONAL_PROJECT, TASK, DOCUMENT],
      ],
    );
    for (const key of touched(h))
      for (const foreign of [OTHER_DOCUMENT, OTHER_TASK, OTHER_PROJECT, OTHER_TEAM])
        assert.ok(!key.includes(foreign), `B or unrelated scope touched: ${key}`);
    assert.equal(h.value("result"), null, "no result published into B");
    assert.ok(forgottenAfterSettlement(h), "command removed only after settlement");
    assert.equal(Command.recoverTransfer(h.storage, ACTOR, SESSION), null);
  } finally {
    h.stop();
  }
});

await test("lost response -> close -> recovery mount without source settles the original source scope before removal", async () => {
  const h = harness();
  try {
    await reviewed(h, "move");
    const first = h.call("confirm") as Promise<void>;
    await settle();
    h.confirms[0]?.reject(new TypeError("response lost after commit"));
    await first;
    h.set("open", false);
    h.props.documentId = null;
    await settle();
    h.set("open", true);
    await settle();
    const replay = h.call("retry") as Promise<void>;
    await settle();
    assert.deepEqual(confirmBody(h, 1), confirmBody(h, 0));
    h.confirms[1]?.resolve({ ...resultValue, replayed: true });
    await replay;
    await settle();
    assert.ok(
      h.calls.some(
        (entry) =>
          entry.method === "taskCaches" &&
          entry.args.join("/") === [SOURCE, PERSONAL_PROJECT, TASK, DOCUMENT].join("/"),
      ),
    );
    assert.ok(forgottenAfterSettlement(h));
    for (const key of touched(h)) assert.ok(!key.includes(OTHER_TEAM), `unrelated: ${key}`);
  } finally {
    h.stop();
  }
});

function storeLegacyMove(h: ReturnType<typeof harness>) {
  // Shape stored before sourceTaskProjectId existed: the field is absent.
  const legacy = {
    actorId: ACTOR,
    sessionId: SESSION,
    sourceWorkspaceId: SOURCE,
    destinationSlug: "Lab",
    destinationProjectKey: "PUB",
    body: {
      requestId: uuid(40),
      confirmed: true as const,
      previewDigest: DIGEST,
      selection: {
        action: "move" as const,
        documentId: DOCUMENT,
        expectedDocumentVersion: 1,
        taskId: TASK,
        expectedTaskVersion: 1,
        destinationWorkspaceId: TEAM,
        destinationProjectId: PROJECT,
        destinationStatusId: BACKLOG,
      },
    },
  };
  h.storage.setItem(`fvoci:personal-transfer:${ACTOR}:${SESSION}`, JSON.stringify(legacy));
  return legacy;
}
const LEGACY_CONSUMERS = [
  ["workspace-tasks", SOURCE, "open-assigned"],
  ["workspace-tasks", SOURCE, "open-assigned", "preview", 8],
  ["task-time-entries", SOURCE, TASK],
  ["collection-item", SOURCE, "task", TASK],
  ["task-origins", SOURCE, DOCUMENT, null],
  ["task-origins", SOURCE, TASK, null],
  ["backlinks", "task", SOURCE, TASK],
  ["search", SOURCE, "q", "task"],
];
const LEGACY_SENTINELS = [
  ["workspace-tasks", OTHER_TEAM, "open-assigned"],
  ["task-time-entries", SOURCE, OTHER_TASK],
  ["search", SOURCE, "q", "document"],
];
function seedLegacyCaches(h: ReturnType<typeof harness>) {
  // Retained MyTasks pages hold the moved task without any project field.
  h.client.setQueryData(LEGACY_CONSUMERS[0] ?? [], {
    pages: [{ items: [{ id: TASK, title: "이동한 작업" }], nextCursor: "c2" }],
    pageParams: [null],
  });
  for (const key of [...LEGACY_CONSUMERS.slice(1), ...LEGACY_SENTINELS])
    h.client.setQueryData(key, { items: [] });
}
async function openLegacyRecovery(h: ReturnType<typeof harness>) {
  h.props.documentId = null;
  h.set("open", true);
  await settle();
  assert.ok(h.value("pending"), "legacy command is recoverable, not dropped");
}

await test("legacy MOVE without project metadata settles every source consumer before removal, sentinels untouched", async () => {
  const h = harness();
  try {
    const legacy = storeLegacyMove(h);
    seedLegacyCaches(h);
    await openLegacyRecovery(h);
    const barrier = deferred();
    h.holdSettlement(barrier.promise);
    const replay = h.call("retry") as Promise<void>;
    await settle();
    assert.deepEqual(confirmBody(h, 0), JSON.parse(JSON.stringify(legacy.body)));
    h.confirms[0]?.resolve({ ...resultValue, replayed: true });
    await settle();
    assert.ok(
      Command.recoverTransfer(h.storage, ACTOR, SESSION),
      "command kept while settlement is still pending",
    );
    barrier.resolve();
    await replay;
    await settle();
    const keys = touched(h);
    for (const key of LEGACY_CONSUMERS) assert.ok(keys.includes(key.join("/")), key.join("/"));
    for (const key of LEGACY_SENTINELS) assert.ok(!keys.includes(key.join("/")), key.join("/"));
    assert.ok(!keys.some((key) => key.startsWith(`${SOURCE}/`)), "no guessed project scope");
    assert.equal(h.count("taskCaches"), 1, "destination only");
    assert.ok(forgottenAfterSettlement(h));
    assert.equal(Command.recoverTransfer(h.storage, ACTOR, SESSION), null);
    const pages = h.client.getQueryData<{ pages: unknown[] }>(LEGACY_CONSUMERS[0] ?? []);
    assert.equal(pages?.pages.length, 1, "loaded MyTasks pages are retained");
  } finally {
    h.stop();
  }
});

await test("a failed legacy settlement keeps the identical command; its replay then settles and removes it", async () => {
  const h = harness();
  try {
    const legacy = storeLegacyMove(h);
    seedLegacyCaches(h);
    await openLegacyRecovery(h);
    h.failSettlementOnce();
    const first = h.call("retry") as Promise<void>;
    await settle();
    h.confirms[0]?.resolve({ ...resultValue, replayed: true });
    await first;
    await settle();
    assert.ok(Command.recoverTransfer(h.storage, ACTOR, SESSION), "survives failed settlement");
    assert.equal(h.value("error"), "personalTransfer.unknown");
    const second = h.call("retry") as Promise<void>;
    await settle();
    assert.deepEqual(confirmBody(h, 1), JSON.parse(JSON.stringify(legacy.body)));
    h.confirms[1]?.resolve({ ...resultValue, replayed: true });
    await second;
    await settle();
    assert.equal(Command.recoverTransfer(h.storage, ACTOR, SESSION), null);
  } finally {
    h.stop();
  }
});

await test("legacy MOVE uses a project only when a retained list positively holds the task", async () => {
  const h = harness();
  try {
    storeLegacyMove(h);
    h.client.setQueryData(["tasks", SOURCE, PERSONAL_PROJECT, ""], {
      items: [{ id: TASK, title: "이동한 작업" }],
    });
    h.client.setQueryData(["tasks", OTHER_TEAM, OTHER_PROJECT, ""], { items: [{ id: TASK }] });
    await openLegacyRecovery(h);
    const replay = h.call("retry") as Promise<void>;
    await settle();
    h.confirms[0]?.resolve({ ...resultValue, replayed: true });
    await replay;
    await settle();
    assert.deepEqual(
      h.calls.filter((entry) => entry.method === "taskCaches").map((entry) => entry.args),
      [
        [TEAM, PROJECT, TASK, DOCUMENT],
        [SOURCE, PERSONAL_PROJECT, TASK, DOCUMENT],
      ],
    );
    assert.ok(forgottenAfterSettlement(h));
  } finally {
    h.stop();
  }
});

await test("legacy MOVE uses the cached source task detail's project", async () => {
  const h = harness();
  try {
    storeLegacyMove(h);
    h.client.setQueryData(["task", SOURCE, TASK], { id: TASK, projectId: PERSONAL_PROJECT });
    await openLegacyRecovery(h);
    const replay = h.call("retry") as Promise<void>;
    await settle();
    h.confirms[0]?.resolve({ ...resultValue, replayed: true });
    await replay;
    await settle();
    assert.ok(touched(h).includes([SOURCE, PERSONAL_PROJECT, TASK, DOCUMENT].join("/")));
    assert.ok(forgottenAfterSettlement(h));
  } finally {
    h.stop();
  }
});
