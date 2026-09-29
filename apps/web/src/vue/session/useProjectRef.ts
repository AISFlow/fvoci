import { useQuery } from "@tanstack/vue-query";
import { computed, toValue, type MaybeRefOrGetter } from "vue";
import { findProjectByKey, projectsQuery } from "@/features/projects/queries";
import { ProblemError } from "@/lib/api";
import { parseRef } from "@/lib/href";

/** `/w/:slug/:ref/...` to the workspace's project (the React useProjectRef's rules). */
export function useProjectRef(workspaceId: MaybeRefOrGetter<string | undefined>, ref: MaybeRefOrGetter<string>) {
  const projectKey = computed(() => {
    const parsed = parseRef(toValue(ref));
    return parsed?.kind === "project" ? parsed.key : null;
  });
  const projects = useQuery(() => projectsQuery(toValue(workspaceId) ?? ""));
  const project = computed(() => findProjectByKey(projects.data.value?.items, projectKey.value ?? ""));
  const notFound = computed(
    () =>
      projectKey.value === null ||
      (projects.isSuccess.value && project.value === undefined) ||
      (projects.error.value instanceof ProblemError && projects.error.value.status === 404),
  );
  /**
   * The list failed for another reason (5xx, network) and there is no cached
   * list to show: the page offers a retry. A failed background refetch keeps
   * the cached list (TanStack keeps `data` with status "error"), so the page
   * stays on it.
   */
  const failed = computed(
    () => projects.isError.value && projects.data.value === undefined && !notFound.value,
  );
  return { projects, project, notFound, failed, retry: () => projects.refetch() };
}
