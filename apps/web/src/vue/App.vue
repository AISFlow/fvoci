<script setup lang="ts">
import UApp from "@nuxt/ui/components/App.vue";
import { ko } from "@nuxt/ui/locale";
import { RouterView } from "vue-router";
</script>

<template>
  <!-- No toaster: errors are shown in place (role="alert"). -->
  <UApp :locale="ko" :toaster="null">
    <RouterView />
  </UApp>
</template>
