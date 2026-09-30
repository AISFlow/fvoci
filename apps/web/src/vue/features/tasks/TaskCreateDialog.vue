<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref, useId, watch } from "vue";
import { taskCreatePayload, type TaskCreateBody } from "@/features/tasks/create-payload";
import {
  TASK_TYPES,
  TASK_TYPE_LABELS,
  isTaskType,
  type TaskType,
} from "@/features/tasks/task-types";
import NativeModal from "../../components/NativeModal.vue";
import "@/features/projects/projects.css";

// "New task" (features/tasks/task-create-dialog.tsx and task-form.tsx): title
// and type, checked by taskCreatePayload before the request; the request's
// error stays in the dialog.
const props = defineProps<{
  open: boolean;
  projectKey: string;
  pending: boolean;
  error: string | null;
}>();
const emit = defineEmits<{ close: []; submit: [body: TaskCreateBody] }>();

const dialogTitleId = useId();
const titleId = useId();
const typeId = useId();
const title = ref("");
const type = ref<TaskType>("task");
const titleError = ref<string | null>(null);
const typeError = ref<string | null>(null);

// Each opening starts from an empty form, as the React dialog remounts it.
watch(
  () => props.open,
  (open) => {
    if (!open) return;
    title.value = "";
    type.value = "task";
    titleError.value = null;
    typeError.value = null;
  },
);

function onTypeChange(event: Event): void {
  const next = (event.target as HTMLSelectElement).value;
  if (isTaskType(next)) type.value = next;
}

function onSubmit(): void {
  titleError.value = null;
  typeError.value = null;
  const parsed = taskCreatePayload({ title: title.value, type: type.value });
  if (!parsed.ok) {
    if (parsed.issue === "parent") typeError.value = t("task.parent.required");
    else if (parsed.issue === "type") typeError.value = t("task.form.type.label");
    else titleError.value = t("task.form.titleRequired");
    return;
  }
  emit("submit", parsed.body);
}
</script>

<template>
  <NativeModal :open="open" :labelled-by="dialogTitleId" @close="emit('close')">
    <nav :aria-label="t('nav.breadcrumb')">
      <ol class="task-home__crumb flex list-none gap-1.5 p-0">
        <li class="tabular-nums">{{ projectKey }}</li>
        <li aria-hidden="true">/</li>
        <li>{{ t("task.create.new") }}</li>
      </ol>
    </nav>
    <h2 :id="dialogTitleId" class="project-dialog__title">{{ t("task.create.new") }}</h2>
    <p class="task-home__note">{{ t("task.create.hint") }}</p>
    <form class="task-form" novalidate @submit.prevent="onSubmit">
      <div class="task-form__field">
        <label :for="titleId" class="text-sm font-medium">{{ t("task.col.title") }}</label>
        <input
          :id="titleId"
          class="h-10 rounded-md border border-default bg-default px-3 text-sm"
          :placeholder="t('task.form.placeholder')"
          :aria-invalid="titleError ? true : undefined"
          autofocus
          :disabled="pending"
          :value="title"
          @input="title = ($event.target as HTMLInputElement).value"
        />
        <p v-if="titleError" class="task-form__alert" role="alert">{{ titleError }}</p>
      </div>
      <div class="task-form__field">
        <label :for="typeId" class="text-sm font-medium">{{ t("task.form.type.label") }}</label>
        <select :id="typeId" :disabled="pending" :value="type" @change="onTypeChange">
          <option v-for="value in TASK_TYPES" :key="value" :value="value">{{
            TASK_TYPE_LABELS[value]
          }}</option>
        </select>
        <p v-if="typeError" class="task-form__alert" role="alert">{{ typeError }}</p>
      </div>
      <div class="task-form__actions">
        <UButton variant="outline" color="neutral" @click="emit('close')">{{
          t("task.create.cancel")
        }}</UButton>
        <UButton type="submit" :disabled="pending">
          {{ pending ? t("task.create.pending") : t("task.create") }}
        </UButton>
      </div>
    </form>
    <p v-if="error" role="alert" class="task-form__alert">{{ error }}</p>
  </NativeModal>
</template>
