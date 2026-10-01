import { useMutation, useQueryClient, type QueryClient } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import type { QueryKey } from "@tanstack/query-core";
import { settleTaskPatch } from "@/features/tasks/task-patch-cache";
import { taskMutationErrorMessage } from "@/features/tasks/task-errors";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { IsoDate } from "@/lib/iso-date";
import { meQuery } from "@/lib/queries";
import { rescheduleBody, type BarChange, type RescheduleItem } from "./reschedule-body";

export interface RescheduleRequest {
  readonly id: string;
  readonly item: RescheduleItem;
  readonly change: BarChange;
}

/** Resolves once no query under `queryKey` is fetching. */
function whenIdle(queryClient: QueryClient, queryKey: QueryKey): Promise<void> {
  return new Promise((resolve) => {
    let unsubscribe = () => {};
    const check = () => {
      if (queryClient.isFetching({ queryKey }) > 0) return;
      unsubscribe();
      resolve();
    };
    unsubscribe = queryClient.getQueryCache().subscribe(check);
    check();
  });
}

/**
 * Saves a bar change through PATCH /tasks/{id} with the layout's dates as
 * expectedDates (see reschedule-body.ts). While a save runs, `savingId` names
 * the task and the chart takes no other change: after a 200 the bar stays at
 * its new place (`pending`) until the refetched layout shows it, because a
 * change made from the old layout would send stale expectedDates.
 *
 * Refusals, with the message from taskMutationErrorMessage:
 * - 400 dependency_contradiction: nothing was written; the bar goes back.
 * - 409 document_version_mismatch (changed elsewhere), 409 task_archived or
 *   project_archived, 404 and other failures: the layout is refetched, so
 *   the bar shows the server's dates (and canEdit turns false when editing
 *   is no longer allowed).
 * - 401: /auth/me is refetched and the session guard sends the page to login.
 */
export function useRescheduleTask(
  context: () => { workspaceId: string; projectId: string; timeZone: string },
) {
  const queryClient = useQueryClient();
  const error = ref<string | null>(null);
  const failed = ref(false);

  const generation = ref(0);
  watch(
    [() => context().workspaceId, () => context().projectId],
    () => {
      generation.value += 1;
      error.value = null;
      failed.value = false;
    },
    { flush: "sync" },
  );
  type CapturedRequest = RescheduleRequest & {
    workspaceId: string;
    projectId: string;
    timeZone: string;
    generation: number;
  };
  const layoutKey = (request: CapturedRequest) =>
    ["task-layout", request.workspaceId, request.projectId] as const;

  const mutation = useMutation({
    mutationFn: async (request: CapturedRequest) => {
      const { workspaceId, timeZone } = request;
      const body = rescheduleBody(request.item, request.change, timeZone);
      if (body === null) return null;
      return ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
          params: { path: { workspace_id: workspaceId, task_id: request.id } },
          body,
        }),
      );
    },
    onMutate: (request) => {
      if (request.generation !== generation.value) return;
      error.value = null;
      failed.value = false;
    },
    onSuccess: async (saved) => {
      if (saved === null) return;
      await settleTaskPatch(queryClient, saved.workspaceId, saved.projectId, saved);
      await whenIdle(queryClient, ["task-layout", saved.workspaceId, saved.projectId]);
    },
    onError: async (err, request) => {
      if (request.generation === generation.value) {
        failed.value = true;
        error.value = taskMutationErrorMessage(err, "gantt.bar.failed");
      }
      if (err instanceof ProblemError && err.status === 401) {
        await queryClient.invalidateQueries({ queryKey: meQuery.queryKey });
        return;
      }
      if (err instanceof ProblemError && err.code === "dependency_contradiction") return;
      await queryClient.invalidateQueries({ queryKey: layoutKey(request) });
      await whenIdle(queryClient, layoutKey(request));
    },
  });

  const savingId = computed(() =>
    mutation.isPending.value && mutation.variables.value?.generation === generation.value
      ? mutation.variables.value.id
      : null,
  );
  /** The saved range to draw until the refetched layout has it. */
  const pending = computed<{ id: string; start: IsoDate; end: IsoDate } | null>(() => {
    const request = mutation.variables.value;
    if (
      !mutation.isPending.value ||
      failed.value ||
      !request ||
      request.generation !== generation.value
    )
      return null;
    return { id: request.id, start: request.change.start, end: request.change.end };
  });

  return {
    reschedule: (request: RescheduleRequest) => {
      // Snapshot at the gesture, before Vue Query's asynchronous onMutate step.
      mutation.mutate({ ...request, ...context(), generation: generation.value });
    },
    savingId,
    pending,
    error,
    dismissError: () => {
      error.value = null;
    },
  };
}
