<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { api, ensureOk } from "@/lib/api";
import { copyText } from "./clipboard";
import { mergeHolidayItems } from "./holidays";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string }>();
const client = useQueryClient();
const date = ref("");
const icsUrl = ref<string | null>(null);
const copyBusy = ref(false);
const writeBusy = ref(false);

const holidays = useQuery(() => ({
  queryKey: ["workspaces", props.workspaceId, "holidays"] as const,
  queryFn: async () =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/holidays", {
        params: { path: { workspace_id: props.workspaceId } },
      }),
    ),
  retry: false as const,
}));

const canEdit = computed(() => holidays.data.value?.canEdit);
const items = computed(() => holidays.data.value?.items ?? []);

const copy = useMutation({
  mutationFn: async () => {
    icsUrl.value ??= (
      await ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/ics-token", {
          params: { path: { workspace_id: props.workspaceId } },
        }),
      )
    ).url;
    await copyText(icsUrl.value);
  },
  onSettled: () => {
    copyBusy.value = false;
  },
});

const write = useMutation({
  mutationFn: async (input: { date: string; remove: boolean }) =>
    input.remove
      ? ensureOk(
          await api.DELETE("/api/v1/workspaces/{workspace_id}/holidays/{date}", {
            params: { path: { workspace_id: props.workspaceId, date: input.date } },
          }),
        )
      : ensureOk(
          await api.POST("/api/v1/workspaces/{workspace_id}/holidays", {
            params: { path: { workspace_id: props.workspaceId } },
            body: { date: input.date },
          }),
        ),
  onSuccess: async (_result, input) => {
    client.setQueryData(
      ["workspaces", props.workspaceId, "holidays"],
      (current: { canEdit?: boolean; items?: string[] } | undefined) => ({
        canEdit: current?.canEdit ?? false,
        items: mergeHolidayItems(current?.items, input.date, input.remove),
      }),
    );
    date.value = "";
    await client.invalidateQueries({ queryKey: ["workspaces", props.workspaceId, "holidays"] });
  },
  onSettled: () => {
    writeBusy.value = false;
  },
});

const writesDisabled = computed(
  () => holidays.isPending.value || holidays.isError.value || write.isPending.value,
);

function changeHoliday(input: { date: string; remove: boolean }): void {
  if (writesDisabled.value || writeBusy.value) return;
  writeBusy.value = true;
  write.mutate(input);
}

function copyFeed(): void {
  if (copyBusy.value) return;
  copyBusy.value = true;
  copy.mutate();
}

function addHoliday(): void {
  if (date.value) changeHoliday({ date: date.value, remove: false });
}
</script>

<template>
  <section class="settings-section">
    <details class="rounded-md border border-default p-3">
      <summary class="min-h-11 cursor-pointer"
        >{{ t("ics.subscribe") }} · {{ t("ics.holidays") }}</summary
      >
      <div class="mt-2 flex flex-col gap-2">
        <UButton type="button" :disabled="copy.isPending.value" @click="copyFeed">
          {{ t(copy.isSuccess.value ? "ics.subscribe.copied" : "ics.subscribe.copy") }}
        </UButton>
        <p v-if="copy.isError.value" role="alert">{{ t("ics.subscribe.failed") }}</p>
        <p v-if="holidays.isPending.value" role="status">{{ t("load.loading") }}</p>
        <p v-else-if="items.length === 0">{{ t("ics.holidays.empty") }}</p>
        <ul v-else>
          <li v-for="day in items" :key="day" class="flex items-center justify-between gap-2">
            <time>{{ day }}</time>
            <UButton
              v-if="canEdit"
              type="button"
              variant="outline"
              color="neutral"
              :disabled="writesDisabled"
              @click="changeHoliday({ date: day, remove: true })"
            >
              {{ day }} {{ t("ics.holidays.remove") }}
            </UButton>
          </li>
        </ul>
        <form v-if="canEdit" class="flex flex-wrap gap-2" @submit.prevent="addHoliday">
          <UInput v-model="date" type="date" :aria-label="t('ics.holidays.date')" />
          <UButton type="submit" :disabled="!date || writesDisabled">{{
            t("ics.holidays.add")
          }}</UButton>
        </form>
        <div v-if="holidays.isError.value" role="alert">
          <p>{{
            t(write.isSuccess.value ? "ics.holidays.refreshFailed" : "ics.holidays.loadFailed")
          }}</p>
          <UButton type="button" @click="holidays.refetch()">{{ t("load.retry") }}</UButton>
        </div>
        <p v-if="write.isSuccess.value" role="status">{{ t("ics.holidays.saved") }}</p>
        <p v-if="write.isError.value" role="alert">{{ t("ics.holidays.writeFailed") }}</p>
      </div>
    </details>
  </section>
</template>
