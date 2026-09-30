<script setup lang="ts">
import ProjectViewFrame from "../ProjectViewFrame.vue";
import ProjectFieldsSettings from "./ProjectFieldsSettings.vue";
</script>

<template>
  <ProjectViewFrame v-slot="{ workspace, project }" active="fields">
    <ProjectFieldsSettings
      :key="`${workspace.id}:${project.id}`"
      :workspace-id="workspace.id"
      :project="project"
    />
  </ProjectViewFrame>
</template>
