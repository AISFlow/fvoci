<script setup lang="ts">
import { computed } from "vue";
import { RouterLink } from "vue-router";
import { isVueAppPath } from "@/app-boundary";

// A link to an app path: a page of this app is an in-app navigation, any
// other page (the React app's) a plain anchor, so leaving is a full load.
const props = defineProps<{ to: string }>();
const inApp = computed(() => isVueAppPath(props.to.split(/[?#]/, 1)[0] ?? ""));
</script>

<template>
  <RouterLink v-if="inApp" :to="to"><slot /></RouterLink>
  <a v-else :href="to"><slot /></a>
</template>
