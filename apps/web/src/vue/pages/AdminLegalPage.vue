<script setup lang="ts">
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { publishLegalDocument } from "@/features/settings/admin-requests";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { legalDocQuery } from "@/lib/queries/admin";
import { meQuery } from "@/lib/queries";
import AdminShell from "../components/AdminShell.vue";
import SettingsLegalView from "../features/settings/SettingsLegalView.vue";

const queryClient = useQueryClient();
const kind = ref("terms");
const me = useQuery(meQuery);
const current = useQuery(() => ({
  ...legalDocQuery(kind.value),
  enabled: me.data.value?.isInstanceAdmin === true && /^[a-z0-9-]{1,50}$/.test(kind.value),
}));
const none = computed(
  () => current.error.value instanceof ProblemError && current.error.value.status === 404,
);
const failed = computed(() => current.isError.value && !none.value);
</script>

<template>
  <AdminShell active="legal">
    <SettingsLegalView
      :kind="kind"
      :on-kind-change="(next) => (kind = next)"
      :current="current.isError.value ? null : (current.data.value ?? null)"
      :loading="current.isLoading.value"
      :error="failed ? loadErrorMessage(current.error.value) : null"
      :on-retry="() => void current.refetch()"
      :on-publish="(input) => publishLegalDocument(queryClient, input)"
    />
  </AdminShell>
</template>
