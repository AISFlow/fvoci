<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, watch } from "vue";
import { ProblemError, loadErrorMessage } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { taskTimeEntriesQuery } from "@/features/tasks/queries";
import { anchoredElapsed, stopwatchText } from "./task-stopwatch-clock";
import {
  taskStopwatchQuery,
  ownerStopwatchQuery,
  sendTimerCommand,
  timerContextChanged,
  type TimerCommand,
} from "./task-stopwatch-queries";

const props = defineProps<{
  workspaceId: string;
  taskId: string;
  readOnly: boolean;
  estimate?: string | null;
  compact?: boolean;
}>();
const me = useQuery(meQuery);
const actor = computed(() => me.data.value?.userId ?? "");
const credential = computed(() => me.data.value?.sessionId ?? "");
const client = useQueryClient();
const state = useQuery(() =>
  taskStopwatchQuery(actor.value, props.workspaceId, props.taskId, credential.value),
);
const revoked = ref(false);
const denial = ref<ProblemError>();
let revokedUpdate = 0;
const visibleState = computed(() => (revoked.value ? undefined : state.data.value));
const editable = computed(
  () => !props.readOnly && !revoked.value && visibleState.value?.canControl === true,
);
function denied(err: unknown): err is ProblemError {
  return (
    (err instanceof ProblemError && [401, 403, 404].includes(err.status)) ||
    timerContextChanged(err)
  );
}
async function retirePrivateState(err: ProblemError): Promise<void> {
  if (revoked.value) return;
  const key = taskStopwatchQuery(
    actor.value,
    props.workspaceId,
    props.taskId,
    credential.value,
  ).queryKey;
  revokedUpdate = state.dataUpdatedAt.value;
  denial.value = err;
  revoked.value = true;
  note.value = "";
  await client.cancelQueries({ queryKey: key, exact: true });
  client.removeQueries({ queryKey: key, exact: true });
  if (
    timerContextChanged(err) &&
    live &&
    key[1] === actor.value &&
    key[2] === credential.value &&
    key[3] === props.workspaceId &&
    key[4] === props.taskId
  ) {
    await client.invalidateQueries({ queryKey: meQuery.queryKey, exact: true });
  }
}
watch(
  [() => state.error.value, () => state.dataUpdatedAt.value],
  async () => {
    const err = state.error.value;
    if (denied(err)) {
      await retirePrivateState(err);
    } else if (
      revoked.value &&
      state.isSuccess.value &&
      state.dataUpdatedAt.value > revokedUpdate
    ) {
      revoked.value = false;
      denial.value = undefined;
    }
  },
  { flush: "sync" },
);
const note = ref("");
const error = ref<string | null>(null);
const pending = ref(false);
const retryable = ref(false);
let live = true;
let generation = 0;
type Capture = {
  actor: string;
  credential: string;
  workspace: string;
  task: string;
  generation: number;
  body: TimerCommand;
  note: string;
};
let command: Capture | undefined;
watch(
  [actor, () => props.workspaceId, () => props.taskId, credential],
  ([nextActor, nextWorkspace, nextTask], [previousActor, previousWorkspace, previousTask]) => {
    if (
      nextActor !== previousActor ||
      nextWorkspace !== previousWorkspace ||
      nextTask !== previousTask
    )
      note.value = "";
    revoked.value = false;
    denial.value = undefined;
    generation++;
    command = undefined;
    error.value = null;
    retryable.value = false;
    pending.value = false;
  },
  { flush: "sync" },
);
watch(
  editable,
  (canEdit) => {
    generation++;
    command = undefined;
    retryable.value = false;
    pending.value = false;
    if (!canEdit) note.value = "";
  },
  { flush: "sync" },
);

const receivedAt = ref(performance.now());
const tick = ref(receivedAt.value);
watch(
  () => state.dataUpdatedAt.value,
  () => {
    receivedAt.value = performance.now();
    tick.value = receivedAt.value;
  },
);
let ticker: ReturnType<typeof setInterval> | undefined;
watch(
  () => visibleState.value?.run?.runningSince,
  (anchor) => {
    if (ticker) clearInterval(ticker);
    ticker = undefined;
    tick.value = performance.now();
    if (anchor)
      ticker = setInterval(() => {
        tick.value = performance.now();
      }, 250);
  },
  { immediate: true },
);
onScopeDispose(() => {
  live = false;
  generation++;
  if (ticker) clearInterval(ticker);
});
const run = computed(() => visibleState.value?.run);
const elapsed = computed(() =>
  visibleState.value
    ? anchoredElapsed(
        run.value?.elapsedMilliseconds ?? 0,
        run.value?.runningSince,
        visibleState.value.serverNow,
        receivedAt.value,
        tick.value,
      )
    : 0,
);
const actual = computed(
  () =>
    (visibleState.value?.actualMilliseconds ?? 0) +
    (run.value?.runningSince ? Math.max(0, elapsed.value - run.value.elapsedMilliseconds) : 0),
);
const disabled = computed(
  () => !editable.value || pending.value || !state.isSuccess.value || !actor.value,
);

function current(capture: Capture): boolean {
  return (
    live &&
    capture.generation === generation &&
    capture.actor === actor.value &&
    capture.credential === credential.value &&
    capture.workspace === props.workspaceId &&
    capture.task === props.taskId
  );
}

async function submit(capture: Capture): Promise<void> {
  if (!current(capture) || !editable.value || pending.value) return;
  pending.value = true;
  retryable.value = false;
  error.value = null;
  try {
    await sendTimerCommand(capture.workspace, capture.task, capture.body);
    if (!current(capture)) return;
    command = undefined;
    if (note.value === capture.note) note.value = "";
    await Promise.all([
      client.invalidateQueries({
        queryKey: ownerStopwatchQuery(capture.actor, capture.credential).queryKey,
        exact: true,
      }),
      client.invalidateQueries({
        predicate: (query) =>
          query.queryKey[0] === "task-timer" && query.queryKey[1] === capture.actor,
      }),
      client.invalidateQueries({
        queryKey: taskTimeEntriesQuery(capture.workspace, capture.task).queryKey,
        exact: true,
      }),
    ]);
  } catch (err) {
    if (!current(capture)) return;
    error.value = loadErrorMessage(err);
    retryable.value = !(err instanceof ProblemError) || err.status === 429 || err.status >= 500;
    if (!retryable.value) command = undefined;
    // Authority/context failures clear private display; network/503 keep the draft and
    // original request id for explicit replay. Session guard owns navigation.
    if (denied(err)) await retirePrivateState(err);
    else if (err instanceof ProblemError && err.status === 409) await state.refetch();
  } finally {
    if (current(capture)) pending.value = false;
  }
}

async function start(operation: TimerCommand["operation"]): Promise<void> {
  if (disabled.value) return;
  const body: TimerCommand = {
    expectedActorId: actor.value,
    expectedSessionId: credential.value,
    requestId: crypto.randomUUID(),
    operation,
    expectedVersion: run.value?.version ?? 0,
    runId: run.value?.id ?? null,
    note: note.value.trim() || null,
  };
  command = {
    actor: actor.value,
    credential: credential.value,
    workspace: props.workspaceId,
    task: props.taskId,
    generation,
    body,
    note: note.value,
  };
  await submit(command);
}
async function retry(): Promise<void> {
  if (command) await submit(command);
}
</script>

<template>
  <section class="flex flex-col gap-2 py-2 text-sm" :data-testid="`task-stopwatch-${taskId}`">
    <div class="flex flex-wrap items-center gap-2">
      <h2 v-if="!compact" class="text-base font-medium">스톱워치</h2>
      <output
        v-if="visibleState"
        class="text-base tabular-nums"
        data-testid="timer-elapsed"
        aria-label="측정 시간"
        >{{ stopwatchText(elapsed) }}</output
      >
      <span v-if="run" class="text-sm text-muted" data-testid="timer-state">{{
        run.status === "running" ? "측정 중" : "일시정지"
      }}</span>
      <span v-if="visibleState" class="text-sm text-muted" data-testid="timer-actual"
        >실제 {{ stopwatchText(actual) }}</span
      >
      <span
        v-if="visibleState && estimate != null"
        class="text-sm text-muted"
        data-testid="timer-estimate"
        >예상 {{ estimate }}</span
      >
    </div>
    <p v-if="revoked || state.isError.value" role="alert" class="break-keep text-error">{{
      loadErrorMessage(denial ?? state.error.value)
    }}</p>
    <p v-if="visibleState?.legacyOpen" role="status" class="break-keep"
      >기존 미종료 기록을 확인하고 종료 시각을 보정한 뒤 시작할 수 있습니다.</p
    >
    <p v-else-if="visibleState?.busyElsewhere" role="status" class="break-keep"
      >다른 작업을 측정하거나 일시정지하고 있습니다. 먼저 그 측정을 종료하세요.</p
    >
    <label v-if="!compact && editable" class="flex flex-col gap-1 text-sm"
      >측정 메모
      <textarea
        v-model="note"
        rows="2"
        maxlength="2000"
        class="rounded-md border border-default bg-default px-3 py-2 text-base"
        :disabled="pending"
      />
    </label>
    <div class="flex flex-wrap gap-2">
      <UButton
        v-if="!run"
        type="button"
        size="sm"
        data-testid="timer-start"
        :disabled="disabled || visibleState?.busyElsewhere || visibleState?.legacyOpen"
        @click="start('start')"
        >시작</UButton
      >
      <UButton
        v-if="run?.status === 'running'"
        type="button"
        size="sm"
        data-testid="timer-pause"
        :disabled="disabled"
        @click="start('pause')"
        >일시정지</UButton
      >
      <UButton
        v-if="run?.status === 'paused'"
        type="button"
        size="sm"
        data-testid="timer-resume"
        :disabled="disabled"
        @click="start('resume')"
        >재개</UButton
      >
      <UButton
        v-if="run"
        type="button"
        size="sm"
        variant="outline"
        color="neutral"
        data-testid="timer-stop"
        :disabled="disabled"
        @click="start('stop')"
        >측정 종료</UButton
      >
      <UButton
        v-if="retryable"
        type="button"
        size="sm"
        variant="outline"
        :disabled="pending"
        @click="retry"
        >같은 요청 다시 보내기</UButton
      >
      <UButton
        v-if="state.isError.value"
        type="button"
        size="sm"
        variant="outline"
        @click="state.refetch()"
        >다시 불러오기</UButton
      >
    </div>
    <p v-if="error" role="alert" class="break-keep text-sm text-error">{{ error }}</p>
    <p v-if="run && !compact" class="break-keep text-sm text-muted"
      >일시정지 시간은 제외합니다. 측정 종료는 할 일 완료 상태를 바꾸지 않습니다.</p
    >
  </section>
</template>
