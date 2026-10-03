import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { compileScript, compileTemplate, parse } from "@vue/compiler-sfc";
import ts from "typescript";
import * as Vue from "vue";
import { renderToString } from "vue/server-renderer";
import { formatPersonName, t } from "@fvoci/i18n";
import { collabUserOf } from "@/features/documents/collab-model";
import { taskOriginsQuery } from "@/features/collections/origin-api";
import {
  persistTaskBodyBeforeArchive,
  runArchiveWithBodyPersist,
} from "@/features/tasks/task-archive-persist";
import { ProblemError } from "@/lib/api";
import { taskTransferDocument, taskTransferPrepare } from "../capture/personal-transfer-command";
import { projectTasksPath } from "@/lib/href";
import { ProblemError } from "@/lib/api";
import {
  compiledComponent,
  evaluate,
  renderFunction,
} from "../../../features/settings/compiled-component-test";
import { QueryClient, useQuery, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp, effectScope } from "vue";
import { isLocalAppPath as isVueAppPath } from "@/vue/route-paths";
import { lookupQuery, resolveLookupTarget, type LookupItem } from "@/features/tasks/lookup";
import { projectLabelsQuery, projectMilestonesQuery, taskQuery } from "@/features/tasks/queries";
import { workflowQuery } from "@/features/projects/queries";
import { WORKSPACE_ITEM_PATH } from "@/vue/route-paths";
import { VUE_ROUTE_PATHS } from "../../route-paths.ts";
import { leaveTo } from "../../session/navigation.ts";

function queryClient(): QueryClient {
  return new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
}

function mount<T>(client: QueryClient, use: () => T): { result: T; stop: () => void } {
  const app = createApp({ render: () => null });
  app.use(VueQueryPlugin, { queryClient: client });
  const scope = effectScope();
  const result = app.runWithContext(() => scope.run(use)) as T;
  return {
    result,
    stop: () => {
      scope.stop();
      client.clear();
    },
  };
}

function source(rel: string): string {
  return readFileSync(path.join(import.meta.dirname, rel), "utf8").replace(
    /\/\*[\s\S]*?\*\/|\/\/.*/g,
    "",
  );
}

await test("the live workspace-item path is the item-ref custom regex", () => {
  assert.equal(VUE_ROUTE_PATHS.workspaceItem, "/w/:slug/:ref([A-Za-z0-9-]{2,32}-[1-9]\\d{0,8})");
});

await test("the live workspace-item boundary regex takes item refs only", () => {
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/GNT-1"), true);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/gnt-12"), true);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/GNT-1/"), true);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/wiki-3"), false);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/WIKI-3"), false);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/GNT"), false);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/GNT-1/tasks"), false);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/GNT/gantt"), false);
  assert.equal(WORKSPACE_ITEM_PATH.test("/w/acme/GNT/tasks"), false);
  assert.equal(isVueAppPath("/w/acme/GNT-1"), true, "boundary now renders Vue");
  assert.equal(isVueAppPath("/w/acme/wiki-3"), true);
  assert.equal(isVueAppPath("/w/acme/GNT"), true);
});

await test("lookup, task, workflow, labels, milestones wait for workspace and ids", () => {
  assert.equal(lookupQuery("", "GNT-1").enabled, false);
  assert.equal(lookupQuery("w", "").enabled, false);
  assert.equal(lookupQuery("w", "GNT-1").enabled, true);
  assert.equal(taskQuery("", "t").enabled, false);
  assert.equal(taskQuery("w", "").enabled, false);
  assert.equal(taskQuery("w", "t").enabled, true);
  assert.equal(workflowQuery("", "p").enabled, false);
  assert.equal(workflowQuery("w", "").enabled, false);
  assert.equal(projectLabelsQuery("w", "").enabled, false);
  assert.equal(projectMilestonesQuery("", "p").enabled, false);
});

await test("item-page queries stay idle until both ids exist", () => {
  const client = queryClient();
  const { result, stop } = mount(client, () => ({
    lookup: useQuery(() => lookupQuery("", "")),
    task: useQuery(() => taskQuery("", "")),
    workflow: useQuery(() => workflowQuery("", "")),
    labels: useQuery(() => projectLabelsQuery("", "")),
    milestones: useQuery(() => projectMilestonesQuery("", "")),
  }));
  try {
    assert.equal(result.lookup.fetchStatus.value, "idle");
    assert.equal(result.task.fetchStatus.value, "idle");
    assert.equal(result.workflow.fetchStatus.value, "idle");
    assert.equal(result.labels.fetchStatus.value, "idle");
    assert.equal(result.milestones.fetchStatus.value, "idle");
    assert.equal(result.lookup.isFetching.value, false);
    assert.equal(result.task.isFetching.value, false);
  } finally {
    stop();
  }
});

await test("after delete the connected tasks list uses Vue navigation", () => {
  const assigns: string[] = [];
  const pushes: string[] = [];
  const env = {
    assign: (url: string) => assigns.push(url),
    push: (path: string) => pushes.push(path),
  };
  leaveTo("/w/acme/GNT/tasks", env);
  assert.deepEqual(assigns, []);
  assert.deepEqual(pushes, ["/w/acme/GNT/tasks"]);
  leaveTo("/w/acme/GNT/gantt", env);
  assert.deepEqual(assigns, []);
  assert.deepEqual(pushes, ["/w/acme/GNT/tasks", "/w/acme/GNT/gantt"]);
});

const doc: LookupItem = {
  kind: "document",
  id: "doc-1",
  displayId: "GNT-1",
  title: "문서",
  projectId: "p1",
};
const task: LookupItem = {
  kind: "task",
  id: "task-1",
  displayId: "GNT-2",
  title: "첫 일",
  projectId: "p1",
};

await test("lookup branches project documents, tasks, 404-class misses, and wiki-excluded items", () => {
  assert.deepEqual(resolveLookupTarget([], "GNT-1"), { kind: "miss" });
  assert.deepEqual(resolveLookupTarget([doc], "GNT-1"), { kind: "project-document", item: doc });
  assert.deepEqual(resolveLookupTarget([doc, task], "gnt-2"), { kind: "task", item: task });
  assert.deepEqual(resolveLookupTarget([task], "HID-2"), { kind: "miss" });
  assert.deepEqual(resolveLookupTarget([{ ...doc, projectId: null }], "GNT-1"), { kind: "miss" });
});

await test("WorkspaceItemPage lookup 404 / miss / project-document vs task (source)", () => {
  const page = source("../../pages/WorkspaceItemPage.vue");
  assert.match(page, /lookupTarget\.value\?\.kind === "miss"/);
  assert.match(page, /lookup\.error\.value\.status === 404/);
  assert.match(page, /task\.error\.value\.status === 404/);
  assert.match(page, /kind === "project-document"/);
  assert.match(page, /kind === "task"/);
  assert.match(page, /prefix !== "WIKI"/);
  assert.match(page, /useWorkspaceSession\(slug\)/);
});

await test("WorkspaceItemPage keeps PATCH MOVE trash archive clone delete and 409 refetch (source)", () => {
  const page = source("../../pages/WorkspaceItemPage.vue");
  assert.match(page, /api\.PATCH\("\/api\/v1\/workspaces\/\{workspace_id\}\/tasks\/\{task_id\}"/);
  assert.match(
    page,
    /api\.POST\("\/api\/v1\/workspaces\/\{workspace_id\}\/tasks\/\{task_id\}\/move"/,
  );
  assert.match(
    page,
    /api\.POST\("\/api\/v1\/workspaces\/\{workspace_id\}\/tasks\/\{task_id\}\/trash"/,
  );
  assert.match(
    page,
    /api\.POST\("\/api\/v1\/workspaces\/\{workspace_id\}\/tasks\/\{task_id\}\/clone"/,
  );
  assert.match(page, /api\.DELETE\("\/api\/v1\/workspaces\/\{workspace_id\}\/tasks\/\{task_id\}"/);
  const recovery = page.match(/async function refetchAfterConflict\([\s\S]*?\n\}/)?.[0];
  assert.ok(recovery);
  assert.match(recovery, /scope: ReturnType<typeof captureTaskMutationScope>/);
  assert.match(
    recovery,
    /err\.status !== 409[\s\S]*scope\.epoch !== patchEpoch[\s\S]*scope\.actorEpoch !== patchActorEpoch[\s\S]*\)\s*return;/,
  );
  assert.match(
    recovery,
    /await invalidateCapturedTask\(scope\);\s*if \(scope\.epoch !== patchEpoch\) return;\s*formEpoch\.value \+= 1/,
  );
  assert.match(page, /formEpoch\.value \+= 1/);
  assert.match(page, /leaveTo\(/);
  assert.match(page, /projectTasksPath/);
});

await test("task body uses collab kind task; project document uses kind document (source)", () => {
  const taskView = source("./TaskDetailView.vue");
  const page = source("../../pages/WorkspaceItemPage.vue");
  const docView = source("../documents/ProjectDocumentView.vue");
  function assertRoom(input: string, kind: string, target: string): void {
    const script = parse(input).descriptor.scriptSetup?.content;
    assert.ok(script);
    const tree = ts.createSourceFile("host.ts", script, ts.ScriptTarget.Latest, true);
    const calls: ts.CallExpression[] = [];
    const visit = (node: ts.Node): void => {
      if (
        ts.isCallExpression(node) &&
        ts.isIdentifier(node.expression) &&
        node.expression.text === "useCollabRoom"
      )
        calls.push(node);
      ts.forEachChild(node, visit);
    };
    visit(tree);
    assert.equal(calls.length, 1, "one owned collaboration room");
    const name = calls[0]?.arguments[0];
    assert.ok(name && ts.isCallExpression(name));
    assert.equal(name.expression.getText(tree), "collabRoomName");
    assert.equal(name.arguments.length, 3);
    assert.equal(name.arguments[0]?.getText(tree), "props.workspaceId");
    const actualKind = name.arguments[1];
    assert.ok(actualKind && ts.isStringLiteral(actualKind));
    assert.equal(actualKind.text, kind);
    assert.equal(name.arguments[2]?.getText(tree), target);
  }
  assertRoom(taskView, "task", "props.task.id");
  assert.equal((taskView.match(/useCollabRoom\(/g) ?? []).length, 1);
  assert.match(page, /collabRoomName\(workspace\.id, ['"]task['"]/);
  assert.match(page, /collabRoomName\(workspace\.id, ['"]document['"]/);
  assertRoom(docView, "document", "props.documentId");
  const taskRoom = 'collabRoomName(props.workspaceId, "task", props.task.id)';
  assert.throws(() => {
    assertRoom(
      taskView.replace(taskRoom, taskRoom.replace('"task"', '"document"')),
      "task",
      "props.task.id",
    );
  });
  assert.throws(() => {
    assertRoom(
      taskView.replace(taskRoom, taskRoom.replace("props.task.id", "props.documentId")),
      "task",
      "props.task.id",
    );
  });
  const room = readFileSync(
    path.join(import.meta.dirname, "../../collab/useCollabRoom.ts"),
    "utf8",
  );
  assert.match(room, /function retire\(/);
});

await test("TaskDetailView wires the React side panels without a second collab room (source)", () => {
  const taskView = source("./TaskDetailView.vue");
  assert.match(taskView, /TaskCollectionProperties/);
  assert.match(taskView, /TaskAttachmentsPanel/);
  assert.match(taskView, /TaskTimeEntries/);
  assert.match(taskView, /TaskActivityPanel/);
  assert.match(taskView, /TaskBacklinks/);
  assert.match(taskView, /OriginPanel/);
  assert.match(taskView, /hide-when-empty/);
  assert.match(taskView, /:task-id="task\.id"/);
  assert.match(taskView, /:current-user-id="currentUserId"/);
  assert.match(taskView, /bodyReadOnly \|\| task\.archivedAt != null/);
  const template = taskView.slice(taskView.indexOf("<template>"));
  const collectionAt = template.indexOf("TaskCollectionProperties");
  const bodyAt = template.indexOf("TaskBodyEditor");
  const attachAt = template.indexOf("TaskAttachmentsPanel");
  assert.ok(collectionAt > 0 && collectionAt < bodyAt && bodyAt < attachAt);
});

// Compile the actual parent; mock room/query snapshots and leaf components only.
// These assertions verify the grants passed to each child, not server authorization.
async function detailGrants(options: {
  pageReadOnly: boolean;
  sessionReadOnly: boolean;
  archived?: boolean;
  canEdit?: boolean;
  restoring?: boolean;
  archiving?: boolean;
  /** The task is in the actor's personal workspace with its single origin. */
  transferOrigin?: boolean;
}) {
  const filename = path.join(import.meta.dirname, "TaskDetailView.vue");
  const { descriptor } = parse(readFileSync(filename, "utf8"), { filename });
  const script = compileScript(descriptor, { id: "task-grants" });
  assert.ok(descriptor.template);
  const template = compileTemplate({
    source: descriptor.template.content,
    filename,
    id: "task-grants",
    compilerOptions: { bindingMetadata: script.bindings },
  });
  assert.deepEqual(template.errors, []);
  const grants = new Map<string, { readOnly: boolean; archivePending: boolean }>();
  const leaf = (name: string) =>
    Vue.defineComponent({
      inheritAttrs: false,
      props: ["readOnly", "archivePending"],
      setup(props, { attrs }) {
        return () => {
          const key = attrs["data-testid"] === "task-clone" ? "Clone" : name;
          grants.set(key, {
            readOnly: Boolean(props.readOnly),
            archivePending: Boolean(props.archivePending),
          });
          return null;
        };
      },
    });
  const imports: Record<string, unknown> = {
    vue: Vue,
    "@fvoci/i18n": { formatPersonName, t },
    "@/features/tasks/task-archive-persist": {
      // The actual persist sequencing is covered by the real archive-persist browser group.
      // Hold the archive boundary here to inspect every compiled child grant mid-flight.
      runArchiveWithBodyPersist: options.archiving
        ? ({ archive }: { archive: () => Promise<void> }) => archive()
        : runArchiveWithBodyPersist,
      persistTaskBodyBeforeArchive,
    },
    "@/features/collections/origin-api": { taskOriginsQuery },
    "@/lib/api": { ProblemError },
    "../capture/personal-transfer-command": { taskTransferDocument, taskTransferPrepare },
    "@/features/documents/collab-model": { collabUserOf },
    "@/lib/href": { projectTasksPath },
    "@/lib/queries": { meQuery: {}, workspacesQuery: { queryKey: ["workspaces"] } },
    "@tanstack/vue-query": {
      // Controlled shapes only: the personal workspace and this task's single
      // origin when asked for, otherwise nothing loaded and no error.
      useQuery: (input: unknown) => {
        const query = (typeof input === "function" ? (input as () => unknown)() : input) as {
          queryKey?: readonly unknown[];
        };
        const family = query.queryKey?.[0];
        const data = !options.transferOrigin
          ? null
          : family === "workspaces"
            ? { items: [{ id: "w", kind: "personal" }] }
            : family === "task-origins"
              ? { count: 1, items: [{ documentId: "d", taskId: "t" }] }
              : null;
        return { data: Vue.ref(data), error: Vue.ref(null) };
      },
    },
    "../../collab/useCollabRoom": {
      collabRoomName: () => "w:task:t",
      useCollabRoom: () => ({ session: Vue.ref({ readOnly: options.sessionReadOnly }) }),
    },
    "@/features/projects/projects.css": {},
  };
  for (const name of [
    "@nuxt/ui/components/Button.vue",
    "../../components/AppLink.vue",
    "../capture/PersonalTransferDialog.vue",
    "../../components/ConfirmActionButton.vue",
    "../collections/TaskCollectionProperties.vue",
    "../comments/TaskActivityPanel.vue",
    "../documents/OriginPanel.vue",
    "../documents/StarToggle.vue",
    "./TaskAttachmentsPanel.vue",
    "./TaskBacklinks.vue",
    "./TaskBodyEditor.vue",
    "./TaskDetailForm.vue",
    "./TaskTimeEntries.vue",
    "./TaskStopwatch.vue",
  ]) {
    imports[name] = { default: leaf(path.basename(name, ".vue")) };
  }
  const component = compiledComponent(evaluate(script.content, imports).default);
  component.render = renderFunction(evaluate(template.code, imports).render);
  let release = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const pending: Promise<unknown>[] = [];
  let restoreCalls = 0;
  const setup = component.setup;
  if (options.restoring || options.archiving) {
    component.setup = (props, context) => {
      const state = setup(props, context);
      const handler = state.handleArchiveToggle;
      assert.equal(typeof handler, "function");
      Vue.onServerPrefetch(() => {
        // Both invocations run before the HTTP archive/restore finishes.
        for (let i = 0; i < 2; i++) {
          pending.push(
            Promise.resolve(
              Reflect.apply(handler as (...args: unknown[]) => unknown, undefined, [
                Boolean(options.archiving),
              ]),
            ),
          );
        }
      });
      return state;
    };
  }
  const callbacks = Object.fromEntries(
    [
      "TitleBlur",
      "StatusChange",
      "PriorityChange",
      "HierarchySave",
      "DueDateBlur",
      "AssigneesChange",
      "LabelsChange",
      "MilestoneChange",
      "AddDependency",
      "RemoveDependency",
      "Trash",
      "Clone",
      "Delete",
    ].map((name) => [`on${name}`, () => Promise.resolve()]),
  );
  try {
    await renderToString(
      Vue.createSSRApp(component, {
        slug: "ws",
        workspaceId: "w",
        projectId: "p",
        projectKey: "TASK",
        currentUserId: "u",
        task: { id: "t", title: "Task", archivedAt: options.archived ? "2026-10-01" : null },
        statuses: [],
        members: [],
        labels: [],
        milestones: [],
        dependencyCandidates: [],
        readOnly: options.pageReadOnly,
        canEdit: options.canEdit ?? true,
        ...callbacks,
        onArchiveToggle: () => {
          restoreCalls++;
          return held;
        },
      }),
    );
  } finally {
    release();
    await Promise.all(pending);
  }
  return { grants, restoreCalls };
}

await test("readonly body admission leaves HTTP metadata editable without granting body or attachment writes", async () => {
  const { grants } = await detailGrants({ pageReadOnly: false, sessionReadOnly: true });
  for (const name of [
    "TaskDetailForm",
    "TaskCollectionProperties",
    "TaskTimeEntries",
    "TaskStopwatch",
    "TaskActivityPanel",
  ]) {
    assert.equal(grants.get(name)?.readOnly, false, name);
  }
  assert.equal(grants.has("Clone"), true);
  assert.equal(grants.get("TaskBodyEditor")?.readOnly, true);
  assert.equal(grants.get("TaskAttachmentsPanel")?.readOnly, true);
  assert.equal(grants.has("PersonalTransferDialog"), false);
});

await test("archived and permission-denied page rights keep metadata and body readonly", async () => {
  for (const options of [{ archived: true }, { canEdit: false }]) {
    const { grants } = await detailGrants({
      pageReadOnly: true,
      sessionReadOnly: false,
      ...options,
    });
    for (const name of [
      "TaskDetailForm",
      "TaskCollectionProperties",
      "TaskTimeEntries",
      "TaskStopwatch",
      "TaskActivityPanel",
    ]) {
      assert.equal(grants.get(name)?.readOnly, true, name);
    }
    assert.equal(grants.has("Clone"), false);
    assert.equal(grants.get("TaskAttachmentsPanel")?.readOnly, true);
    assert.equal(grants.get("TaskBodyEditor")?.readOnly, true);
  }
});

await test("in-flight archive and restore hold REST panels and body readonly and prevent duplicate dispatch", async () => {
  for (const archiving of [false, true]) {
    const { grants, restoreCalls } = await detailGrants({
      pageReadOnly: false,
      sessionReadOnly: false,
      restoring: !archiving,
      archiving,
    });
    for (const name of [
      "TaskDetailForm",
      "TaskCollectionProperties",
      "TaskTimeEntries",
      "TaskStopwatch",
      "TaskActivityPanel",
    ]) {
      assert.equal(grants.get(name)?.readOnly, true, name);
    }
    assert.equal(grants.has("Clone"), false);
    assert.equal(grants.get("TaskAttachmentsPanel")?.readOnly, true);
    assert.equal(grants.get("TaskDetailForm")?.archivePending, true);
    assert.equal(grants.get("TaskBodyEditor")?.readOnly, true);
    assert.equal(restoreCalls, 1);
  }
});

await test("a personal task with its own origin mounts the transfer only on an editable page", async () => {
  const editable = await detailGrants({
    pageReadOnly: false,
    sessionReadOnly: false,
    transferOrigin: true,
  });
  assert.equal(editable.grants.has("PersonalTransferDialog"), true);
  for (const options of [{ archived: true }, { canEdit: false }]) {
    const { grants } = await detailGrants({
      pageReadOnly: true,
      sessionReadOnly: false,
      transferOrigin: true,
      ...options,
    });
    assert.equal(grants.has("PersonalTransferDialog"), false);
    assert.equal(grants.get("TaskDetailForm")?.readOnly, true);
  }
});
