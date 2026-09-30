<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { ref } from "vue";
import type { WorkflowStatus } from "@/features/projects/queries";
import {
  STATUS_CATEGORIES,
  asCategory,
  categoryLabel,
  statusPatch,
  type StatusPatch,
} from "./workflow";

const props = defineProps<{ row: WorkflowStatus; pending: boolean; readOnly: boolean }>();
const emit = defineEmits<{ patch: [id: string, patch: StatusPatch]; delete: [id: string] }>();
const name = ref(props.row.name);
const category = ref(asCategory(props.row.category));
function submit(): void {
  if (props.readOnly || props.pending || !name.value.trim()) return;
  const patch = statusPatch(props.row, name.value, category.value);
  if (Object.keys(patch).length) emit("patch", props.row.id, patch);
}
</script>

<template>
  <li class="flex flex-wrap items-end gap-2" :data-testid="`workflow-status-${row.id}`">
    <form class="flex min-w-0 flex-1 flex-wrap items-end gap-2" @submit.prevent="submit">
      <UInput
        v-model="name"
        :aria-label="t('project.workflow.statusName')"
        maxlength="100"
        :disabled="pending || readOnly"
        class="min-w-48"
      />
      <select
        v-model="category"
        :aria-label="categoryLabel(category)"
        :disabled="pending || readOnly"
        class="collection-select"
      >
        <option v-for="value in STATUS_CATEGORIES" :key="value" :value="value">{{
          categoryLabel(value)
        }}</option>
      </select>
      <UButton v-if="!readOnly" type="submit" size="sm" :disabled="pending || !name.trim()">{{
        t("project.workflow.saveStatus")
      }}</UButton>
    </form>
    <UButton
      v-if="!readOnly"
      type="button"
      size="sm"
      variant="outline"
      color="neutral"
      :disabled="pending"
      @click="emit('delete', row.id)"
      >{{ t("project.workflow.deleteStatus") }}</UButton
    >
  </li>
</template>
