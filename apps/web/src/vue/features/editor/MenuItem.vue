<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";

// One item of an editor menu. mousedown keeps the editor's selection (the
// command runs on click, react/menu-keyboard.ts preventSelectionLoss); a
// radio or checkbox item says whether it is on with aria-checked.
const props = withDefaults(
  defineProps<{ role?: "menuitem" | "menuitemradio" | "menuitemcheckbox"; checked?: boolean; label?: string }>(),
  { role: "menuitem", checked: false, label: undefined },
);
const emit = defineEmits<{ select: [] }>();
</script>

<template>
  <UButton
    :role="props.role"
    :aria-checked="props.role === 'menuitem' ? undefined : props.checked ? 'true' : 'false'"
    :aria-label="props.label"
    variant="ghost"
    color="neutral"
    size="sm"
    block
    class="fvoci-vue-menu__item justify-start"
    @mousedown.prevent
    @click="emit('select')"
  >
    <slot />
  </UButton>
</template>
