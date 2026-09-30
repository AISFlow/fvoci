<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, useId } from "vue";
import { customEqualsValue, isoToZonedLocal, SET_FIELD_TYPES } from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import type { CollectionField } from "@/lib/queries/collections";
import {
  addCustomFilter,
  removeCustomFilter,
  type CustomFilter,
  type ViewQuery,
} from "@/lib/view-query";
import "@/features/collections/collections.css";

// Property filters over the view query (features/collections/custom-filters.tsx):
// the applied filters as removable chips and a form that adds "equals" or
// "empty" for one field. A datetime value is read in the user's time zone.
const props = defineProps<{
  fields: readonly CollectionField[];
  members: readonly MemberOutput[];
  timeZone: string;
  query: ViewQuery;
}>();
const emit = defineEmits<{ change: [next: ViewQuery] }>();

const baseId = useId();
const fieldId = ref("");
const operator = ref<"equals" | "empty">("equals");
const raw = ref("");
const field = computed(() => props.fields.find((item) => item.id === fieldId.value));
const current = computed(() => props.query.filters.custom ?? []);
const value = computed(() =>
  field.value && operator.value === "equals"
    ? customEqualsValue(field.value.type, raw.value, props.timeZone)
    : null,
);
const ready = computed(
  () => Boolean(field.value) && (operator.value === "empty" || value.value !== null),
);
const people = computed(() => field.value?.type === "user" || field.value?.type === "user_multi");
const inputType = computed(() => {
  switch (field.value?.type) {
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

function describe(filter: CustomFilter): { name: string; text: string } {
  const match = props.fields.find((item) => item.id === filter.fieldId);
  const name = match?.name ?? filter.fieldId;
  if (filter.operator === "empty")
    return { name, text: `${name}: ${t("collection.filter.empty")}` };
  let label = String(filter.value);
  if (match && typeof filter.value === "string") {
    const option = match.options.find((item) => item.id === filter.value);
    const member = props.members.find((item) => item.userId === filter.value);
    if (option) label = option.label;
    else if (member) label = formatPersonName(member);
    else if (match.type === "datetime")
      label = isoToZonedLocal(filter.value, props.timeZone).replace("T", " ");
  }
  if (typeof filter.value === "boolean") {
    label = filter.value ? t("collection.filter.true") : t("collection.filter.false");
  }
  return { name, text: `${name} ${t("collection.filter.equals")} ${label}` };
}

function onFieldChange(event: Event): void {
  fieldId.value = (event.target as HTMLSelectElement).value;
  raw.value = "";
}

function onSubmit(): void {
  const selected = field.value;
  if (!selected || !ready.value) return;
  const filter: CustomFilter =
    operator.value === "empty"
      ? { fieldId: selected.id, operator: "empty" }
      : {
          fieldId: selected.id,
          operator: "equals",
          value: value.value as string | number | boolean,
        };
  emit("change", addCustomFilter(props.query, filter));
  raw.value = "";
}
</script>

<template>
  <div class="flex flex-col gap-2" data-testid="custom-filters">
    <ul v-if="current.length > 0" class="flex flex-wrap gap-2">
      <li
        v-for="(filter, index) in current"
        :key="`${filter.fieldId}:${index}`"
        class="tag-chip"
        data-color="blue"
      >
        {{ describe(filter).text }}
        <button
          type="button"
          class="tags-bar__remove"
          :aria-label="t('collection.filter.remove', { name: describe(filter).name })"
          @click="emit('change', removeCustomFilter(query, index))"
        >
          <span aria-hidden="true">×</span>
        </button>
      </li>
    </ul>
    <form class="collection-toolbar" @submit.prevent="onSubmit">
      <div class="collection-field">
        <label :for="`${baseId}-field`">{{ t("collection.filter.field") }}</label>
        <select
          :id="`${baseId}-field`"
          class="collection-select"
          :value="fieldId"
          @change="onFieldChange"
        >
          <option value="">{{ t("collection.none") }}</option>
          <option v-for="item in fields" :key="item.id" :value="item.id">{{ item.name }}</option>
        </select>
      </div>
      <div class="collection-field">
        <label :for="`${baseId}-operator`">{{ t("collection.filter.operator") }}</label>
        <select
          :id="`${baseId}-operator`"
          class="collection-select"
          :value="operator"
          @change="
            operator = ($event.target as HTMLSelectElement).value === 'empty' ? 'empty' : 'equals'
          "
        >
          <option value="equals">{{ t("collection.filter.equals") }}</option>
          <option value="empty">{{ t("collection.filter.empty") }}</option>
        </select>
      </div>
      <div v-if="field && operator === 'equals'" class="collection-field">
        <label :for="`${baseId}-value`">{{ t("collection.filter.value") }}</label>
        <select
          v-if="field.type === 'checkbox'"
          :id="`${baseId}-value`"
          class="collection-select"
          :value="raw"
          @change="raw = ($event.target as HTMLSelectElement).value"
        >
          <option value="">{{ t("collection.none") }}</option>
          <option value="true">{{ t("collection.filter.true") }}</option>
          <option value="false">{{ t("collection.filter.false") }}</option>
        </select>
        <select
          v-else-if="SET_FIELD_TYPES.includes(field.type)"
          :id="`${baseId}-value`"
          class="collection-select"
          :value="raw"
          @change="raw = ($event.target as HTMLSelectElement).value"
        >
          <option value="">{{ t("collection.none") }}</option>
          <template v-if="people">
            <option v-for="member in members" :key="member.userId" :value="member.userId">
              {{ formatPersonName(member) }}
            </option>
          </template>
          <template v-else>
            <option
              v-for="option in field.options.filter((item) => item.deletedAt === null)"
              :key="option.id"
              :value="option.id"
            >
              {{ option.label }}
            </option>
          </template>
        </select>
        <input
          v-else
          :id="`${baseId}-value`"
          class="h-9 rounded-md border border-default bg-default px-3 text-sm"
          :type="inputType"
          step="any"
          :value="raw"
          @input="raw = ($event.target as HTMLInputElement).value"
        />
      </div>
      <UButton type="submit" size="sm" variant="outline" color="neutral" :disabled="!ready">
        {{ t("collection.filter.add") }}
      </UButton>
    </form>
  </div>
</template>
