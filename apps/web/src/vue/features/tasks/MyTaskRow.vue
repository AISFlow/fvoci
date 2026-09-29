<script setup lang="ts">
import { computed } from "vue";
import { RouterLink } from "vue-router";
import { isVueAppPath } from "@/app-boundary";
import { formatDateKo } from "@/lib/datetime";
import { formatDisplayId, itemPath } from "@/lib/href";

const props = defineProps<{
  slug: string;
  itemId: string;
  title: string;
  number: number;
  projectKey?: string;
  statusName?: string;
  due?: string | null;
  timeZone: string;
}>();

const href = computed(() =>
  props.projectKey ? itemPath(props.slug, formatDisplayId(props.projectKey, props.number)) : null,
);
const vuePath = computed(() => (href.value !== null ? isVueAppPath(href.value) : false));
const displayId = computed(() =>
  props.projectKey ? formatDisplayId(props.projectKey, props.number) : props.itemId.slice(0, 8),
);
</script>

<template>
  <RouterLink v-if="href && vuePath" :to="href" class="task-row" :data-testid="`my-task-${itemId}`">
    <span class="task-row__id">{{ displayId }}</span>
    <span class="task-row__title">{{ title }}</span>
    <span v-if="statusName" class="project-list__private">{{ statusName }}</span>
    <span v-if="due" class="project-list__private">{{ formatDateKo(due, timeZone) }}</span>
  </RouterLink>
  <a v-else-if="href" :href="href" class="task-row" :data-testid="`my-task-${itemId}`">
    <span class="task-row__id">{{ displayId }}</span>
    <span class="task-row__title">{{ title }}</span>
    <span v-if="statusName" class="project-list__private">{{ statusName }}</span>
    <span v-if="due" class="project-list__private">{{ formatDateKo(due, timeZone) }}</span>
  </a>
  <span v-else class="task-row">
    <span class="task-row__id">{{ displayId }}</span>
    <span class="task-row__title">{{ title }}</span>
    <span v-if="statusName" class="project-list__private">{{ statusName }}</span>
    <span v-if="due" class="project-list__private">{{ formatDateKo(due, timeZone) }}</span>
  </span>
</template>
