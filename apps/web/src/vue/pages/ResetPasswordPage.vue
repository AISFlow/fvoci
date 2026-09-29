<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, watchEffect } from "vue";
import { useRoute, useRouter } from "vue-router";
import { api, ensureOk } from "@/lib/api";
import { setupStatusQuery } from "@/lib/queries";
import { redirectTo } from "../session/navigation";
import ResetPasswordView from "../features/auth/ResetPasswordView.vue";

// /reset-password?token=: the confirm form. Boot still sends this path to
// React until apps/web/src/app-boundary.ts includes:
//   /^\/reset-password\/?$/i
// Pair that with VUE_ROUTE_PATHS.resetPassword = "/reset-password".

const route = useRoute();
const router = useRouter();
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

async function onConfirm(newPassword: string): Promise<void> {
  const value = token.value;
  if (!value) return;
  await ensureOk(
    await api.POST("/api/v1/auth/password-reset/confirm", {
      body: { token: value, newPassword },
    }),
  );
  await router.replace("/login?reset=1");
}
</script>

<template>
  <p v-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <ResetPasswordView v-else :token="token" :submit-confirm="onConfirm" />
</template>
