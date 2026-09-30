<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed, watchEffect } from "vue";
import { useRoute, useRouter } from "vue-router";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import ConsentView from "../features/auth/ConsentView.vue";
import AuthLayout from "../features/auth/AuthLayout.vue";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { safeReturnTo } from "@/lib/consent";

const router = useRouter();
const route = useRoute();
const returnTo = computed(() => {
  const value = route.query.returnTo;
  return safeReturnTo(
    typeof value === "string" ? value : Array.isArray(value) ? value[0] : null,
    window.location.origin,
  );
});
const pending = useQuery({
  queryKey: ["consents-pending"],
  queryFn: async () => (await ensureOk(await api.GET("/api/v1/auth/consents/pending"))).pending,
  retry: false,
  staleTime: 0,
});

const unauthorized = computed(() => {
  const err = pending.error.value;
  return err instanceof ProblemError && err.status === 401;
});

watchEffect(() => {
  if (unauthorized.value) {
    router.replace("/login").catch(() => {
      // Recover with a full load if the SPA navigation fails.
      window.location.replace("/login");
    });
    return;
  }
  // Nothing (left) to accept: continue where the gate interrupted.
  if (pending.data.value !== undefined && pending.data.value.length === 0) {
    window.location.assign(returnTo.value);
  }
});

async function onSubmit(items: { kind: string; version: number }[]): Promise<void> {
  await ensureOk(await api.POST("/api/v1/auth/consents", { body: { items } }));
  // A full load drops every query that failed on the gate.
  window.location.assign(returnTo.value);
}
</script>

<template>
  <p v-if="unauthorized" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <AuthLayout v-else-if="pending.isError.value">
    <QueryError :message="loadErrorMessage(pending.error.value)" @retry="pending.refetch()" />
  </AuthLayout>
  <AuthLayout v-else-if="!pending.data.value || pending.data.value.length === 0">
    <QueryLoading />
  </AuthLayout>
  <ConsentView v-else :pending="pending.data.value" :submit-consents="onSubmit" />
</template>
