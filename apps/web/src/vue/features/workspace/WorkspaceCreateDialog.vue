<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, useId } from "vue";
import { ProblemError } from "@/lib/api";
import { workspaceCreateInput } from "@/lib/validators";
import NativeModal from "../../components/NativeModal.vue";
import { issueMessage, useAuthForm } from "../auth/useAuthForm";
import "@/features/workspace/workspace-aux.css";
import "@/features/projects/projects.css";
import "./workspace-home.css";

const props = defineProps<{
  open: boolean;
  onCreate: (input: { name: string; slug: string }) => Promise<void>;
}>();
const emit = defineEmits<{ close: [] }>();

const titleId = useId();
const rootError = ref<string | null>(null);
const form = useAuthForm({
  initial: { name: "", slug: "" },
  schema: workspaceCreateInput,
  ids: { name: "create-workspace-name", slug: "create-workspace-slug" },
});
const fieldError = computed(() => form.errors.name ?? form.errors.slug);

const onSubmit = form.handleSubmit(async (values) => {
  rootError.value = null;
  try {
    await props.onCreate(values);
    emit("close");
  } catch (error) {
    if (error instanceof ProblemError && (error.status === 400 || error.status === 409)) {
      form.setError("slug", issueMessage(error.status === 409 ? "i18n:slug taken" : "i18n:form.invalid"));
    } else {
      rootError.value =
        error instanceof ProblemError && error.titleKnown ? error.title : t("error.workspace.create");
    }
  }
});
</script>

<template>
  <NativeModal :open="open" :labelled-by="titleId" @close="emit('close')">
    <h2 :id="titleId" class="workspace-empty__heading">{{ t("workspace.create.dialog.title") }}</h2>
    <p class="workspace-create__hint">{{ t("workspace.create.dialog.description") }}</p>
    <form class="workspace-create" novalidate @submit="onSubmit">
      <div class="workspace-create__field">
        <label class="workspace-create__label" for="create-workspace-name">{{ t("workspace.create.name") }}</label>
        <input
          id="create-workspace-name"
          name="name"
          class="workspace-create__input"
          :disabled="form.submitting.value"
          @input="form.onInput('name', ($event.target as HTMLInputElement).value)"
        />
      </div>
      <div class="workspace-create__field">
        <label class="workspace-create__label" for="create-workspace-slug">{{ t("workspace.create.slug") }}</label>
        <input
          id="create-workspace-slug"
          name="slug"
          class="workspace-create__input"
          :disabled="form.submitting.value"
          @input="form.onInput('slug', ($event.target as HTMLInputElement).value)"
        />
        <p class="workspace-create__hint">{{ t("form.pattern.slug") }}</p>
      </div>
      <p v-if="fieldError" role="alert" class="workspace-create__alert">{{ fieldError }}</p>
      <p v-if="rootError" role="alert" class="workspace-create__alert">{{ rootError }}</p>
      <div class="workspace-empty__actions">
        <UButton type="submit" :disabled="form.submitting.value">{{ t("workspace.create.action") }}</UButton>
        <UButton type="button" variant="outline" color="neutral" @click="emit('close')">{{ t("common.cancel") }}</UButton>
      </div>
    </form>
  </NativeModal>
</template>
