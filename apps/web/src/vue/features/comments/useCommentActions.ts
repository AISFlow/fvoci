import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, type Ref } from "vue";
import {
  createComment,
  deleteComment,
  patchComment,
  reactToComment,
  resolveComment,
  unresolveComment,
} from "@/features/comments/comment-api";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { groupsQuery } from "@/lib/queries";
import type { CommentsTargetKind } from "@/lib/queries/comments";

/** Comment mutations and draft state of a comment panel (features/comments/comment-actions.tsx). */
export function useCommentActions(target: {
  workspaceId: string;
  kind: CommentsTargetKind;
  targetId: string;
  projectId: string | null;
  invalidate: () => Promise<unknown>;
}) {
  const queryClient = useQueryClient();
  const { workspaceId, invalidate } = target;
  const groupsList = useQuery(groupsQuery(workspaceId));
  const groups = computed(() =>
    groupsList.error.value instanceof ProblemError ? [] : (groupsList.data.value?.items ?? []),
  );
  const draft = ref("");
  const replyDraft = ref("");
  const replyToId: Ref<string | null> = ref(null);
  const editingId: Ref<string | null> = ref(null);
  const editDraft = ref("");
  const actionError: Ref<string | null> = ref(null);
  const onError = (error: unknown) => {
    actionError.value = loadErrorMessage(error);
  };

  const create = useMutation({
    mutationFn: (body: { text: string; parentId?: string | null }) =>
      createComment(queryClient, target, body),
    onSuccess: async () => {
      draft.value = "";
      replyDraft.value = "";
      replyToId.value = null;
      actionError.value = null;
      await invalidate();
    },
    onError,
  });

  const patch = useMutation({
    mutationFn: ({ id, body }: { id: string; body: string }) => patchComment(workspaceId, id, body),
    onSuccess: async () => {
      editingId.value = null;
      editDraft.value = "";
      actionError.value = null;
      await invalidate();
    },
    onError,
  });

  const remove = useMutation({
    mutationFn: (id: string) => deleteComment(workspaceId, id),
    onSuccess: async () => {
      actionError.value = null;
      await invalidate();
    },
    onError,
  });

  const resolve = useMutation({
    mutationFn: (id: string) => resolveComment(workspaceId, id),
    onSuccess: invalidate,
    onError,
  });

  const unresolve = useMutation({
    mutationFn: (id: string) => unresolveComment(workspaceId, id),
    onSuccess: invalidate,
    onError,
  });

  const react = useMutation({
    mutationFn: ({ id, emoji, on }: { id: string; emoji: string; on: boolean }) =>
      reactToComment(workspaceId, id, emoji, on),
    onSuccess: invalidate,
    onError,
  });

  const pending = computed(
    () =>
      create.isPending.value ||
      patch.isPending.value ||
      remove.isPending.value ||
      resolve.isPending.value ||
      unresolve.isPending.value ||
      react.isPending.value,
  );

  return {
    groups,
    draft,
    replyDraft,
    replyToId,
    editingId,
    editDraft,
    actionError,
    create,
    patch,
    remove,
    resolve,
    unresolve,
    react,
    pending,
  };
}

export type CommentActions = ReturnType<typeof useCommentActions>;
