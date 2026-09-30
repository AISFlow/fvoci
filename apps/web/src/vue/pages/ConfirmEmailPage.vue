<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, watchEffect } from "vue";
import { useRoute } from "vue-router";
import { api, ensureOk } from "@/lib/api";
import { setupStatusQuery } from "@/lib/queries";
import { redirectTo } from "../session/navigation";
import ConfirmEmailView from "../features/auth/ConfirmEmailView.vue";

const route = useRoute();
const setup = useQuery(setupStatusQuery);
const token = computed(() => {
  const value = route.query.token;
  return typeof value === "string" && value.length > 0 ? value : null;
});
const leaving = computed(() => setup.data.value?.needed === true);

watchEffect(() => {
  if (setup.isLoading.value || setup.isError.value) return;
  if (setup.data.value?.needed) {
    redirectTo("/setup");
  }
});

async function onConfirm(value: string): Promise<void> {
  await ensureOk(
    await api.POST("/api/v1/auth/email/confirm", {
      body: { token: value },
    }),
  );
  // Account settings is the React app: a full load.
  window.location.replace("/settings/account?email_changed=1");
}
</script>

<template>
  <p v-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <ConfirmEmailView v-else :key="token ?? ''" :token="token" :confirm-email="onConfirm" />
</template>
