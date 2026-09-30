import { useQuery } from "@tanstack/vue-query";
import { computed, toValue, type MaybeRefOrGetter } from "vue";
import { parseWikiRef } from "@/lib/href";
import { treeQuery } from "@/lib/queries/documents";

/**
 * `/w/:slug/WIKI-<n>` to the workspace's wiki document: the ref's number
 * (parseWikiRef) looked up in the workspace tree (treeQuery) among the nodes
 * outside any project.
 */
export function useWikiDocumentRef(
  workspaceId: MaybeRefOrGetter<string | undefined>,
  ref: MaybeRefOrGetter<string>,
) {
  const number = computed(() => parseWikiRef(toValue(ref))?.number ?? null);
  const tree = useQuery(() => treeQuery(toValue(workspaceId) ?? ""));
  const node = computed(() =>
    number.value === null
      ? undefined
      : tree.data.value?.items.find((item) => item.projectId === null && item.number === number.value),
  );
  /** The ref names no wiki document the user can see: the page goes to the wiki list. */
  const notFound = computed(
    () => number.value === null || (tree.isSuccess.value && node.value === undefined),
  );
  /**
   * The tree failed (5xx, network) and gives no document to show: the page
   * offers a retry. A failed background refetch keeps the cached tree, so a
   * page that resolved its document stays on it; while a retry is fetching
   * the page shows loading instead.
   */
  const failed = computed(
    () => tree.isError.value && node.value === undefined && !notFound.value && !tree.isFetching.value,
  );
  return { tree, node, notFound, failed, retry: () => tree.refetch() };
}
