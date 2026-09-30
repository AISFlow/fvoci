<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { computed, ref, useId } from "vue";
import { fieldTypeLabel } from "@/features/collections/field-type-label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { fieldTakesOptions, optionPatch, type OptionDraft } from "@/lib/collection-values";
import type { CollectionField } from "@/lib/queries/collections";
import ConfirmActionButton from "../../../components/ConfirmActionButton.vue";

const props = defineProps<{ workspaceId: string; collectionId: string; field: CollectionField; canManage: boolean; onSaved: () => Promise<void> }>();
const emit = defineEmits<{ conflict: [] }>();
const baseId = useId();
const name = ref(props.field.name);
const description = ref(props.field.description ?? "");
const options = ref<OptionDraft[]>(props.field.options.map(option => ({ id: option.id, label: option.label, deleted: option.deletedAt !== null })));
const saving = ref(false);
const error = ref<string | null>(null);
const disabled = computed(() => saving.value || !props.canManage);
const archived = computed(() => props.field.deletedAt !== null);
function moveUp(index: number): void {
  if (disabled.value || index === 0) return;
  const previous = options.value[index - 1];
  const current = options.value[index];
  if (previous && current) [options.value[index - 1], options.value[index]] = [current, previous];
}
async function save(deleted: boolean): Promise<void> {
  if (disabled.value) return;
  saving.value = true;
  error.value = null;
  try {
    await ensureOk(await api.PATCH("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields/{field_id}", {
      params: { path: { workspace_id: props.workspaceId, collection_id: props.collectionId, field_id: props.field.id } },
      body: { expectedVersion: props.field.version, name: name.value.trim(), description: description.value.trim() || null, deleted, ...(fieldTakesOptions(props.field.type) ? { options: optionPatch(options.value) } : {}) },
    }));
    await props.onSaved();
  } catch (err) {
    error.value = err instanceof ProblemError && err.status !== 409 && err.titleKnown ? err.title : t("collection.saveError");
    if (err instanceof ProblemError && err.status === 409) {
      // Refresh remounts this versioned row; keep the refusal visible in its parent.
      emit("conflict");
      await props.onSaved();
    }
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <details class="border-b border-default py-2" :data-testid="`field-settings-${field.key}`">
    <summary class="min-h-11 cursor-pointer py-2 text-sm font-medium">{{ field.name }} · {{ fieldTypeLabel(field.type) }}{{ archived ? ` · ${t("collection.archived")}` : "" }}</summary>
    <div class="flex flex-col gap-3 py-3">
      <div class="collection-field">
        <label :for="`${baseId}-name`">{{ t("collection.fieldName") }}</label>
        <UInput :id="`${baseId}-name`" v-model="name" class="w-full" maxlength="100" :disabled="disabled" />
      </div>
      <div class="collection-field">
        <label :for="`${baseId}-description`">{{ t("project.description") }}</label>
        <UInput :id="`${baseId}-description`" v-model="description" class="w-full" maxlength="1000" :disabled="disabled" />
      </div>
      <p class="text-sm text-muted">{{ t("collection.fieldKey") }}: <code>{{ field.key }}</code></p>
      <fieldset v-if="fieldTakesOptions(field.type)" class="flex flex-col gap-2">
        <legend class="text-sm font-medium">{{ t("collection.optionLabel") }}</legend>
        <div v-for="(option, index) in options" :key="option.id ?? `new-${index}`" class="flex flex-wrap items-center gap-2" data-testid="field-option">
          <UInput v-model="option.label" class="min-w-0 flex-1" :aria-label="`${t('collection.optionLabel')} ${index + 1}`" maxlength="100" :disabled="disabled" />
          <span v-if="option.deleted" class="text-sm text-muted">{{ t("collection.archived") }}</span>
          <UButton type="button" size="sm" variant="outline" color="neutral" :disabled="disabled || index === 0" @click="moveUp(index)">{{ t("collection.moveUp") }}</UButton>
          <UButton type="button" size="sm" variant="outline" color="neutral" :disabled="disabled" @click="option.deleted = !option.deleted">{{ option.deleted ? t("collection.restore") : t("collection.archive") }}</UButton>
        </div>
        <UButton type="button" size="sm" variant="outline" color="neutral" class="w-fit" :disabled="disabled" @click="options.push({ label: '', deleted: false })">{{ t("collection.addOption") }}</UButton>
      </fieldset>
      <div v-if="canManage" class="flex flex-wrap gap-2">
        <UButton type="button" size="sm" :disabled="saving || !name.trim()" @click="save(archived)">{{ t("collection.save") }}</UButton>
        <UButton v-if="archived" type="button" size="sm" variant="outline" color="neutral" :disabled="saving" @click="save(false)">{{ t("collection.restore") }}</UButton>
        <ConfirmActionButton v-else :title="t('collection.archive')" :description="t('collection.archiveDescription')" :action-label="t('collection.archive')" :disabled="saving" :action="() => save(true)">{{ t("collection.archive") }}</ConfirmActionButton>
      </div>
      <p v-if="error" role="alert" class="text-sm text-error">{{ error }}</p>
    </div>
  </details>
</template>
