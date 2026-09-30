<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import UFormField from "@nuxt/ui/components/FormField.vue";
import UTextarea from "@nuxt/ui/components/Textarea.vue";
import UPageCard from "@nuxt/ui/components/PageCard.vue";
import { useMutation } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import { fieldTypeLabel } from "@/features/collections/field-type-label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import {
  FIELD_KEY_PATTERN,
  FIELD_TYPES,
  fieldTakesOptions,
  parseOptionLines,
  suggestedFieldKey,
  type FieldType,
} from "@/lib/collection-values";
import type { CollectionField } from "@/lib/queries/collections";
import ProjectFieldSettings from "./ProjectFieldSettings.vue";
import "@/features/collections/collections.css";

const props = defineProps<{
  workspaceId: string;
  collectionId: string;
  fields: readonly CollectionField[];
  canManage: boolean;
  onSaved: () => Promise<void>;
}>();
const baseId = useId();
const name = ref("");
const key = ref("");
const keyTouched = ref(false);
const type = ref<FieldType>("text");
const options = ref("");
const error = ref<string | null>(null);
const conflict = ref(false);
const keyValue = computed({
  get: () => (keyTouched.value ? key.value : suggestedFieldKey(name.value)),
  set: (value: string) => {
    keyTouched.value = true;
    key.value = value;
  },
});
const trimmedKey = computed(() => keyValue.value.trim());
const keyValid = computed(
  () => trimmedKey.value === "" || FIELD_KEY_PATTERN.test(trimmedKey.value),
);
const create = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields", {
        params: { path: { workspace_id: props.workspaceId, collection_id: props.collectionId } },
        body: {
          name: name.value.trim(),
          type: type.value,
          ...(trimmedKey.value === "" ? {} : { key: trimmedKey.value }),
          ...(fieldTakesOptions(type.value) ? { options: parseOptionLines(options.value) } : {}),
        },
      }),
    ),
  onMutate: () => {
    error.value = null;
  },
  onError: (err) => {
    error.value =
      err instanceof ProblemError && err.titleKnown ? err.title : t("collection.saveError");
  },
  onSuccess: async () => {
    name.value = "";
    key.value = "";
    keyTouched.value = false;
    options.value = "";
    await props.onSaved();
  },
});
function submit(): void {
  if (props.canManage && !create.isPending.value && keyValid.value && name.value.trim())
    create.mutate();
}
function preventComposingSubmit(event: KeyboardEvent): void {
  if (event.key === "Enter" && event.isComposing) event.preventDefault();
}
</script>

<template>
  <UPageCard
    as="section"
    variant="subtle"
    :aria-labelledby="`${baseId}-title`"
    data-testid="collection-field-manager"
  >
    <div>
      <h2 :id="`${baseId}-title`" class="settings-section__title">{{
        t("collection.fieldSettings")
      }}</h2>
      <p class="settings-section__lede">{{ t("project.settings.fields.description") }}</p>
    </div>
    <form
      v-if="canManage"
      class="flex flex-col gap-2"
      @submit.prevent="submit"
      @keydown="preventComposingSubmit"
    >
      <div class="collection-toolbar">
        <UFormField :label="t('collection.fieldName')" :name="'name'">
          <UInput
            :id="`${baseId}-name`"
            v-model="name"
            class="w-full"
            maxlength="100"
            :disabled="create.isPending.value"
          />
        </UFormField>
        <UFormField :label="t('collection.fieldKey')" :name="'key'">
          <UInput
            :id="`${baseId}-key`"
            v-model="keyValue"
            class="w-full"
            :aria-describedby="`${baseId}-key-hint`"
            :aria-invalid="keyValid ? undefined : true"
            maxlength="50"
            :disabled="create.isPending.value"
          />
        </UFormField>
        <UFormField :label="t('collection.fieldType')" :name="'type'">
          <select
            :id="`${baseId}-type`"
            v-model="type"
            :aria-label="t('collection.fieldType')"
            class="collection-select"
            :disabled="create.isPending.value"
          >
            <option v-for="value in FIELD_TYPES" :key="value" :value="value">{{
              fieldTypeLabel(value)
            }}</option>
          </select>
        </UFormField>
        <UButton
          type="submit"
          size="sm"
          :disabled="create.isPending.value || !name.trim() || !keyValid"
          >{{ t("collection.addField") }}</UButton
        >
      </div>
      <p :id="`${baseId}-key-hint`" class="text-sm text-muted">{{
        t("collection.fieldKey.hint")
      }}</p>
      <UFormField v-if="fieldTakesOptions(type)" :label="t('collection.options')" name="options">
        <UTextarea
          :id="`${baseId}-options`"
          v-model="options"
          class="w-full"
          :disabled="create.isPending.value"
        />
      </UFormField>
      <p v-if="error" role="alert" class="text-sm text-error">{{ error }}</p>
    </form>
    <p v-else class="text-sm text-muted">{{ t("collection.readonly") }}</p>
    <p v-if="conflict" role="alert" class="text-sm text-error">{{ t("collection.saveError") }}</p>
    <p v-if="fields.length === 0" class="text-sm text-muted">{{
      t("project.settings.fields.empty")
    }}</p>
    <div v-else class="flex flex-col">
      <ProjectFieldSettings
        v-for="field in fields"
        :key="`${field.id}:${field.version}`"
        :workspace-id="workspaceId"
        :collection-id="collectionId"
        :field="field"
        :can-manage="canManage"
        :on-saved="onSaved"
        @conflict="conflict = true"
      />
    </div>
  </UPageCard>
</template>
