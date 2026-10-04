<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { useQueryClient } from "@tanstack/vue-query";
import { onScopeDispose, ref, useId, watch } from "vue";
import type { components } from "@/generated/api";
import { taskQuery } from "@/features/tasks/queries";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { explicitMinutes, minuteEstimate } from "./task-plan-input";
import {
  sendTaskEstimate,
  taskStopwatchQuery,
  timerContextChanged,
  type TaskEstimateCommand,
} from "./task-stopwatch-queries";

type Estimate = components["schemas"]["TaskEstimate"];
const props = defineProps<{
  workspaceId: string;
  taskId: string;
  actor: string;
  session: string;
  snapshot: Estimate;
  canEdit: boolean;
}>();
const emit = defineEmits<{ denied: [error: ProblemError]; refresh: [] }>();
const client = useQueryClient();
const id = useId();
const draft = ref("");
const reason = ref("");
const pending = ref(false);
const retryable = ref(false);
const conflict = ref(false);
const error = ref<string>();
let baseline: Estimate;
let lifetime = 0;
let revision = 0;
let generation = 0;
let dirty = false;
let live = true;
type Capture = {
  actor: string;
  session: string;
  workspace: string;
  task: string;
  lifetime: number;
  generation: number;
  revision: number;
  body: TaskEstimateCommand;
};
let command: Capture | undefined;

function loadCurrent() {
  baseline = Object.freeze({ ...props.snapshot });
  const minutes = minuteEstimate(baseline.value, baseline.unit);
  draft.value = minutes === null ? "" : String(minutes);
  reason.value = "";
  dirty = false;
  revision++;
  command = undefined;
  retryable.value = false;
  conflict.value = false;
  error.value = undefined;
}
watch(
  () => [props.actor, props.session, props.workspaceId, props.taskId, props.canEdit],
  () => {
    lifetime++;
    generation++;
    pending.value = false;
    loadCurrent();
  },
  { immediate: true, flush: "sync" },
);
watch(
  () => props.snapshot.updatedAt,
  () => {
    if (!dirty && !pending.value) loadCurrent();
  },
);
onScopeDispose(() => {
  live = false;
  lifetime++;
  generation++;
});
function edited() {
  dirty = true;
  revision++;
  command = undefined;
  retryable.value = false;
}
function current(c: Capture) {
  return (
    live &&
    props.canEdit &&
    c.lifetime === lifetime &&
    c.generation === generation &&
    c.actor === props.actor &&
    c.session === props.session &&
    c.workspace === props.workspaceId &&
    c.task === props.taskId
  );
}
async function submit(c: Capture) {
  if (!current(c) || pending.value) return;
  pending.value = true;
  retryable.value = false;
  error.value = undefined;
  try {
    const result = await sendTaskEstimate(c.workspace, c.task, c.body);
    if (!current(c)) return;
    command = undefined;
    if (revision === c.revision) {
      baseline = Object.freeze({ ...result });
      const minutes = minuteEstimate(result.value, result.unit);
      draft.value = minutes === null ? "" : String(minutes);
      reason.value = "";
      dirty = false;
      conflict.value = false;
    }
    await Promise.all([
      client.invalidateQueries({
        queryKey: taskStopwatchQuery(c.actor, c.workspace, c.task, c.session).queryKey,
        exact: true,
      }),
      client.invalidateQueries({ queryKey: taskQuery(c.workspace, c.task).queryKey, exact: true }),
    ]);
  } catch (err) {
    if (!current(c)) return;
    conflict.value = err instanceof ProblemError && err.reason === "estimate_changed";
    error.value = loadErrorMessage(err);
    if (conflict.value) {
      error.value = "예상 시간이 다른 곳에서 변경되었습니다. 현재 값을 확인한 뒤 다시 입력하세요.";
      emit("refresh");
    }
    retryable.value = !(err instanceof ProblemError) || err.status === 429 || err.status >= 500;
    if (!retryable.value) command = undefined;
    if (
      err instanceof ProblemError &&
      ([401, 403, 404].includes(err.status) || timerContextChanged(err))
    )
      emit("denied", err);
  } finally {
    if (current(c)) pending.value = false;
  }
}
async function save() {
  if (!props.canEdit || pending.value) return;
  const parsed = explicitMinutes(draft.value);
  if (!parsed.valid || !reason.value.trim()) {
    error.value = "0 이상의 정수 분과 변경 사유를 입력하세요.";
    return;
  }
  command = {
    actor: props.actor,
    session: props.session,
    workspace: props.workspaceId,
    task: props.taskId,
    lifetime,
    generation: ++generation,
    revision,
    body: {
      expectedActorId: props.actor,
      expectedSessionId: props.session,
      requestId: crypto.randomUUID(),
      expected: Object.freeze({ ...baseline }),
      minutes: parsed.minutes,
      reason: reason.value.trim(),
    },
  };
  await submit(command);
}
</script>

<template>
  <details
    v-if="canEdit"
    class="min-w-0 rounded-md border border-default p-3"
    data-testid="task-estimate-editor"
  >
    <summary class="cursor-pointer break-keep text-sm">예상 시간 설정</summary>
    <form class="mt-3 flex min-w-0 flex-col gap-3" @submit.prevent="save">
      <label :for="`${id}-minutes`" class="flex min-w-0 flex-col gap-1 text-sm"
        >예상 시간(분)
        <input
          :id="`${id}-minutes`"
          v-model="draft"
          inputmode="numeric"
          :disabled="pending"
          class="min-w-0 rounded-md border border-default bg-default px-3 py-2 text-base"
          @input="edited"
        />
      </label>
      <p class="break-keep text-sm text-muted"
        >비워서 저장하면 예상 시간을 제거합니다. 기존 단위 미지정 값은 자동으로 분으로 바꾸지
        않습니다.</p
      >
      <label :for="`${id}-reason`" class="flex min-w-0 flex-col gap-1 text-sm"
        >예상 시간 변경 사유
        <textarea
          :id="`${id}-reason`"
          v-model="reason"
          maxlength="2000"
          rows="2"
          :disabled="pending"
          class="min-w-0 rounded-md border border-default bg-default px-3 py-2 text-base"
          @input="edited"
        />
      </label>
      <div class="flex flex-wrap gap-2">
        <UButton type="submit" size="sm" :disabled="pending">예상 시간 저장</UButton>
        <UButton
          v-if="retryable"
          type="button"
          size="sm"
          variant="outline"
          :disabled="pending"
          @click="command && submit(command)"
          >같은 요청 다시 보내기</UButton
        >
        <UButton
          v-if="conflict"
          type="button"
          size="sm"
          variant="outline"
          :disabled="pending"
          @click="loadCurrent"
          >현재 값 불러오기</UButton
        >
      </div>
      <p v-if="error" role="alert" class="break-keep text-sm text-error">{{ error }}</p>
    </form>
  </details>
</template>
