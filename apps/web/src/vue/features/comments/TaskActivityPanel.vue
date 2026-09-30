<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { taskActivityQuery, type TaskActivityFilter } from "@/features/tasks/queries";
import type { components } from "@/generated/api";
import { loadErrorMessage } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import CommentCompose from "./CommentCompose.vue";
import CommentItem from "./CommentItem.vue";
import TaskActivityChangeItem from "./TaskActivityChangeItem.vue";
import { changeItem, FALLBACK_TIME_ZONE, formatActivityTime } from "./task-activity-format";
import { useCommentActions } from "./useCommentActions";
import "@/features/comments/comments.css";

type ApiActivityItem = components["schemas"]["ActivityItemOutput"];
type ActivityCommentItem = Extract<ApiActivityItem, { type: "comment" }>;

// Task comments and field changes in one feed (features/comments/task-activity-panel.tsx).
const props = withDefaults(
  defineProps<{
    workspaceId: string;
    taskId: string;
    currentUserId: string;
    readOnly?: boolean;
  }>(),
  { readOnly: false },
);

const queryClient = useQueryClient();
const filter = ref<TaskActivityFilter>("all");
const activity = useInfiniteQuery(() =>
  taskActivityQuery(props.workspaceId, props.taskId, filter.value),
);
const me = useQuery(meQuery);
const timeZone = computed(() => me.data.value?.timezone || FALLBACK_TIME_ZONE);
const actions = useCommentActions({
  workspaceId: props.workspaceId,
  kind: "task",
  targetId: props.taskId,
  projectId: null,
  invalidate: () =>
    Promise.all([
      queryClient.invalidateQueries({
        queryKey: ["task-activity", props.workspaceId, props.taskId],
      }),
      queryClient.invalidateQueries({
        queryKey: ["comments", props.workspaceId, "task", props.taskId],
      }),
    ]),
});
const activityItems = computed(
  () => activity.data.value?.pages.flatMap((page) => page.items) ?? [],
);

function commentNode(item: ActivityCommentItem) {
  return { comment: item.comment, children: [] };
}
</script>

<template>
  <QueryLoading v-if="activity.isLoading.value" />
  <QueryError
    v-else-if="activity.error.value"
    :message="loadErrorMessage(activity.error.value)"
    @retry="activity.refetch()"
  />
  <section
    v-else
    id="fv-comments"
    tabindex="-1"
    class="comment-panel task-activity-panel"
    :aria-label="t('task.activity.title')"
    data-testid="task-comments"
  >
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h2>{{ t("task.activity.title") }}</h2>
      <label class="task-activity-panel__filter">
        <span class="sr-only">{{ t("task.activity.filter.label") }}</span>
        <select
          :aria-label="t('task.activity.filter.label')"
          :value="filter"
          @change="filter = ($event.target as HTMLSelectElement).value as TaskActivityFilter"
        >
          <option value="all">{{ t("task.activity.filter.all") }}</option>
          <option value="comments">{{ t("task.activity.filter.comments") }}</option>
          <option value="changes">{{ t("task.activity.filter.changes") }}</option>
        </select>
      </label>
    </div>
    <p v-if="actions.actionError.value" role="alert" class="comment-panel__error">{{
      actions.actionError.value
    }}</p>
    <div
      v-if="activityItems.length === 0"
      class="mx-auto flex w-full max-w-lg flex-1 flex-col items-start justify-center gap-3 px-6 py-12 sm:px-8"
    >
      <p class="text-title break-keep font-semibold">{{ t("task.activity.empty") }}</p>
    </div>
    <ul class="task-activity-panel__list flex flex-col gap-4">
      <template v-for="item in activityItems" :key="`${item.type}:${item.id}`">
        <TaskActivityChangeItem
          v-if="item.type === 'change'"
          :item="changeItem(item)"
          :time-zone="timeZone"
        />
        <CommentItem
          v-else
          :node="commentNode(item)"
          :actions="actions"
          :current-user-id="currentUserId"
          :read-only="readOnly"
          :depth="0"
        >
          <template v-if="item.parent" #before>
            <div class="comment-thread__parent-preview">
              <p class="comment-thread__parent-label">
                {{
                  t("task.activity.replyTo", {
                    name: item.parent.actor?.name ?? t("task.activity.actor.unknown"),
                  })
                }}
              </p>
              <p>{{ item.parent.body }}</p>
            </div>
          </template>
          <template #meta>
            <p class="comment-thread__meta">
              {{ item.actor?.name ?? t("task.activity.actor.unknown") }} ·
              <time :datetime="item.createdAt">{{
                formatActivityTime(item.createdAt, timeZone)
              }}</time>
            </p>
          </template>
        </CommentItem>
      </template>
    </ul>
    <UButton
      v-if="activity.hasNextPage.value"
      class="w-fit"
      variant="outline"
      color="neutral"
      :disabled="activity.isFetchingNextPage.value"
      @click="activity.fetchNextPage()"
    >
      {{ t("task.activity.loadMore") }}
    </UButton>
    <CommentCompose
      v-if="!readOnly && filter !== 'changes'"
      v-model:draft="actions.draft.value"
      :actions="actions"
      @submit="actions.create.mutate({ text: $event, parentId: null })"
    />
  </section>
</template>
