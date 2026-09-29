import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { QueryClient, useInfiniteQuery, useQuery, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp, effectScope } from "vue";
import { documentTaskProjectsQuery, taskOriginsQuery } from "@/features/collections/origin-api";
import { collectionFieldsQuery, taskCollectionItemQuery } from "@/lib/queries/collections";
import {
  taskActivityQuery,
  taskAttachmentsQuery,
  taskBacklinksQuery,
  taskTimeEntriesQuery,
} from "@/features/tasks/queries";
import {
  actorName,
  changeItem,
  displayValue,
  FALLBACK_TIME_ZONE,
  fieldLabel,
  formatActivityTime,
} from "../comments/task-activity-format.ts";

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
  return readFileSync(path.join(import.meta.dirname, rel), "utf8").replace(/\/\*[\s\S]*?\*\/|\/\/.*/g, "");
}

test("time entries, backlinks, activity, collection item, and origins wait for ids", () => {
  assert.equal(taskTimeEntriesQuery("", "t").enabled, false);
  assert.equal(taskTimeEntriesQuery("w", "").enabled, false);
  assert.equal(taskTimeEntriesQuery("w", "t").enabled, true);
  assert.equal(taskBacklinksQuery("", "t").enabled, false);
  assert.equal(taskBacklinksQuery("w", "").enabled, false);
  assert.equal(taskBacklinksQuery("w", "t").enabled, true);
  assert.equal(taskActivityQuery("", "t", "all").enabled, false);
  assert.equal(taskActivityQuery("w", "", "comments").enabled, false);
  assert.equal(taskActivityQuery("w", "t", "changes").enabled, true);
  assert.equal(taskCollectionItemQuery("", "t").enabled, false);
  assert.equal(taskCollectionItemQuery("w", "").enabled, false);
  assert.equal(taskCollectionItemQuery("w", "t").enabled, true);
  assert.equal(collectionFieldsQuery("w", "").enabled, false);
  assert.equal(taskOriginsQuery("", { taskId: "t" }, null).enabled, false);
  assert.equal(taskOriginsQuery("w", {}, null).enabled, false);
  assert.equal(taskOriginsQuery("w", { taskId: "t" }, null).enabled, true);
  assert.equal(taskOriginsQuery("w", { documentId: "d" }, null).enabled, true);
  assert.equal(documentTaskProjectsQuery("w", undefined).enabled, false);
  assert.equal(documentTaskProjectsQuery("w", "d").enabled, true);
  assert.equal(taskAttachmentsQuery("w", "t").queryKey[0], "task-attachments");
  assert.equal("enabled" in taskAttachmentsQuery("w", "t"), false);
});

test("gated panel queries stay idle until both ids exist", () => {
  const client = queryClient();
  const { result, stop } = mount(client, () => ({
    time: useQuery(() => taskTimeEntriesQuery("", "")),
    backlinks: useQuery(() => taskBacklinksQuery("", "")),
    activity: useInfiniteQuery(() => taskActivityQuery("", "", "all")),
    collection: useQuery(() => taskCollectionItemQuery("", "")),
    origins: useQuery(() => taskOriginsQuery("", { taskId: "" }, null)),
  }));
  try {
    assert.equal(result.time.fetchStatus.value, "idle");
    assert.equal(result.backlinks.fetchStatus.value, "idle");
    assert.equal(result.activity.fetchStatus.value, "idle");
    assert.equal(result.collection.fetchStatus.value, "idle");
    assert.equal(result.origins.fetchStatus.value, "idle");
    assert.equal(result.time.isFetching.value, false);
    assert.equal(result.activity.isFetching.value, false);
  } finally {
    stop();
  }
});

test("activity display matches React field, actor, and list values", () => {
  assert.equal(fieldLabel("title"), "제목");
  assert.equal(fieldLabel("unknownField"), "unknownField");
  assert.equal(displayValue("title", null), "없음");
  assert.equal(displayValue("archived", true), "보관됨");
  assert.equal(displayValue("archived", false), "활성");
  assert.equal(displayValue("priority", "urgent"), "긴급");
  assert.equal(displayValue("type", "bug"), "버그");
  assert.equal(displayValue("recurrence", "weekly"), "매주");
  assert.equal(displayValue("assigneeIds", { items: [], totalCount: 0 }), "없음");
  assert.equal(
    displayValue("labelIds", {
      items: [{ id: "1", label: "A" }, { id: "2", label: null }],
      totalCount: 4,
    }),
    "A, 확인할 수 없는 항목 외 2개",
  );
  assert.equal(displayValue("statusId", { id: "s", label: null }), "확인할 수 없는 항목");
  assert.equal(
    actorName({
      type: "change",
      id: "1",
      createdAt: "2026-01-01T00:00:00Z",
      actor: null,
      channel: "api",
      kind: "changed",
      changes: [],
    }),
    "API",
  );
  assert.equal(
    actorName({
      type: "change",
      id: "1",
      createdAt: "2026-01-01T00:00:00Z",
      actor: { id: "u", name: "Yeonghwan" },
      channel: "web",
      kind: "created",
      changes: [],
    }),
    "Yeonghwan",
  );
  assert.equal(FALLBACK_TIME_ZONE, "Asia/Seoul");
  assert.equal(formatActivityTime("not-a-date", "Invalid/Zone"), "not-a-date");
  const created = changeItem({
    type: "change",
    id: "c1",
    createdAt: "2026-01-01T00:00:00Z",
    actor: null,
    channel: "system",
    kind: "created",
    changes: [],
  });
  assert.equal(created.kind, "created");
});

test("panel modules keep React testids, empty gates, and comment slots (source)", () => {
  const attachments = source("./TaskAttachmentsPanel.vue");
  const time = source("./TaskTimeEntries.vue");
  const backlinks = source("./TaskBacklinks.vue");
  const properties = source("../collections/TaskCollectionProperties.vue");
  const activity = source("../comments/TaskActivityPanel.vue");
  const origin = source("../documents/OriginPanel.vue");
  const comment = source("../comments/CommentItem.vue");
  assert.match(attachments, /createTaskAttachmentBridge/);
  assert.match(attachments, /readOnly && items\.value\.length === 0/);
  assert.match(time, /data-testid="task-time-entries"/);
  assert.match(time, /data-testid="task-time-total"/);
  assert.match(time, /id="task-time-started"/);
  assert.match(backlinks, /data-testid="task-backlinks"/);
  assert.match(backlinks, /items\.length > 0/);
  assert.match(properties, /data-testid="task-properties"/);
  assert.match(properties, /lookup\.data\.value\?\.item === null/);
  assert.match(activity, /data-testid="task-comments"/);
  assert.match(activity, /id="fv-comments"/);
  assert.match(activity, /kind: "task"/);
  assert.match(activity, /task\.activity\.empty/);
  assert.doesNotMatch(activity, /useCollabRoom/);
  assert.match(origin, /hideWhenEmpty/);
  assert.match(origin, /taskId\?: string/);
  assert.match(origin, /collection\.sourceDocument/);
  assert.match(origin, /v-if="documentId"/);
  assert.match(comment, /slot name="before"/);
  assert.match(comment, /slot name="meta"/);
});
