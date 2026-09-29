<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { ref, watch } from "vue";
import { workspaceDeleteInput, workspaceNameInput } from "@/lib/validators";
import { parseForm } from "./form";
import "@/features/settings/settings-shell.css";

const props = defineProps<{
  workspaceName: string;
  workspaceSlug: string;
  workspaceKind: string;
  canManage: boolean;
  isOwner: boolean;
  namePending: boolean;
  nameError: string | null;
  nameSaved: boolean;
  deletePending: boolean;
  deleteError: string | null;
}>();

const emit = defineEmits<{
  saveName: [name: string];
  delete: [confirmSlug: string];
}>();

const name = ref(props.workspaceName);
const nameFieldError = ref<string | null>(null);
const confirmSlug = ref("");
const confirmSlugError = ref<string | null>(null);

watch(
  () => props.workspaceName,
  (next, previous) => {
    if (name.value === previous) name.value = next;
  },
);

function onSaveName(): void {
  nameFieldError.value = null;
  const parsed = parseForm(workspaceNameInput, { name: name.value });
  if (!parsed.ok) {
    nameFieldError.value = parsed.message;
    return;
  }
  emit("saveName", parsed.data.name);
}

function onDelete(): void {
  confirmSlugError.value = null;
  const parsed = parseForm(workspaceDeleteInput, { confirmSlug: confirmSlug.value });
  if (!parsed.ok) {
    confirmSlugError.value = parsed.message;
    return;
  }
  emit("delete", parsed.data.confirmSlug);
}
</script>

<template>
  <section class="settings-section">
    <h1 class="settings-section__title">{{ t("settings.workspace") }}</h1>
    <p v-if="workspaceName !== '' && !(workspaceKind === 'team' && canManage)" class="text-sm font-medium break-keep">
      {{ workspaceName }}
    </p>
    <p v-if="workspaceSlug !== ''" class="settings-section__lede">
      {{ t("workspace.settings.slug") }} <span class="settings-tabular">{{ workspaceSlug }}</span>
    </p>
    <p v-if="workspaceKind === 'personal'" class="text-sm text-muted">{{ t("personal workspace is immutable") }}</p>
    <form v-if="workspaceKind === 'team' && canManage" class="settings-form" novalidate @submit.prevent="onSaveName">
      <label for="workspace-name">{{ t("workspace.name") }}</label>
      <div class="settings-form__row">
        <UInput
          id="workspace-name"
          v-model="name"
          class="min-w-0 flex-1 sm:min-w-40"
          :disabled="namePending"
          :aria-invalid="nameFieldError || nameError ? true : undefined"
        />
        <UButton type="submit" size="sm" :disabled="namePending">{{ t("workspace.save") }}</UButton>
      </div>
      <p v-if="nameFieldError" role="alert" class="settings-notice settings-notice--danger">{{ nameFieldError }}</p>
      <p v-if="nameError" role="alert" class="settings-notice settings-notice--danger">{{ nameError }}</p>
      <p v-if="nameSaved && !nameError" role="status" class="settings-notice settings-notice--ok">
        {{ t("workspace.settings.saved") }}
      </p>
    </form>
    <p v-if="workspaceKind === 'team' && !canManage" class="text-sm text-muted">{{ t("workspace.settings.readOnly") }}</p>
    <details v-if="workspaceKind === 'team' && isOwner" class="settings-disclosure">
      <summary class="settings-disclosure__summary">{{ t("workspace.delete") }}</summary>
      <div class="settings-disclosure__body">
        <form class="settings-form" novalidate @submit.prevent="onDelete">
          <label for="workspace-delete-confirm">{{ t("workspace.deleteConfirm") }}</label>
          <UInput
            id="workspace-delete-confirm"
            v-model="confirmSlug"
            type="text"
            autocomplete="off"
            :disabled="deletePending"
            :aria-invalid="confirmSlugError || deleteError ? true : undefined"
            :aria-describedby="confirmSlugError ? 'workspace-delete-confirm-error' : undefined"
          />
          <p
            v-if="confirmSlugError"
            id="workspace-delete-confirm-error"
            role="alert"
            class="settings-notice settings-notice--danger"
          >
            {{ confirmSlugError }}
          </p>
          <p v-if="deleteError" role="alert" class="settings-notice settings-notice--danger">{{ deleteError }}</p>
          <UButton type="submit" size="sm" color="error" :disabled="deletePending">{{ t("workspace.delete") }}</UButton>
        </form>
      </div>
    </details>
  </section>
</template>
