<script setup lang="ts">
import { t } from "@fvoci/i18n";
import type { OperatorInfo } from "@/features/legal/operator-fields";
import ServiceInfoFooter from "./ServiceInfoFooter.vue";
import "./auth-shell.css";

// The public pages' frame: the wordmark, the page's panel, and (with
// `footerOperator`, null included) the service info and legal links.
withDefaults(
  defineProps<{
    brandingName?: string | null;
    footerOperator?: OperatorInfo | null;
    width?: "narrow" | "wide";
    showWordmark?: boolean;
  }>(),
  { brandingName: undefined, footerOperator: undefined, width: "narrow", showWordmark: true },
);
</script>

<template>
  <div class="auth-shell">
    <main :class="width === 'wide' ? 'auth-shell__main auth-shell__main--wide' : 'auth-shell__main'">
      <div v-if="showWordmark" class="mb-8 flex flex-col items-center gap-2 text-center">
        <p class="auth-shell__wordmark break-keep">{{ brandingName ?? t("auth.wordmark") }}</p>
        <p class="auth-shell__tagline break-keep">{{ t("auth.tagline") }}</p>
      </div>
      <slot />
      <ServiceInfoFooter v-if="footerOperator !== undefined" :operator="footerOperator" />
    </main>
  </div>
</template>
