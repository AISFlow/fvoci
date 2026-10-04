<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute, useRouter } from "vue-router";
import type { CollectionViewType } from "@/features/collections/collection-view";
import type { ProjectListItem } from "@/features/projects/queries";
import type { components } from "@/generated/api";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { projectCollectionPath } from "@/lib/href";
import { projectCollectionQuery } from "@/lib/queries/collections";
import QueryError from "../../components/QueryError.vue";
import CollectionContents from "./CollectionContents.vue";

type Workspace = components["schemas"]["WorkspaceListItemResponse"];

const props = defineProps<{
  slug: string;
  workspace: Workspace;
  project: ProjectListItem;
  type: CollectionViewType;
}>();

const route = useRoute();
const router = useRouter();
const collection = useQuery(() => projectCollectionQuery(props.workspace.id, props.project.id));
// A cached Calendar may survive transport failures, never an authority denial.
const collectionRefreshFailed = computed(() => {
  const error = collection.error.value;
  return (
    props.type === "calendar" &&
    collection.data.value !== undefined &&
    collection.isError.value &&
    (error instanceof TypeError ||
      (error instanceof ProblemError && [408, 429, 500, 502, 503, 504].includes(error.status)))
  );
});
const viewId = computed(() => (typeof route.query.view === "string" ? route.query.view : null));

async function onOpenView(nextType: CollectionViewType, nextViewId: string | null): Promise<void> {
  if (nextType === props.type && nextViewId === viewId.value) return;
  const base = projectCollectionPath(props.slug, props.project.key, nextType);
  const href = nextViewId ? `${base}?view=${encodeURIComponent(nextViewId)}` : base;
  await router[nextType === props.type ? "replace" : "push"](href);
}
</script>

<template>
  <p v-if="collection.isPending.value" role="status">{{ t("collection.loading") }}</p>
  <QueryError
    v-else-if="collection.isError.value && !collectionRefreshFailed"
    :message="loadErrorMessage(collection.error.value)"
    @retry="collection.refetch()"
  />
  <template v-else-if="collection.data.value">
    <QueryError
      v-if="collectionRefreshFailed"
      :message="loadErrorMessage(collection.error.value)"
      @pointerdown.stop
      @focusin.stop
      @retry="collection.refetch()"
    />
    <CollectionContents
      :key="`${collection.data.value.id}:${type}`"
      :workspace-id="workspace.id"
      :slug="slug"
      :collection-id="collection.data.value.id"
      :project-id="project.id"
      :type="type"
      :initial-view-id="viewId"
      @open-view="onOpenView"
    />
  </template>
</template>
