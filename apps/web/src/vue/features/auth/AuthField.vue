<script setup lang="ts">
import { computed, onMounted, useTemplateRef } from "vue";

// A labelled text input with its hint and error (role="alert"), wired by
// aria-describedby and aria-invalid. Other attributes (type, name,
// autocomplete, inputmode, maxlength, disabled, ...) go to the input.
// Uncontrolled, like the React forms' defaultValues / register(): the DOM
// holds the value so Korean IME composition is native.
defineOptions({ inheritAttrs: false });

const props = defineProps<{
  id: string;
  label: string;
  error?: string | null;
  hint?: string;
  /** Focus the input when it appears (the step replaced the control that had focus). */
  autofocus?: boolean;
}>();
const emit = defineEmits<{ input: [value: string] }>();
const input = useTemplateRef<HTMLInputElement>("input");

const errorId = computed(() => (props.error ? `${props.id}-error` : undefined));
const hintId = computed(() => (props.hint ? `${props.id}-hint` : undefined));
const describedBy = computed(
  () => [hintId.value, errorId.value].filter(Boolean).join(" ") || undefined,
);

onMounted(() => {
  if (props.autofocus) input.value?.focus();
});
</script>

<template>
  <div class="auth-shell__field">
    <label :for="id" class="auth-shell__label select-none">{{ label }}</label>
    <input
      :id="id"
      ref="input"
      v-bind="$attrs"
      class="auth-shell__input"
      :aria-describedby="describedBy"
      :aria-invalid="error ? true : undefined"
      @input="emit('input', ($event.target as HTMLInputElement).value)"
    />
    <p v-if="hint" :id="hintId" class="auth-shell__hint">{{ hint }}</p>
    <p v-if="error" :id="errorId" role="alert" class="auth-shell__alert">{{ error }}</p>
  </div>
</template>
