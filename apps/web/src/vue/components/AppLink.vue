<script setup lang="ts">
import { computed } from "vue";
import { RouterLink } from "vue-router";
import { isLocalAppPath as isVueAppPath } from "@/vue/route-paths";

// Local SPA paths use router navigation; external targets remain anchors.
const props = defineProps<{ to: string }>();
const inApp = computed(() => isVueAppPath(props.to.split(/[?#]/, 1)[0] ?? ""));
</script>

<template>
  <RouterLink v-if="inApp" :to="to"><slot /></RouterLink>
  <a v-else :href="to"><slot /></a>
</template>
