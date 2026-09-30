<script setup lang="ts">
import { t } from "@fvoci/i18n";
import {
  actorName,
  displayValue,
  fieldLabel,
  formatActivityTime,
  type ActivityChangeItem,
} from "./task-activity-format";

defineProps<{
  item: ActivityChangeItem;
  timeZone: string;
}>();
</script>

<template>
  <li class="flex gap-3 border-b border-default/60 pb-4 last:border-b-0 last:pb-0">
    <div
      class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-muted text-muted"
      aria-hidden="true"
    >
      ◷
    </div>
    <div class="min-w-0 flex-1 space-y-1.5">
      <p class="text-sm">
        <span class="font-medium">{{ actorName(item) }}</span>
        {{ item.kind === "created" ? t("task.activity.created") : t("task.activity.changed") }}
      </p>
      <ul v-if="item.changes.length > 0" class="space-y-1 text-sm text-muted">
        <li
          v-for="change in item.changes"
          :key="change.field"
          class="flex min-w-0 flex-wrap items-baseline gap-x-1"
        >
          <span class="font-medium text-highlighted">{{ fieldLabel(change.field) }}</span>
          <span class="break-words">{{ displayValue(change.field, change.from) }}</span>
          <span aria-hidden="true">→</span>
          <span class="break-words text-highlighted">{{ displayValue(change.field, change.to) }}</span>
        </li>
      </ul>
      <p class="text-xs tabular-nums text-muted">
        <time :datetime="item.createdAt">{{ formatActivityTime(item.createdAt, timeZone) }}</time>
      </p>
    </div>
  </li>
</template>
