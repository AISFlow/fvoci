<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref, watch, watchEffect } from "vue";
import { useRoute } from "vue-router";
import { api, ensureOk } from "@/lib/api";
import { setupStatusQuery } from "@/lib/queries";
import { redirectTo } from "../session/navigation";
import MagicLinkView from "../features/auth/MagicLinkView.vue";
import MfaStep from "../features/auth/MfaStep.vue";

const route = useRoute();
const setup = useQuery(setupStatusQuery);
const token = computed(() => {
  const value = route.query.token;
  return typeof value === "string" && value.length > 0 ? value : null;
});
const mfaToken = ref<string | null>(null);
watch(token, () => {
  mfaToken.value = null;
});
const leaving = computed(() => setup.data.value?.needed === true);

watchEffect(() => {
  if (setup.isLoading.value || setup.isError.value) return;
  if (setup.data.value?.needed) {
    redirectTo("/setup");
  }
});

async function enterApp(): Promise<void> {
  // A full load clears anonymous queries after sign-in.
  window.location.replace("/");
}

function leaveToLogin(): void {
  window.location.assign("/login");
}

async function onConsume(value: string): Promise<void> {
  const result = await ensureOk(
    await api.POST("/api/v1/auth/magic-link/consume", {
      body: { token: value },
    }),
  );
  // A response for a link we have left must not restore its MFA challenge.
  if (token.value !== value) return;
  if (result.mfaToken) {
    mfaToken.value = result.mfaToken;
    return;
  }
  await enterApp();
}
</script>

<template>
  <p v-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{
    t("load.loading")
  }}</p>
  <div v-else-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <MfaStep
    v-else-if="mfaToken !== null"
    :mfa-token="mfaToken"
    @back="leaveToLogin"
    @verified="enterApp"
  />
  <MagicLinkView v-else :key="token ?? ''" :token="token" :consume-token="onConsume" />
</template>
