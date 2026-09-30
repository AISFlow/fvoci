<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery, useQueryClient } from "@tanstack/vue-query";
import { computed } from "vue";
import { buildCommentTree } from "@/features/comments/comment-tree";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { commentsQuery, type CommentsTargetKind } from "@/lib/queries/comments";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import CommentCompose from "./CommentCompose.vue";
import CommentItem from "./CommentItem.vue";
import { useCommentActions } from "./useCommentActions";
import "@/features/comments/comments.css";

// A document's comment threads (features/comments/comment-panel.tsx).
const props = defineProps<{
  workspaceId: string;
  kind: CommentsTargetKind;
  targetId: string;
  projectId: string | null;
  currentUserId: string;
  readOnly: boolean;
}>();
const queryClient = useQueryClient();
const list = useInfiniteQuery(
  commentsQuery(props.workspaceId, props.kind, props.targetId, props.projectId),
);
const actions = useCommentActions({
  workspaceId: props.workspaceId,
  kind: props.kind,
  targetId: props.targetId,
  projectId: props.projectId,
  invalidate: () =>
    queryClient.invalidateQueries({
      queryKey: ["comments", props.workspaceId, props.kind, props.targetId],
    }),
});
const roots = computed(() =>
  buildCommentTree(list.data.value?.pages.flatMap((page) => page.items) ?? []),
);
const testId = computed(() => (props.kind === "document" ? "document-comments" : "task-comments"));
const notFound = computed(
  () => list.error.value instanceof ProblemError && list.error.value.status === 404,
);
</script>

<template>
  <QueryLoading v-if="list.isLoading.value" />
  <template v-else-if="list.error.value">
    <QueryError
      v-if="!notFound"
      :message="loadErrorMessage(list.error.value)"
      @retry="list.refetch()"
    />
  </template>
  <section
    v-else
    :id="testId"
    class="comment-panel"
    :aria-label="t('comment.title')"
    :data-testid="testId"
  >
    <h2 class="comment-panel__title">{{ t("comment.title") }}</h2>
    <p v-if="actions.actionError.value" role="alert" class="comment-panel__error">{{
      actions.actionError.value
    }}</p>
    <p v-if="roots.length === 0" class="py-6 text-base font-semibold break-keep">{{
      t("comment.empty")
    }}</p>
    <ul v-else class="comment-thread__list">
      <CommentItem
        v-for="root in roots"
        :key="root.comment.id"
        :node="root"
        :actions="actions"
        :current-user-id="currentUserId"
        :read-only="readOnly"
        :depth="0"
      />
    </ul>
    <UButton
      v-if="list.hasNextPage.value"
      class="w-fit"
      variant="outline"
      color="neutral"
      :disabled="list.isFetchingNextPage.value"
      @click="list.fetchNextPage()"
    >
      {{ t("comment.list.loadMore") }}
    </UButton>
    <CommentCompose
      v-if="!readOnly"
      v-model:draft="actions.draft.value"
      :actions="actions"
      @submit="actions.create.mutate({ text: $event, parentId: null })"
    />
  </section>
</template>
