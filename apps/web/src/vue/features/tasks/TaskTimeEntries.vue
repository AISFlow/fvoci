<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { taskTimeEntriesQuery } from "@/features/tasks/queries";
import { formatDuration } from "@/features/tasks/time-entry-format";
import type { components } from "@/generated/api";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import type { MemberOutput } from "@/lib/contracts";
import { datetimeLocalInTimeZoneToIso, durationSecondsBetween, FALLBACK_TZ, formatInstant } from "@/lib/datetime";
import { meQuery } from "@/lib/queries";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";

type TimeEntry = components["schemas"]["TimeEntryOutput"];
type TimeEntryCreateBody = components["schemas"]["TimeEntryCreateBody"];

const props = defineProps<{
  workspaceId: string;
  taskId: string;
  members: readonly MemberOutput[];
  readOnly: boolean;
}>();

const queryClient = useQueryClient();
const list = useQuery(() => taskTimeEntriesQuery(props.workspaceId, props.taskId));
const me = useQuery(meQuery);
const timeZone = computed(() => me.data.value?.timezone ?? FALLBACK_TZ);
const formOpen = ref(false);
const formError = ref<string | null>(null);
const startedLocal = ref("");
const endedLocal = ref("");
const note = ref("");
const localError = ref<string | null>(null);
const shownError = computed(() => localError.value ?? formError.value);

const items = computed<TimeEntry[]>(() => list.data.value?.items ?? []);
const canCreate = computed(
  () => !props.readOnly && list.isSuccess.value && Boolean(list.data.value?.canCreate),
);
const total = computed(() => items.value.reduce((sum, row) => sum + (row.durationSeconds ?? 0), 0));
const showHeading = computed(() => !list.isSuccess.value || items.value.length > 0 || formOpen.value);

const create = useMutation({
  mutationFn: async (body: TimeEntryCreateBody) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries", {
        params: { path: { workspace_id: props.workspaceId, task_id: props.taskId } },
        body,
      }),
    ),
  onSuccess: async () => {
    formError.value = null;
    formOpen.value = false;
    startedLocal.value = "";
    endedLocal.value = "";
    note.value = "";
    await queryClient.invalidateQueries({ queryKey: taskTimeEntriesQuery(props.workspaceId, props.taskId).queryKey });
  },
  onError: (err) => {
    formError.value = err instanceof ProblemError ? err.title : t("load.failed");
  },
});

function memberName(userId: string): string {
  const member = props.members.find((m) => m.userId === userId);
  return member ? formatPersonName(member) : userId.slice(0, 8);
}

function openForm(): void {
  formOpen.value = true;
}

function cancelForm(): void {
  formOpen.value = false;
  formError.value = null;
  localError.value = null;
}

async function onSubmit(): Promise<void> {
  if (startedLocal.value === "" && endedLocal.value === "") return;
  if (startedLocal.value === "" || endedLocal.value === "") {
    localError.value = t("task.time.duration.needRange");
    return;
  }
  const startedAt = datetimeLocalInTimeZoneToIso(startedLocal.value, timeZone.value);
  const endedAt = datetimeLocalInTimeZoneToIso(endedLocal.value, timeZone.value);
  const span = durationSecondsBetween(startedAt, endedAt);
  if (startedAt === "" || endedAt === "" || !Number.isFinite(span) || span <= 0) {
    localError.value = t("task.time.duration.invalid");
    return;
  }
  const body: TimeEntryCreateBody = { startedAt, endedAt };
  const trimmed = note.value.trim();
  if (trimmed !== "") body.note = trimmed;
  localError.value = null;
  try {
    await create.mutateAsync(body);
  } catch {
    /* onError keeps the form open with the problem title. */
  }
}
</script>

<template>
  <section class="flex flex-col gap-2 text-sm" data-testid="task-time-entries">
    <h2 v-if="showHeading" class="text-sm font-medium">{{ t("task.time.heading") }}</h2>
    <QueryLoading v-if="list.isLoading.value" />
    <QueryError
      v-if="list.isError.value"
      :message="loadErrorMessage(list.error.value)"
      @retry="list.refetch()"
    />
    <div v-if="items.length > 0" class="flex flex-col gap-1.5">
      <p class="break-keep text-sm" data-testid="task-time-total">
        {{ t("task.time.total") }} {{ formatDuration(total) }}
      </p>
      <ul class="flex flex-col gap-1">
        <li v-for="row in items" :key="row.id" class="break-keep text-sm">
          {{ memberName(row.userId) }}
          {{
            formatInstant(row.startedAt, timeZone, {
              month: "numeric",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
            })
          }}
          {{ row.durationSeconds == null ? t("task.time.open") : formatDuration(row.durationSeconds) }}{{
            row.note ? ` · ${row.note}` : ""
          }}
        </li>
      </ul>
    </div>
    <UButton
      v-if="canCreate && !formOpen"
      type="button"
      size="sm"
      variant="outline"
      color="neutral"
      class="self-start"
      :aria-expanded="false"
      @click="openForm"
    >
      {{ t("task.time.submit") }}
    </UButton>
    <form v-if="canCreate && formOpen" class="flex flex-col gap-2 sm:max-w-lg" novalidate @submit.prevent="onSubmit">
      <div class="grid gap-2 sm:grid-cols-2">
        <div class="flex flex-col gap-1">
          <label class="text-sm text-muted" for="task-time-started">{{ t("task.time.startedAt") }}</label>
          <input
            id="task-time-started"
            v-model="startedLocal"
            type="datetime-local"
            class="h-10 rounded-md border border-default bg-default px-3 text-sm"
          />
        </div>
        <div class="flex flex-col gap-1">
          <label class="text-sm text-muted" for="task-time-ended">{{ t("task.time.endedAt") }}</label>
          <input
            id="task-time-ended"
            v-model="endedLocal"
            type="datetime-local"
            class="h-10 rounded-md border border-default bg-default px-3 text-sm"
          />
        </div>
      </div>
      <div class="flex flex-col gap-1">
        <label class="text-sm text-muted" for="task-time-note">{{ t("task.time.note") }}</label>
        <textarea
          id="task-time-note"
          v-model="note"
          class="min-h-16 rounded-md border border-default bg-default px-3 py-2 text-sm"
          maxlength="2000"
        />
      </div>
      <p v-if="shownError" role="alert" class="break-keep text-sm text-error">{{ shownError }}</p>
      <div class="flex flex-wrap gap-2">
        <UButton type="submit" size="sm" :disabled="create.isPending.value">{{ t("task.time.submit") }}</UButton>
        <UButton type="button" size="sm" variant="outline" color="neutral" @click="cancelForm">
          {{ t("task.create.cancel") }}
        </UButton>
      </div>
    </form>
  </section>
</template>
