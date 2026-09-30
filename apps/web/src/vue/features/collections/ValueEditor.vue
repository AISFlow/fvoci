<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, useId, watch } from "vue";
import {
  draftFromValue,
  selectedOptionIds,
  selectedUserIds,
  valueFromDraft,
  type CollectionValue,
} from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import type { CollectionField } from "@/lib/queries/collections";
import "@/features/collections/collections.css";

const props = withDefaults(
  defineProps<{
    field: CollectionField;
    value: CollectionValue;
    members: readonly MemberOutput[];
    timeZone: string;
    readOnly: boolean;
    showLabel?: boolean;
    labelSuffix?: string;
    saveValue: (value: CollectionValue) => Promise<void>;
  }>(),
  { showLabel: true, labelSuffix: "" },
);

const controlId = useId();
const draft = ref(draftFromValue(props.value, props.timeZone));
watch(
  () => draftFromValue(props.value, props.timeZone),
  (next) => {
    draft.value = next;
  },
);
const error = ref(false);
const saving = ref(false);
const disabled = computed(() => props.readOnly || saving.value || props.field.deletedAt !== null);
const selectedOptions = computed(() => selectedOptionIds(props.value));
const selectedUsers = computed(() => selectedUserIds(props.value));
const accessibleName = computed(() =>
  props.labelSuffix ? `${props.field.name} · ${props.labelSuffix}` : props.field.name,
);
const people = computed(() => props.field.type === "user" || props.field.type === "user_multi");
const currentSingle = computed(() =>
  people.value ? (selectedUsers.value[0] ?? "") : (selectedOptions.value[0] ?? ""),
);
const multiSelected = computed(() => (people.value ? selectedUsers.value : selectedOptions.value));
const multiChoices = computed(() =>
  people.value
    ? props.members.map((member) => ({ id: member.userId, label: formatPersonName(member), deleted: false }))
    : props.field.options.map((option) => ({
        id: option.id,
        label: option.label,
        deleted: option.deletedAt !== null,
      })),
);
const inputType = computed(() => {
  switch (props.field.type) {
    case "number":
      return "number";
    case "date":
      return "date";
    case "datetime":
      return "datetime-local";
    default:
      return "text";
  }
});

async function save(next: CollectionValue): Promise<void> {
  saving.value = true;
  error.value = false;
  try {
    await props.saveValue(next);
  } catch {
    error.value = true;
  } finally {
    saving.value = false;
  }
}

function onSingleChange(event: Event): void {
  const id = (event.target as HTMLSelectElement).value;
  void save(id === "" ? null : people.value ? { users: [id] } : { options: [id] });
}

function onMultiToggle(id: string, checked: boolean): void {
  const ids = checked ? [...multiSelected.value, id] : multiSelected.value.filter((item) => item !== id);
  void save(ids.length === 0 ? null : people.value ? { users: ids } : { options: ids });
}

function onDraftSubmit(event: Event): void {
  event.preventDefault();
  if (disabled.value || draft.value === draftFromValue(props.value, props.timeZone)) return;
  const next = valueFromDraft(props.field.type, draft.value, props.timeZone);
  if (next === "invalid") {
    error.value = true;
    return;
  }
  void save(next);
}

function onDraftKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter" && event.isComposing) event.preventDefault();
}
</script>

<template>
  <div class="flex min-w-0 flex-col gap-1" :data-testid="`value-editor-${field.key}`">
    <span v-if="showLabel" class="text-dense text-muted">
      {{ field.name }}{{ field.type === "datetime" ? ` · ${timeZone}` : ""
      }}{{ field.description ? ` — ${field.description}` : "" }}
    </span>
    <input
      v-if="field.type === 'checkbox'"
      :id="controlId"
      type="checkbox"
      class="size-5"
      :aria-label="accessibleName"
      :disabled="disabled"
      :checked="value !== null && 'checkbox' in value && value.checkbox"
      @change="save({ checkbox: ($event.target as HTMLInputElement).checked })"
    />
    <select
      v-else-if="field.type === 'select' || field.type === 'user'"
      :id="controlId"
      class="collection-select"
      :aria-label="accessibleName"
      :disabled="disabled"
      :value="currentSingle"
      @change="onSingleChange"
    >
      <option value="">{{ t("collection.unassigned") }}</option>
      <template v-if="people">
        <option v-for="member in members" :key="member.userId" :value="member.userId">
          {{ formatPersonName(member) }}
        </option>
      </template>
      <template v-else>
        <option
          v-for="option in field.options"
          :key="option.id"
          :value="option.id"
          :disabled="option.deletedAt !== null && option.id !== currentSingle"
        >
          {{ option.label }}{{ option.deletedAt ? ` · ${t("collection.archived")}` : "" }}
        </option>
      </template>
    </select>
    <fieldset
      v-else-if="['multi_select', 'checkboxes', 'labels', 'user_multi'].includes(field.type)"
      class="flex flex-wrap gap-x-3 gap-y-1"
      :aria-label="accessibleName"
    >
      <label
        v-for="choice in multiChoices"
        :key="choice.id"
        :for="`${controlId}-${choice.id}`"
        class="flex min-h-8 items-center gap-1.5 text-sm"
      >
        <input
          :id="`${controlId}-${choice.id}`"
          type="checkbox"
          :disabled="disabled || (choice.deleted && !multiSelected.includes(choice.id))"
          :checked="multiSelected.includes(choice.id)"
          @change="onMultiToggle(choice.id, ($event.target as HTMLInputElement).checked)"
        />
        {{ choice.label }}{{ choice.deleted ? ` · ${t("collection.archived")}` : "" }}
      </label>
    </fieldset>
    <form v-else class="flex flex-wrap items-center gap-2" @submit="onDraftSubmit" @keydown="onDraftKeydown">
      <textarea
        v-if="field.type === 'paragraph'"
        :id="controlId"
        class="collection-textarea"
        :aria-label="accessibleName"
        :disabled="disabled"
        :value="draft"
        @input="draft = ($event.target as HTMLTextAreaElement).value"
      />
      <input
        v-else
        :id="controlId"
        class="h-9 min-w-0 flex-1 rounded-md border border-default bg-default px-3 text-sm"
        :aria-label="accessibleName"
        :disabled="disabled"
        :type="inputType"
        step="any"
        :value="draft"
        @input="draft = ($event.target as HTMLInputElement).value"
      />
      <UButton v-if="!readOnly" type="submit" size="sm" variant="outline" color="neutral" :disabled="disabled || draft === draftFromValue(value, timeZone)">
        {{ t("collection.save") }}
      </UButton>
    </form>
    <p v-if="error" role="alert" class="text-dense text-error">{{ t("collection.saveError") }}</p>
  </div>
</template>
