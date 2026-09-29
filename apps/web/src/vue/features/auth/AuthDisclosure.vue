<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";

defineProps<{ trigger: string; open: boolean }>();
const emit = defineEmits<{ "update:open": [open: boolean] }>();
</script>

<template>
  <UButton
    v-if="!open"
    type="button"
    variant="link"
    class="auth-shell__link-button"
    @click="emit('update:open', true)"
  >
    {{ trigger }}
  </UButton>
  <div v-else class="auth-shell__stack">
    <slot />
  </div>
</template>
