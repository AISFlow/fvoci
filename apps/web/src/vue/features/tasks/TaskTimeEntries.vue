<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, reactive, ref, useId, watch } from "vue";
import { taskTimeEntriesQuery } from "@/features/tasks/queries";
import { formatDuration } from "@/features/tasks/time-entry-format";
import type { components } from "@/generated/api";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import type { MemberOutput } from "@/lib/contracts";
import { FALLBACK_TZ, formatInstant } from "@/lib/datetime";
import { meQuery } from "@/lib/queries";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import { timerCalendarRange } from "./task-stopwatch-calendar";
import { stopwatchText } from "./task-stopwatch-clock";
import {
  correctionFromDraft,
  manualFromDraft,
  openRecordEditor,
  type RecordDraft,
  type RecordEditor,
} from "./task-stopwatch-record-editor";
import {
  captureTimerDenial,
  captureTimerQuery,
  removeCapturedTimerQuery,
  ownerStopwatchQuery,
  personalTimerHistoryQuery,
  personalTimerSummaryQuery,
  sendLegacyRelease,
  sendPersonalCorrection,
  sendPersonalManual,
  taskStopwatchQuery,
  timerContextChanged,
  timerHistoryChanged,
  type TimerHistoryScope,
  type TimerRecord,
  type TimerManual,
  type TimerCorrection,
  type TimerLegacyRelease,
} from "./task-stopwatch-queries";

type TimeEntry = components["schemas"]["TimeEntryOutput"];
const props = defineProps<{
  workspaceId: string;
  taskId: string;
  members: readonly MemberOutput[];
  readOnly: boolean;
}>();
const client = useQueryClient();
const me = useQuery(meQuery);
const actor = computed(() => me.data.value?.userId ?? "");
const session = computed(() => me.data.value?.sessionId ?? "");
const timeZone = computed(() => me.data.value?.timezone ?? FALLBACK_TZ);
// Keep the ordinary shared-row contract and author labels. Its optional captured
// GET guard requires the separately authorized original A/B negative and seam.
const list = useQuery(() => taskTimeEntriesQuery(props.workspaceId, props.taskId));
const timer = useQuery(() =>
  taskStopwatchQuery(actor.value, props.workspaceId, props.taskId, session.value),
);
const calendar = computed(() =>
  timer.data.value
    ? timerCalendarRange(
        timer.data.value.serverNow,
        timeZone.value,
        me.data.value?.weekStartsOn === 0 ? 0 : 1,
      )
    : undefined,
);
const fromDate = ref("");
const toDate = ref("");
function rangeScope(from: string, to: string): TimerHistoryScope {
  return {
    actor: actor.value,
    session: session.value,
    workspace: props.workspaceId,
    task: props.taskId,
    timeZone: timeZone.value,
    from,
    to,
  };
}
const historyScope = computed(() =>
  rangeScope(
    fromDate.value || calendar.value?.weekFrom || "",
    toDate.value || calendar.value?.weekTo || "",
  ),
);
const dayScope = computed(() =>
  rangeScope(calendar.value?.today || "", calendar.value?.today || ""),
);
const weekScope = computed(() =>
  rangeScope(calendar.value?.weekFrom || "", calendar.value?.weekTo || ""),
);
const history = useInfiniteQuery(() => personalTimerHistoryQuery(historyScope.value));
const today = useQuery(() => personalTimerSummaryQuery(dayScope.value));
const week = useQuery(() => personalTimerSummaryQuery(weekScope.value));
const revoked = ref(false);
let revokedAt = 0;
const historyChanged = ref(false);
const personalError = ref<unknown>();
const records = computed(() =>
  revoked.value || historyChanged.value
    ? []
    : (history.data.value?.pages.flatMap((page) => page.items) ?? []),
);
const items = computed<TimeEntry[]>(() => list.data.value?.items ?? []);
const total = computed(() => items.value.reduce((sum, row) => sum + (row.durationSeconds ?? 0), 0));
const canEdit = computed(
  () =>
    !props.readOnly &&
    !revoked.value &&
    timer.data.value?.canControl === true &&
    list.data.value?.canCreate === true,
);
const formOpen = ref(false);
const editor = ref<RecordEditor>();
const draft = reactive<RecordDraft>({ startedLocal: "", endedLocal: "", note: "", reason: "" });
const formError = ref<string>();
const conflict = ref(false);
const saved = ref(false);
const pending = ref(false);
const fieldId = useId();
let live = true;
let lifetime = 0;
let draftVersion = 0;
let pendingCommand: Mutation | undefined;
const replay = ref<Mutation>();
type Scope = {
  actor: string;
  session: string;
  workspace: string;
  task: string;
  timeZone: string;
  lifetime: number;
};
type Mutation = { scope: Scope; draftVersion: number } & (
  | { operation: "manual"; body: Readonly<TimerManual> }
  | { operation: "correct"; record: string; body: Readonly<TimerCorrection> }
  | { operation: "release"; body: Readonly<TimerLegacyRelease> }
);
function captureScope(): Scope {
  return Object.freeze({
    actor: actor.value,
    session: session.value,
    workspace: props.workspaceId,
    task: props.taskId,
    timeZone: timeZone.value,
    lifetime,
  });
}
function current(scope: Scope): boolean {
  return (
    live &&
    lifetime === scope.lifetime &&
    actor.value === scope.actor &&
    session.value === scope.session &&
    props.workspaceId === scope.workspace &&
    props.taskId === scope.task &&
    timeZone.value === scope.timeZone
  );
}
function clearDraft(): void {
  draftVersion++;
  editor.value = undefined;
  Object.assign(draft, { startedLocal: "", endedLocal: "", note: "", reason: "" });
  formOpen.value = false;
  formError.value = undefined;
  conflict.value = false;
  replay.value = undefined;
}
watch(
  () => [draft.startedLocal, draft.endedLocal, draft.note, draft.reason],
  () => {
    draftVersion++;
    saved.value = false;
  },
  { flush: "sync" },
);
watch(
  [actor, session, () => props.workspaceId, () => props.taskId, timeZone],
  () => {
    lifetime++;
    pendingCommand = undefined;
    pending.value = false;
    revoked.value = false;
    personalError.value = undefined;
    historyChanged.value = false;
    saved.value = false;
    fromDate.value = "";
    toDate.value = "";
    clearDraft();
  },
  { flush: "sync" },
);
watch(
  canEdit,
  (allowed) => {
    if (!allowed) {
      lifetime++;
      pendingCommand = undefined;
      pending.value = false;
      clearDraft();
    }
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  live = false;
  lifetime++;
  pendingCommand = undefined;
});
function denied(error: unknown): error is ProblemError {
  return (
    (error instanceof ProblemError && [401, 403, 404].includes(error.status)) ||
    timerContextChanged(error)
  );
}
async function retirePersonal(error: ProblemError): Promise<void> {
  if (revoked.value) return;
  const captures = [
    taskStopwatchQuery(actor.value, props.workspaceId, props.taskId, session.value).queryKey,
    personalTimerHistoryQuery(historyScope.value).queryKey,
    personalTimerSummaryQuery(dayScope.value).queryKey,
    personalTimerSummaryQuery(weekScope.value).queryKey,
  ].map((key) => captureTimerQuery(client, key));
  revokedAt = timer.dataUpdatedAt.value;
  revoked.value = true;
  personalError.value = error;
  clearDraft();
  // Revocation also retires mutation authority. Capture its resulting lifetime
  // after the synchronous capability watcher, retaining the original queries.
  const scope = captureScope();
  await Promise.all(
    captures.map((capture) => removeCapturedTimerQuery(client, capture, () => current(scope))),
  );
  if (timerContextChanged(error) && current(scope))
    await client.invalidateQueries({ queryKey: meQuery.queryKey, exact: true });
}
function watchPrivateRead(
  result: typeof timer | typeof today | typeof history,
  key: () => readonly string[],
): void {
  watch(
    [
      () => result.error.value,
      () => result.dataUpdatedAt.value,
      () => result.status.value,
      () => result.fetchStatus.value,
    ],
    async () => {
      const error = result.error.value;
      if (!denied(error)) return;
      const capture = captureTimerDenial(
        client,
        error,
        key(),
        result.status.value,
        result.fetchStatus.value,
      );
      if (!capture) return;
      await retirePersonal(error);
    },
    { flush: "pre" },
  );
}
watchPrivateRead(
  timer,
  () => taskStopwatchQuery(actor.value, props.workspaceId, props.taskId, session.value).queryKey,
);
watchPrivateRead(history, () => personalTimerHistoryQuery(historyScope.value).queryKey);
watchPrivateRead(today, () => personalTimerSummaryQuery(dayScope.value).queryKey);
watchPrivateRead(week, () => personalTimerSummaryQuery(weekScope.value).queryKey);
watch(
  [() => timer.dataUpdatedAt.value, () => timer.status.value, () => timer.fetchStatus.value],
  async () => {
    if (
      revoked.value &&
      timer.isSuccess.value &&
      timer.fetchStatus.value === "idle" &&
      timer.dataUpdatedAt.value > revokedAt
    ) {
      revoked.value = false;
      personalError.value = undefined;
      await refresh(captureScope());
    }
  },
  { flush: "pre" },
);
watch(
  () => personalTimerHistoryQuery(historyScope.value).queryKey.join("|"),
  () => {
    historyChanged.value = false;
  },
  { flush: "sync" },
);
watch(
  () => [history.error.value, history.status.value, history.fetchStatus.value],
  () => {
    const error = history.error.value;
    if (
      timerHistoryChanged(error) &&
      captureTimerDenial(
        client,
        error,
        personalTimerHistoryQuery(historyScope.value).queryKey,
        history.status.value,
        history.fetchStatus.value,
      )
    )
      historyChanged.value = true;
  },
  { flush: "pre" },
);
async function reloadHistory(): Promise<void> {
  const scope = captureScope();
  const capture = captureTimerQuery(client, personalTimerHistoryQuery(historyScope.value).queryKey);
  await client.resetQueries({
    queryKey: capture.queryKey,
    exact: true,
    predicate: (query) => query === capture.query,
  });
  if (
    current(scope) &&
    client.getQueryCache().find({ queryKey: capture.queryKey, exact: true }) === capture.query &&
    history.isSuccess.value
  )
    historyChanged.value = false;
}
async function refresh(scope: Scope): Promise<void> {
  if (!current(scope)) return;
  await Promise.all([
    reloadHistory(),
    client.invalidateQueries({
      queryKey: personalTimerSummaryQuery(dayScope.value).queryKey,
      exact: true,
    }),
    client.invalidateQueries({
      queryKey: personalTimerSummaryQuery(weekScope.value).queryKey,
      exact: true,
    }),
    client.invalidateQueries({
      queryKey: taskStopwatchQuery(scope.actor, scope.workspace, scope.task, scope.session)
        .queryKey,
      exact: true,
    }),
    client.invalidateQueries({
      queryKey: ownerStopwatchQuery(scope.actor, scope.session).queryKey,
      exact: true,
    }),
    client.invalidateQueries({
      queryKey: taskTimeEntriesQuery(scope.workspace, scope.task).queryKey,
      exact: true,
    }),
  ]);
}
function openManual(): void {
  clearDraft();
  saved.value = false;
  formOpen.value = true;
}
function openCorrection(record: TimerRecord): void {
  clearDraft();
  saved.value = false;
  editor.value = openRecordEditor(record, timeZone.value);
  Object.assign(draft, {
    startedLocal: editor.value.initialStart,
    endedLocal: editor.value.initialEnd,
    note: editor.value.initialNote,
    reason: "",
  });
  formOpen.value = true;
}
const currentConflictRecord = computed(() =>
  editor.value
    ? records.value.find(
        (row) => row.id === editor.value?.record.id && row.kind === editor.value.record.kind,
      )
    : undefined,
);
function applyCurrentRecord(): void {
  const row = currentConflictRecord.value;
  const old = editor.value;
  if (!row || !old) return;
  const next = openRecordEditor(row, timeZone.value);
  if (draft.startedLocal === old.initialStart) draft.startedLocal = next.initialStart;
  if (draft.endedLocal === old.initialEnd) draft.endedLocal = next.initialEnd;
  if (draft.note === old.initialNote) draft.note = next.initialNote;
  editor.value = next;
  conflict.value = false;
  formError.value = undefined;
  replay.value = undefined;
}
async function submit(): Promise<void> {
  if (!canEdit.value || pending.value || replay.value || conflict.value) return;
  const scope = captureScope();
  const intent = {
    expectedActorId: scope.actor,
    expectedSessionId: scope.session,
    requestId: crypto.randomUUID(),
  };
  let command: Mutation | undefined;
  if (editor.value) {
    const body = correctionFromDraft(editor.value, draft, intent);
    if (body)
      command = { scope, draftVersion, operation: "correct", record: editor.value.record.id, body };
  } else {
    const body = manualFromDraft(draft, timeZone.value, intent);
    if (body) command = { scope, draftVersion, operation: "manual", body };
  }
  if (!command) {
    formError.value = !draft.reason.trim()
      ? t("task.time.personal.reason")
      : t("task.time.duration.invalid");
    return;
  }
  await execute(command);
}
async function release(record: TimerRecord): Promise<void> {
  if (!canEdit.value || pending.value || replay.value || !record.reservedLegacy) return;
  const scope = captureScope();
  await execute({
    scope,
    draftVersion,
    operation: "release",
    body: Object.freeze({
      expectedActorId: scope.actor,
      expectedSessionId: scope.session,
      requestId: crypto.randomUUID(),
      timeEntryId: record.id,
    }),
  });
}
async function execute(command: Mutation): Promise<void> {
  if (!current(command.scope) || pending.value) return;
  pending.value = true;
  pendingCommand = command;
  formError.value = undefined;
  try {
    if (command.operation === "manual")
      await sendPersonalManual(command.scope.workspace, command.scope.task, command.body);
    else if (command.operation === "correct")
      await sendPersonalCorrection(
        command.scope.workspace,
        command.scope.task,
        command.record,
        command.body,
      );
    else await sendLegacyRelease(command.body);
    if (!current(command.scope)) return;
    if (replay.value === command) replay.value = undefined;
    if (command.operation !== "release" && command.draftVersion === draftVersion) {
      clearDraft();
      saved.value = true;
    }
    await refresh(command.scope);
  } catch (error) {
    if (!current(command.scope)) return;
    if (denied(error)) {
      await retirePersonal(error);
    } else if (error instanceof ProblemError && error.status === 409) {
      formError.value = loadErrorMessage(error);
      if (command.operation === "correct" && error.reason === "time_record_version") {
        conflict.value = true;
        await reloadHistory();
      }
    } else {
      formError.value = loadErrorMessage(error);
      if (!(error instanceof ProblemError) || [429, 503].includes(error.status))
        replay.value = command;
    }
  } finally {
    if (current(command.scope) && pendingCommand === command) {
      pending.value = false;
      pendingCommand = undefined;
    }
  }
}
function memberName(userId: string): string {
  const member = props.members.find((m) => m.userId === userId);
  return member ? formatPersonName(member) : userId.slice(0, 8);
}
function millisecondsText(value: number): string {
  return `${stopwatchText(value)}.${String(Math.max(0, Math.trunc(value)) % 1000).padStart(3, "0")}`;
}
function recordInstant(value: string): string {
  return formatInstant(value, timeZone.value, {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
  });
}
</script>

<template>
  <section class="flex min-w-0 flex-col gap-3 text-sm" data-testid="task-time-entries">
    <h2 class="text-sm font-medium">{{ t("task.time.heading") }}</h2>
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
        <li
          v-for="row in items"
          :key="row.id"
          :data-time-entry-id="row.id"
          class="break-keep text-sm"
        >
          {{ memberName(row.userId) }}
          {{
            formatInstant(row.startedAt, timeZone, {
              month: "numeric",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
            })
          }}
          {{
            row.durationSeconds == null ? t("task.time.open") : formatDuration(row.durationSeconds)
          }}{{ row.note ? ` · ${row.note}` : "" }}
        </li>
      </ul>
    </div>
    <section
      class="flex min-w-0 flex-col gap-2 border-t border-default pt-3"
      data-testid="task-personal-time-records"
    >
      <h3 class="text-sm font-medium">{{ t("task.time.personal.heading") }}</h3>
      <QueryError
        v-if="revoked"
        :message="loadErrorMessage(personalError)"
        @retry="timer.refetch()"
      />
      <template v-else>
        <QueryLoading
          v-if="timer.isLoading.value || today.isLoading.value || week.isLoading.value"
        />
        <dl
          v-if="today.data.value && week.data.value"
          class="flex flex-wrap gap-x-6 gap-y-2 tabular-nums"
          data-testid="task-personal-time-summary"
        >
          <div
            ><dt class="text-muted">{{ t("task.time.personal.today") }}</dt
            ><dd>{{ millisecondsText(today.data.value.totalMilliseconds) }}</dd></div
          >
          <div
            ><dt class="text-muted">{{ t("task.time.personal.week") }}</dt
            ><dd>{{ millisecondsText(week.data.value.totalMilliseconds) }}</dd></div
          >
        </dl>
        <QueryError
          v-if="today.isError.value && !denied(today.error.value)"
          :message="loadErrorMessage(today.error.value)"
          @retry="today.refetch()"
        />
        <QueryError
          v-if="week.isError.value && !denied(week.error.value)"
          :message="loadErrorMessage(week.error.value)"
          @retry="week.refetch()"
        />
        <p v-if="week.data.value?.unfinished" class="break-keep text-muted">{{
          t("task.time.personal.unfinished")
        }}</p>
        <p v-if="week.data.value?.unresolvedManual" class="break-keep text-muted">{{
          t("task.time.personal.unresolved")
        }}</p>
        <fieldset class="grid min-w-0 gap-2 sm:grid-cols-2">
          <legend class="mb-1 text-muted">{{ t("task.time.personal.range") }}</legend>
          <label class="flex min-w-0 flex-col gap-1"
            >{{ t("task.time.personal.from") }}
            <input
              :value="historyScope.from"
              type="date"
              class="min-w-0 rounded-md border border-default bg-default px-3 py-2 text-sm"
              @input="fromDate = ($event.target as HTMLInputElement).value"
            />
          </label>
          <label class="flex min-w-0 flex-col gap-1"
            >{{ t("task.time.personal.to") }}
            <input
              :value="historyScope.to"
              type="date"
              class="min-w-0 rounded-md border border-default bg-default px-3 py-2 text-sm"
              @input="toDate = ($event.target as HTMLInputElement).value"
            />
          </label>
        </fieldset>
        <QueryLoading v-if="history.isLoading.value" />
        <div v-if="historyChanged" class="flex flex-wrap items-start gap-2" role="alert">
          <p class="break-keep">{{ t("task.time.personal.changed") }}</p>
          <UButton type="button" size="sm" variant="outline" @click="reloadHistory">{{
            t("task.time.personal.reload")
          }}</UButton>
        </div>
        <QueryError
          v-else-if="history.isError.value && !denied(history.error.value)"
          :message="loadErrorMessage(history.error.value)"
          @retry="history.refetch()"
        />
        <p
          v-if="history.isSuccess.value && !historyChanged && records.length === 0"
          class="text-muted"
          >{{ t("task.time.personal.none") }}</p
        >
        <ul class="flex min-w-0 flex-col gap-3">
          <li
            v-for="row in records"
            :key="`${row.kind}:${row.id}`"
            class="flex min-w-0 flex-col gap-1 rounded-md border border-default p-3"
            :data-record-id="row.id"
            :data-record-kind="row.kind"
          >
            <p class="break-keep text-muted"
              >{{
                row.kind === "manual"
                  ? t("task.time.personal.manual")
                  : t("task.time.personal.segment")
              }}
              · {{ t("task.time.personal.revision", { revision: row.revision }) }}</p
            >
            <p class="break-keep tabular-nums"
              >{{ recordInstant(row.startedAt) }} →
              {{ row.endedAt ? recordInstant(row.endedAt) : t("task.time.open") }}</p
            >
            <p v-if="row.note" class="whitespace-pre-wrap break-words">{{ row.note }}</p>
            <div v-if="canEdit" class="flex flex-wrap gap-2">
              <UButton
                v-if="row.kind === 'manual' || row.endedAt !== null"
                type="button"
                size="sm"
                variant="outline"
                color="neutral"
                :disabled="pending"
                @click="openCorrection(row)"
                >{{ t("task.time.personal.edit") }}</UButton
              >
              <details v-if="row.reservedLegacy" class="min-w-0">
                <summary
                  class="cursor-pointer rounded text-sm focus-visible:outline-2 focus-visible:outline-offset-2"
                  >{{ t("task.time.personal.release") }}</summary
                >
                <p class="my-2 max-w-prose break-keep text-muted">{{
                  t("task.time.personal.releaseDisclosure")
                }}</p>
                <UButton
                  type="button"
                  size="sm"
                  variant="outline"
                  color="neutral"
                  :disabled="pending"
                  @click="release(row)"
                  >{{ t("task.time.personal.release") }}</UButton
                >
              </details>
            </div>
          </li>
        </ul>
        <UButton
          v-if="!historyChanged && history.hasNextPage.value"
          type="button"
          size="sm"
          variant="outline"
          color="neutral"
          class="self-start"
          :disabled="history.isFetchingNextPage.value"
          @click="history.fetchNextPage()"
          >{{ t("task.time.personal.loadMore") }}</UButton
        >
        <p v-if="saved" role="status">{{ t("task.time.personal.saved") }}</p>
        <UButton
          v-if="canEdit && !formOpen"
          type="button"
          size="sm"
          variant="outline"
          color="neutral"
          class="self-start"
          :aria-expanded="false"
          @click="openManual"
          >{{ t("task.time.submit") }}</UButton
        >
      </template>
      <form
        v-if="canEdit && formOpen"
        class="flex min-w-0 flex-col gap-2 sm:max-w-lg"
        novalidate
        @submit.prevent="submit"
      >
        <h4 class="font-medium">{{
          editor ? t("task.time.personal.edit") : t("task.time.submit")
        }}</h4>
        <div class="grid min-w-0 gap-2 sm:grid-cols-2">
          <label :for="`${fieldId}-start`" class="flex min-w-0 flex-col gap-1 text-muted"
            >{{ t("task.time.startedAt") }}
            <input
              :id="`${fieldId}-start`"
              v-model="draft.startedLocal"
              type="datetime-local"
              class="min-w-0 rounded-md border border-default bg-default px-3 py-2 text-sm text-default"
            />
          </label>
          <label :for="`${fieldId}-end`" class="flex min-w-0 flex-col gap-1 text-muted"
            >{{ t("task.time.endedAt") }}
            <input
              :id="`${fieldId}-end`"
              v-model="draft.endedLocal"
              type="datetime-local"
              class="min-w-0 rounded-md border border-default bg-default px-3 py-2 text-sm text-default"
            />
          </label>
        </div>
        <label :for="`${fieldId}-note`" class="flex min-w-0 flex-col gap-1 text-muted"
          >{{ t("task.time.note") }}
          <textarea
            :id="`${fieldId}-note`"
            v-model="draft.note"
            class="min-h-16 rounded-md border border-default bg-default px-3 py-2 text-sm text-default"
            maxlength="2000"
          />
        </label>
        <label :for="`${fieldId}-reason`" class="flex min-w-0 flex-col gap-1 text-muted"
          >{{ t("task.time.personal.reason") }}
          <textarea
            :id="`${fieldId}-reason`"
            v-model="draft.reason"
            class="min-h-16 rounded-md border border-default bg-default px-3 py-2 text-sm text-default"
            maxlength="2000"
            required
          />
        </label>
        <p v-if="formError" role="alert" class="break-keep text-error">{{ formError }}</p>
        <p v-if="conflict" role="alert" class="break-keep">{{
          t("task.time.personal.correctionConflict")
        }}</p>
        <UButton
          v-if="conflict && currentConflictRecord"
          type="button"
          size="sm"
          variant="outline"
          class="self-start"
          @click="applyCurrentRecord"
          >{{ t("task.time.personal.applyNew") }}</UButton
        >
        <div class="flex flex-wrap gap-2">
          <UButton type="submit" size="sm" :disabled="pending || conflict || Boolean(replay)">{{
            editor ? t("task.time.personal.edit") : t("task.time.submit")
          }}</UButton>
          <UButton
            v-if="replay"
            type="button"
            size="sm"
            variant="outline"
            :disabled="pending"
            @click="execute(replay)"
            >{{ t("task.time.personal.replay") }}</UButton
          >
          <UButton type="button" size="sm" variant="outline" color="neutral" @click="clearDraft">{{
            t("task.create.cancel")
          }}</UButton>
        </div>
      </form>
      <div v-if="!formOpen && replay" class="flex flex-wrap gap-2" role="alert">
        <p class="break-keep">{{ formError }}</p>
        <UButton
          type="button"
          size="sm"
          variant="outline"
          :disabled="pending"
          @click="execute(replay)"
          >{{ t("task.time.personal.replay") }}</UButton
        >
      </div>
    </section>
  </section>
</template>
