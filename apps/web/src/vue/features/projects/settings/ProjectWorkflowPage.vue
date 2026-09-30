<script setup lang="ts">
import ProjectViewFrame from "../ProjectViewFrame.vue";
import ProjectWorkflowSettings from "./ProjectWorkflowSettings.vue";
</script>

<template>
  <ProjectViewFrame v-slot="{ workspace, project }" active="workflow">
    <ProjectWorkflowSettings
      :key="`${workspace.id}:${project.id}`"
      :workspace-id="workspace.id"
      :project="project"
    />
  </ProjectViewFrame>
</template>
