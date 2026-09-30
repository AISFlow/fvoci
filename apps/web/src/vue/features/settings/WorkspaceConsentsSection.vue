<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import type { components } from "@/generated/api";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import { FALLBACK_TZ, formatInstant } from "@/lib/datetime";
import { membersQuery, meQuery } from "@/lib/queries";

const props = defineProps<{ workspaceId: string }>();
const me = useQuery(meQuery);
const members = useQuery(() => membersQuery(props.workspaceId));
const consents = useQuery(() => ({
  queryKey: ["workspaces", props.workspaceId, "consents"],
  queryFn: async (): Promise<components["schemas"]["WorkspaceConsentsResponse"]> =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/consents", {
        params: { path: { workspace_id: props.workspaceId } },
      }),
    ),
  retry: false,
}));
const rows = computed(() =>
  (consents.data.value?.members ?? []).flatMap((member) => {
    const person = members.data.value?.items.find((item) => item.userId === member.userId);
    const name = person ? formatPersonName(person) || member.userId : member.userId;
    return member.consents.map((consent) => ({ ...consent, userId: member.userId, name }));
  }),
);
const timeZone = computed(() => me.data.value?.timezone ?? FALLBACK_TZ);

function consentDate(value: string): string {
  return formatInstant(value, timeZone.value, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}
</script>

<template>
  <details class="settings-disclosure" data-testid="workspace-consents">
    <summary class="settings-disclosure__summary">{{ t("workspace.consents") }}</summary>
    <div class="settings-disclosure__body">
      <p v-if="consents.isLoading.value" role="status" class="text-muted">{{
        t("load.loading")
      }}</p>
      <div v-if="consents.isError.value" class="flex flex-col items-start gap-2">
        <p role="alert" class="text-error">{{ loadErrorMessage(consents.error.value) }}</p>
        <UButton type="button" variant="outline" color="neutral" @click="consents.refetch()">{{
          t("load.retry")
        }}</UButton>
      </div>
      <p
        v-if="!consents.isLoading.value && !consents.isError.value && rows.length === 0"
        class="text-muted"
      >
        {{ t("workspace.consents.empty") }}
      </p>
      <div v-if="rows.length > 0" class="overflow-x-auto">
        <table class="w-full border-collapse">
          <thead>
            <tr>
              <th scope="col">{{ t("workspace.consents.member") }}</th>
              <th scope="col">{{ t("workspace.consents.kind") }}</th>
              <th scope="col">{{ t("workspace.consents.version") }}</th>
              <th scope="col">{{ t("workspace.consents.at") }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in rows" :key="`${row.userId}:${row.kind}:${row.version}`">
              <td class="border-b border-default px-2 py-2">{{ row.name }}</td>
              <td class="border-b border-default px-2 py-2">{{ row.kind }}</td>
              <td class="border-b border-default px-2 py-2">{{ row.version }}</td>
              <td class="border-b border-default px-2 py-2">{{ consentDate(row.consentedAt) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </details>
</template>
