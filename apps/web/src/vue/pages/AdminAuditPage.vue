<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { ProblemError } from "@/lib/api";
import { FALLBACK_TZ } from "@/lib/datetime";
import { meQuery } from "@/lib/queries";
import { adminAuditQuery } from "@/lib/queries/admin";
import AdminShell from "../components/AdminShell.vue";
import AuditSettingsView from "../features/settings/AuditSettingsView.vue";

const me = useQuery(meQuery);
const audit = useQuery(() => ({
  ...adminAuditQuery,
  enabled: me.data.value?.isInstanceAdmin === true,
}));
const eeRequired = computed(
  () => audit.error.value instanceof ProblemError && audit.error.value.status === 404,
);
const failure = computed(() => {
  if (eeRequired.value || !audit.error.value) return null;
  return audit.error.value instanceof ProblemError ? audit.error.value.title : t("error.network");
});
</script>

<template>
  <AdminShell active="audit">
    <AuditSettingsView
      :items="audit.data.value?.items ?? []"
      :time-zone="me.data.value?.timezone ?? FALLBACK_TZ"
      :loading="audit.isLoading.value"
      :ee-required="eeRequired"
      :error="failure"
    />
  </AdminShell>
</template>
