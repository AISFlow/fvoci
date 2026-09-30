<script setup lang="ts">
import ProjectViewFrame from "../features/projects/ProjectViewFrame.vue";
import ProjectTasksPanel from "../features/tasks/ProjectTasksPanel.vue";
</script>

<template>
  <ProjectViewFrame v-slot="{ slug, workspace, project }" active="tasks">
    <ProjectTasksPanel :slug="slug" :workspace="workspace" :project="project" />
  </ProjectViewFrame>
</template>
