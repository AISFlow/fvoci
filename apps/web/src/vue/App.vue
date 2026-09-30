<script setup lang="ts">
import UApp from "@nuxt/ui/components/App.vue";
import { ko } from "@nuxt/ui/locale";
import { RouterView, useRoute } from "vue-router";
import { useQuery } from "@tanstack/vue-query";
import { computed, watch } from "vue";
import { meQuery } from "@/lib/queries";
import { applyTextScale } from "@/lib/ui-preferences";

const route = useRoute();
// Public policies and auth/token links remain readable without a gated me request.
const privatePage = computed(
  () => route.matched.length > 0 && /^(?:\/$|\/w\/|\/settings(?:\/|$))/i.test(route.path),
);
const me = useQuery(() => ({ ...meQuery, enabled: privatePage.value }));
watch(
  () => me.data.value?.textScale,
  (scale) => {
    if (scale !== undefined) applyTextScale(scale);
  },
  { immediate: true },
);
</script>

<template>
  <!-- No toaster: errors are shown in place (role="alert"). -->
  <UApp :locale="ko" :toaster="null">
    <RouterView />
  </UApp>
</template>
