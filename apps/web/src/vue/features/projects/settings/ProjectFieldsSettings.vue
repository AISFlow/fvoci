<script setup lang="ts">
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed } from "vue";
import type { ProjectListItem } from "@/features/projects/queries";
import { loadErrorMessage } from "@/lib/api";
import { collectionFieldsQuery, collectionPrefix, projectCollectionQuery } from "@/lib/queries/collections";
import QueryError from "../../../components/QueryError.vue";
import QueryLoading from "../../../components/QueryLoading.vue";
import ProjectMilestonesSection from "../ProjectMilestonesSection.vue";
import ProjectLabelsSettings from "./ProjectLabelsSettings.vue";
import ProjectFieldManager from "./ProjectFieldManager.vue";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string; project: ProjectListItem }>();
const client = useQueryClient();
const collection = useQuery(() => projectCollectionQuery(props.workspaceId, props.project.id));
const fields = useQuery(() => collectionFieldsQuery(props.workspaceId, collection.data.value?.id ?? ""));
const canEdit = computed(() => props.project.status === "active" && props.project.canEdit);
async function refresh(): Promise<void> {
  if (collection.data.value) await client.invalidateQueries({ queryKey: collectionPrefix(props.workspaceId, collection.data.value.id) });
}
</script>

<template>
  <QueryLoading v-if="collection.isPending.value || (collection.data.value && fields.isPending.value)" />
  <QueryError v-else-if="collection.isError.value || fields.isError.value" :message="loadErrorMessage(collection.error.value ?? fields.error.value)" @retry="collection.refetch(); fields.refetch()" />
  <div v-else-if="collection.data.value && fields.data.value" class="settings-stack mt-4">
    <ProjectLabelsSettings :workspace-id="workspaceId" :project-id="project.id" :can-edit="canEdit" />
    <ProjectMilestonesSection :workspace-id="workspaceId" :project-id="project.id" :can-manage="canEdit" />
    <ProjectFieldManager :workspace-id="workspaceId" :collection-id="collection.data.value.id" :fields="fields.data.value.items" :can-manage="collection.data.value.canEdit && project.status === 'active'" :on-saved="refresh" />
  </div>
</template>
