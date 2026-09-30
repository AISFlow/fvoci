<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UPageCard from "@nuxt/ui/components/PageCard.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import type { components } from "@/generated/api";
import { formatInstant } from "@/lib/datetime";
import { tableCellClass } from "./field-classes";
import "@/features/settings/settings-shell.css";

type AuditLogItem = components["schemas"]["AuditLogItemOutput"];

defineProps<{
  items: AuditLogItem[];
  timeZone: string;
  loading: boolean;
  eeRequired: boolean;
  error: string | null;
}>();
</script>

<template>
  <div class="settings-stack">
    <UPageCard as="section" variant="subtle" class="settings-section" aria-labelledby="audit-title">
      <h2 id="audit-title" class="settings-section__title text-title">{{ t("audit.title") }}</h2>
      <div class="flex flex-col gap-4">
        <p v-if="eeRequired" class="text-sm text-muted">{{ t("ee.required") }}</p>
        <p v-if="error" class="text-sm text-error" role="alert">{{ error }}</p>
        <QueryLoading v-if="loading" />
        <p
          v-if="!eeRequired && !loading && !error && items.length === 0"
          class="text-sm text-muted"
        >
          {{ t("audit.empty") }}
        </p>
        <div v-if="!eeRequired && !loading && items.length > 0" class="overflow-x-auto">
          <table class="w-full border-collapse">
            <tbody>
              <tr v-for="row in items" :key="row.id">
                <td :class="[tableCellClass, 'font-mono']">{{ row.verb }}</td>
                <td :class="[tableCellClass, 'settings-tabular']">
                  {{
                    formatInstant(row.createdAt, timeZone, {
                      year: "numeric",
                      month: "2-digit",
                      day: "2-digit",
                      hour: "2-digit",
                      minute: "2-digit",
                    })
                  }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </UPageCard>
  </div>
</template>
