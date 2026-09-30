import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
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
  assert.match(page, /err\.status === 409/);
  assert.match(page, /formEpoch\.value \+= 1/);
  assert.match(page, /leaveTo\(/);
  assert.match(page, /projectTasksPath/);
});

await test("task body uses collab kind task; project document uses kind document (source)", () => {
  const taskView = source("./TaskDetailView.vue");
  const page = source("../../pages/WorkspaceItemPage.vue");
  const docView = source("../documents/ProjectDocumentView.vue");
  assert.match(
    taskView,
    /useCollabRoom\(collabRoomName\(props\.workspaceId, "task", props\.task\.id\)/,
  );
  assert.equal((taskView.match(/useCollabRoom\(/g) ?? []).length, 1);
  assert.match(page, /collabRoomName\(workspace\.id, ['"]task['"]/);
  assert.match(page, /collabRoomName\(workspace\.id, ['"]document['"]/);
  assert.match(
    docView,
    /useCollabRoom\(\s*collabRoomName\(props\.workspaceId, "document", props\.documentId\)/,
  );
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
  assert.match(taskView, /readOnly \|\| task\.archivedAt != null/);
  const template = taskView.slice(taskView.indexOf("<template>"));
  const collectionAt = template.indexOf("TaskCollectionProperties");
  const bodyAt = template.indexOf("TaskBodyEditor");
  const attachAt = template.indexOf("TaskAttachmentsPanel");
  assert.ok(collectionAt > 0 && collectionAt < bodyAt && bodyAt < attachAt);
});
