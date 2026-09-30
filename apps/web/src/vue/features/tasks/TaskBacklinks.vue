<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { taskBacklinksQuery } from "@/features/tasks/queries";
import { itemPath } from "@/lib/href";
import AppLink from "../../components/AppLink.vue";

const props = defineProps<{
  slug: string;
  workspaceId: string;
  taskId: string;
}>();

const backlinks = useQuery(() => taskBacklinksQuery(props.workspaceId, props.taskId));
const items = computed(() => backlinks.data.value?.items ?? []);
</script>

<template>
  <section v-if="items.length > 0" class="flex flex-col gap-2" data-testid="task-backlinks">
    <h2 class="text-sm font-medium">{{ t("backlinks.title") }}</h2>
    <ul class="flex flex-col gap-1">
      <li v-for="item in items" :key="item.id">
        <AppLink v-if="item.from.displayId" :to="itemPath(slug, item.from.displayId)" class="break-keep hover:underline">
          {{ item.from.title }}
        </AppLink>
        <span v-else class="break-keep">{{ item.from.title }}</span>
      </li>
    </ul>
  </section>
</template>
