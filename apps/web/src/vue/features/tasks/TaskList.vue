<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref } from "vue";
import type { WorkflowStatus } from "@/features/projects/queries";
import type { TaskListItem } from "@/features/tasks/queries";
import {
  visibleTaskStatusSections,
  type TaskListStatusCount,
} from "@/features/tasks/task-list-page";
import { taskTypeLabel } from "@/features/tasks/task-types";
import { formatDisplayId, itemPath } from "@/lib/href";
import AppLink from "../../components/AppLink.vue";
import "@/features/projects/projects.css";

// The project's tasks grouped by workflow status (features/tasks/task-list.tsx):
// server status counts, collapsible sections, create per status and "load
// more" with its own error. AppLink keeps connected task navigation in Vue.
const props = defineProps<{
  slug: string;
  projectKey: string;
  items: readonly TaskListItem[];
  statusCounts: readonly TaskListStatusCount[];
  statuses: readonly WorkflowStatus[];
  canCreate: boolean;
  defaultStatusId: string | null;
  hasMore: boolean;
  loadMorePending: boolean;
  loadMoreError: string | null;
}>();
const emit = defineEmits<{ create: [statusId: string]; loadMore: [] }>();

const collapsed = ref(new Set<string>());
const sections = computed(() =>
  visibleTaskStatusSections(
    props.items,
    props.statuses,
    props.statusCounts,
    t("task.list.otherStatus"),
  ),
);
const catalogEmpty = computed(
  () => props.items.length === 0 && !props.statusCounts.some((row) => row.count > 0),
);

function toggle(id: string): void {
  const next = new Set(collapsed.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  collapsed.value = next;
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <div class="flex flex-wrap items-center justify-end gap-2">
      <UButton
        v-if="canCreate && defaultStatusId"
        size="sm"
        @click="emit('create', defaultStatusId)"
      >
        {{ t("task.create.new") }}
      </UButton>
    </div>
    <div
      v-if="catalogEmpty"
      class="mx-auto flex w-full max-w-lg flex-1 flex-col items-start justify-center gap-3 px-6 py-12 sm:px-8"
    >
      <p class="text-xl font-semibold break-keep text-highlighted">{{ t("task.view.empty") }}</p>
    </div>
    <div class="flex flex-col gap-8">
      <section v-for="section in sections" :key="section.id" class="task-status">
        <div class="task-status__head">
          <button
            type="button"
            class="task-status__toggle"
            :aria-expanded="!collapsed.has(section.id)"
            @click="toggle(section.id)"
          >
            <span>{{ section.name }}</span>
            <span class="task-status__count">{{ section.count }}</span>
          </button>
          <UButton
            v-if="canCreate && section.known"
            size="sm"
            variant="outline"
            color="neutral"
            :aria-label="t('task.list.createInStatus', { status: section.name })"
            @click="emit('create', section.id)"
          >
            {{ t("task.create") }}
          </UButton>
        </div>
        <ul v-if="!collapsed.has(section.id)" class="task-status-list">
          <li v-for="item in section.items" :key="item.id">
            <AppLink
              :to="itemPath(slug, formatDisplayId(projectKey, item.number))"
              class="task-row"
              :data-testid="`task-row-${item.id}`"
            >
              <span class="task-row__id">{{ formatDisplayId(projectKey, item.number) }}</span>
              <span class="task-row__title">{{ item.title }}</span>
              <span class="project-list__private">{{ taskTypeLabel(item.type) }}</span>
            </AppLink>
          </li>
        </ul>
      </section>
    </div>
    <div v-if="hasMore" class="flex flex-col items-start gap-2">
      <p v-if="loadMoreError" role="alert" class="task-form__alert">{{ loadMoreError }}</p>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="loadMorePending"
        @click="emit('loadMore')"
      >
        {{ loadMorePending ? t("load.loading") : t("task.list.loadMore") }}
      </UButton>
    </div>
  </div>
</template>
