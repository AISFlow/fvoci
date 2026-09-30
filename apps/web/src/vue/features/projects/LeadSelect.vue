<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import type { components } from "@/generated/api";

type Member = components["schemas"]["MemberResponse"];

defineProps<{
  id: string;
  modelValue: string;
  members: readonly Member[];
  disabled?: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [userId: string] }>();

function onChange(event: Event): void {
  const target = event.target;
  if (target instanceof HTMLSelectElement) emit("update:modelValue", target.value);
}
</script>

<template>
  <div class="project-form__field">
    <label :for="id">{{ t("project.lead") }}</label>
    <select
      :id="id"
      :value="modelValue"
      :disabled="disabled || members.length === 0"
      @change="onChange"
    >
      <option value="">{{ t("project.lead.none") }}</option>
      <option v-for="member in members" :key="member.userId" :value="member.userId">
        {{ formatPersonName(member) }}
      </option>
    </select>
  </div>
</template>
