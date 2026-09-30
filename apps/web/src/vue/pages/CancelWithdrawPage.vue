<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, watchEffect } from "vue";
import { api, ensureOk } from "@/lib/api";
import { parseErasureHash } from "@/lib/erasure-hash";
import { setupStatusQuery } from "@/lib/queries";
import { redirectTo } from "../session/navigation";
import CancelWithdrawView from "../features/auth/CancelWithdrawView.vue";

const setup = useQuery(setupStatusQuery);
const fragment = parseErasureHash(window.location.hash);
const recoveryHref =
  fragment.scheduled && fragment.token
    ? `${window.location.origin}/cancel-withdraw${window.location.hash}`
    : null;
const leaving = computed(() => setup.data.value?.needed === true);

watchEffect(() => {
  if (setup.isLoading.value || setup.isError.value) return;
  if (setup.data.value?.needed) {
    redirectTo("/setup");
  }
});

async function onCancel(token: string): Promise<void> {
  await ensureOk(
    await api.POST("/api/v1/auth/cancel-withdraw", {
      body: { token },
    }),
  );
  // The spent token must not linger in history or a copied address.
  window.history.replaceState(null, "", "/cancel-withdraw");
}
</script>

<template>
  <p v-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <CancelWithdrawView
    v-else
    :token="fragment.token"
    :erase-at="fragment.eraseAt"
    :scheduled="fragment.scheduled"
    :mail-sent="fragment.mailSent"
    :recovery-href="recoveryHref"
    :submit-cancel="onCancel"
  />
</template>
