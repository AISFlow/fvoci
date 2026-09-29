<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref } from "vue";
import { ProblemError } from "@/lib/api";
import { workspaceCreateInput } from "@/lib/validators";
import { issueMessage, useAuthForm } from "../auth/useAuthForm";
import "@/features/workspace/workspace-aux.css";
import "./workspace-home.css";

const props = defineProps<{
  isAdmin: boolean;
  onCreate: (input: { name: string; slug: string }) => Promise<void>;
  onLogout: () => void;
  error?: string | null;
  onRetry?: () => void;
}>();

const createError = ref<string | null>(null);
const form = useAuthForm({
  initial: { name: "", slug: "" },
  schema: workspaceCreateInput,
  ids: { name: "ws-name", slug: "ws-slug" },
});
const fieldError = computed(() => form.errors.name ?? form.errors.slug);

const onSubmit = form.handleSubmit(async (values) => {
  createError.value = null;
  try {
    await props.onCreate(values);
  } catch (err) {
    if (err instanceof ProblemError && err.status === 403) {
      createError.value = t("unauthorized");
    } else if (err instanceof ProblemError && (err.status === 400 || err.status === 409)) {
      form.setError("slug", issueMessage(err.status === 409 ? "i18n:slug taken" : "i18n:form.invalid"));
    } else if (err instanceof ProblemError) {
      createError.value = err.titleKnown ? err.title : t("error.workspace.create");
    } else {
      createError.value = t("error.network");
    }
  }
});
</script>

<template>
  <div v-if="error" class="workspace-empty">
    <p role="alert" class="workspace-empty__lead">{{ error }}</p>
    <UButton type="button" size="sm" class="w-fit" @click="onRetry">{{ t("load.retry") }}</UButton>
  </div>
  <div v-else-if="!isAdmin" class="workspace-empty">
    <p class="workspace-empty__lead">{{ t("workspace.none.invite") }}</p>
    <div class="workspace-empty__actions">
      <UButton type="button" variant="outline" color="neutral" size="sm" class="w-fit" @click="onLogout">
        {{ t("nav.logout") }}
      </UButton>
    </div>
  </div>
  <form v-else class="workspace-empty" novalidate @submit="onSubmit">
    <h1 class="workspace-empty__heading">{{ t("workspace.none") }}</h1>
    <div class="workspace-create__field">
      <label class="workspace-create__label" for="ws-name">{{ t("workspace.create.name") }}</label>
      <input
        id="ws-name"
        name="name"
        class="workspace-create__input"
        :disabled="form.submitting.value"
        @input="form.onInput('name', ($event.target as HTMLInputElement).value)"
      />
    </div>
    <div class="workspace-create__field">
      <label class="workspace-create__label" for="ws-slug">{{ t("workspace.create.slug") }}</label>
      <input
        id="ws-slug"
        name="slug"
        class="workspace-create__input"
        :disabled="form.submitting.value"
        @input="form.onInput('slug', ($event.target as HTMLInputElement).value)"
      />
      <p class="workspace-create__hint">{{ t("form.pattern.slug") }}</p>
    </div>
    <p v-if="fieldError" role="alert" class="workspace-create__alert">{{ fieldError }}</p>
    <p v-if="createError" role="alert" class="workspace-create__alert">{{ createError }}</p>
    <div class="workspace-empty__actions">
      <UButton type="submit" size="sm" :disabled="form.submitting.value">{{ t("workspace.create") }}</UButton>
      <UButton type="button" variant="outline" color="neutral" size="sm" class="w-fit" @click="onLogout">
        {{ t("nav.logout") }}
      </UButton>
    </div>
  </form>
</template>
