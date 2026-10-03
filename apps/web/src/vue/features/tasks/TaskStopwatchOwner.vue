<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, watch } from "vue";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import type { components } from "@/generated/api";
import {
  ownerStopwatchQuery,
  taskStopwatchQuery,
  timerContextChanged,
  captureTimerDenial,
  captureTimerQuery,
  removeCapturedTimerQuery,
  sendLegacyRelease,
  type TimerQueryCapture,
} from "./task-stopwatch-queries";

const client = useQueryClient();
const me = useQuery(meQuery);
const actor = computed(() => me.data.value?.userId ?? "");
const credential = computed(() => me.data.value?.sessionId ?? "");
const owner = useQuery(() => ownerStopwatchQuery(actor.value, credential.value));
const pending = ref(false);
const error = ref<string | null>(null);
const replayAllowed = ref(false);
const revoked = ref(false);
const denial = ref<ProblemError>();
let revokedUpdate = 0;
const visibleOwner = computed(() => (revoked.value ? undefined : owner.data.value));
let generation = 0;
let scopeLifetime = 0;
let live = true;
let retry:
  | {
      actor: string;
      credential: string;
      generation: number;
      scopeLifetime: number;
      visibleTarget: { workspace: string; task: string } | undefined;
      body: components["schemas"]["TimerCleanupBody"];
    }
  | undefined;
let legacyRetry:
  | {
      actor: string;
      credential: string;
      generation: number;
      scopeLifetime: number;
      body: components["schemas"]["LegacyReleaseBody"];
    }
  | undefined;
function denied(err: unknown): err is ProblemError {
  return (
    (err instanceof ProblemError && [401, 403, 404].includes(err.status)) ||
    timerContextChanged(err)
  );
}
async function retireOwnerState(
  err: ProblemError,
  capture: TimerQueryCapture,
  lifetime: number,
): Promise<void> {
  if (revoked.value) return;
  const key = capture.queryKey;
  const currentScope = () =>
    live && scopeLifetime === lifetime && key[1] === actor.value && key[2] === credential.value;
  if (!currentScope()) return;
  revokedUpdate = owner.dataUpdatedAt.value;
  denial.value = err;
  revoked.value = true;
  generation++;
  retry = undefined;
  legacyRetry = undefined;
  pending.value = false;
  replayAllowed.value = false;
  error.value = null;
  const retired = await removeCapturedTimerQuery(client, capture, currentScope);
  if (retired && timerContextChanged(err) && currentScope()) {
    await client.invalidateQueries({ queryKey: meQuery.queryKey, exact: true });
  }
}
watch(
  [
    () => owner.error.value,
    () => owner.dataUpdatedAt.value,
    () => owner.status.value,
    () => owner.fetchStatus.value,
  ],
  async () => {
    const err = owner.error.value;
    if (denied(err)) {
      const capture = captureTimerDenial(
        client,
        err,
        ownerStopwatchQuery(actor.value, credential.value).queryKey,
        owner.status.value,
        owner.fetchStatus.value,
      );
      if (capture) await retireOwnerState(err, capture, scopeLifetime);
    } else if (
      revoked.value &&
      owner.isSuccess.value &&
      owner.dataUpdatedAt.value > revokedUpdate
    ) {
      revoked.value = false;
      denial.value = undefined;
    }
  },
  { flush: "pre" },
);
watch(
  [actor, credential],
  () => {
    scopeLifetime++;
    generation++;
    retry = undefined;
    legacyRetry = undefined;
    pending.value = false;
    error.value = null;
    replayAllowed.value = false;
    revoked.value = false;
    denial.value = undefined;
  },
  { flush: "sync" },
);
// A server-confirmed successor owns its controls immediately, even while an
// earlier run's committed response is still waiting for delivery to this tab.
watch(
  [
    () => owner.data.value?.runId,
    () => owner.data.value?.version,
    () => owner.data.value?.legacyOpenIds.join(","),
  ],
  () => {
    generation++;
    retry = undefined;
    legacyRetry = undefined;
    pending.value = false;
    error.value = null;
    replayAllowed.value = false;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  scopeLifetime++;
  live = false;
  generation++;
});

async function stop(): Promise<void> {
  if (pending.value || !actor.value || revoked.value || owner.isError.value) return;
  const state = visibleOwner.value;
  if (!state?.runId || !state.version) return;
  const capture = retry ?? {
    actor: actor.value,
    credential: credential.value,
    generation,
    scopeLifetime,
    visibleTarget: state.visibleRun
      ? { workspace: state.visibleRun.workspaceId, task: state.visibleRun.taskId }
      : undefined,
    body: {
      expectedActorId: actor.value,
      expectedSessionId: credential.value,
      requestId: crypto.randomUUID(),
      runId: state.runId,
      expectedVersion: state.version,
    },
  };
  const current = () =>
    live &&
    capture.actor === actor.value &&
    capture.credential === credential.value &&
    capture.generation === generation &&
    capture.scopeLifetime === scopeLifetime &&
    capture.body.runId === owner.data.value?.runId &&
    capture.body.expectedVersion === owner.data.value.version;
  pending.value = true;
  error.value = null;
  replayAllowed.value = false;
  try {
    await ensureOk(await api.POST("/api/v1/me/task-timer/stop", { body: capture.body }));
    if (!current()) return;
    retry = undefined;
    await Promise.all([
      client.invalidateQueries({
        queryKey: ownerStopwatchQuery(capture.actor, capture.credential).queryKey,
        exact: true,
      }),
      ...(capture.visibleTarget
        ? [
            client.invalidateQueries({
              queryKey: taskStopwatchQuery(
                capture.actor,
                capture.visibleTarget.workspace,
                capture.visibleTarget.task,
                capture.credential,
              ).queryKey,
              exact: true,
            }),
          ]
        : []),
    ]);
  } catch (err) {
    if (!current()) return;
    error.value = loadErrorMessage(err);
    replayAllowed.value = !(err instanceof ProblemError) || err.status === 429 || err.status >= 500;
    retry = replayAllowed.value ? capture : undefined;
    if (denied(err))
      await retireOwnerState(
        err,
        captureTimerQuery(client, ownerStopwatchQuery(capture.actor, capture.credential).queryKey),
        capture.scopeLifetime,
      );
    else if (err instanceof ProblemError && err.status === 409) await owner.refetch();
  } finally {
    if (current()) pending.value = false;
  }
}

async function releaseLegacy(timeEntryId: string): Promise<void> {
  if (
    pending.value ||
    !actor.value ||
    !credential.value ||
    revoked.value ||
    owner.isError.value ||
    !visibleOwner.value?.legacyOpenIds.includes(timeEntryId)
  )
    return;
  const capture =
    legacyRetry?.body.timeEntryId === timeEntryId
      ? legacyRetry
      : {
          actor: actor.value,
          credential: credential.value,
          generation,
          scopeLifetime,
          body: {
            expectedActorId: actor.value,
            expectedSessionId: credential.value,
            requestId: crypto.randomUUID(),
            timeEntryId,
          },
        };
  const current = () =>
    live &&
    capture.actor === actor.value &&
    capture.credential === credential.value &&
    capture.generation === generation &&
    capture.scopeLifetime === scopeLifetime &&
    owner.data.value?.legacyOpenIds.includes(capture.body.timeEntryId);
  pending.value = true;
  error.value = null;
  replayAllowed.value = false;
  legacyRetry = undefined;
  try {
    await sendLegacyRelease(capture.body);
    if (!current()) return;
    await client.invalidateQueries({
      queryKey: ownerStopwatchQuery(capture.actor, capture.credential).queryKey,
      exact: true,
    });
  } catch (err) {
    if (!current()) return;
    error.value = loadErrorMessage(err);
    replayAllowed.value = !(err instanceof ProblemError) || err.status === 429 || err.status >= 500;
    legacyRetry = replayAllowed.value ? capture : undefined;
    if (denied(err))
      await retireOwnerState(
        err,
        captureTimerQuery(client, ownerStopwatchQuery(capture.actor, capture.credential).queryKey),
        capture.scopeLifetime,
      );
    else if (err instanceof ProblemError && err.status === 409) await owner.refetch();
  } finally {
    if (current()) pending.value = false;
  }
}
</script>

<template>
  <p v-if="revoked" role="alert" class="break-keep text-error">{{ loadErrorMessage(denial) }}</p>
  <section
    v-if="visibleOwner?.runId"
    class="flex flex-col gap-2 border-b border-default pb-3 text-sm"
    data-testid="timer-owner"
  >
    <p class="break-keep"
      >진행 중이거나 일시정지한 측정이 있습니다. 다른 작업을 시작하려면 현재 측정을 종료하세요.</p
    >
    <p v-if="!visibleOwner.visibleRun" class="break-keep"
      >이전 작업에 접근할 수 없어도 내 측정은 종료할 수 있습니다. 기존 기록은 보존합니다.</p
    >
    <UButton
      type="button"
      size="sm"
      color="neutral"
      variant="outline"
      class="self-start"
      :disabled="pending || owner.isError.value"
      @click="stop"
      >{{ replayAllowed ? "같은 종료 요청 다시 보내기" : "현재 측정 종료" }}</UButton
    >
    <p v-if="error" role="alert" class="break-keep text-error">{{ error }}</p>
    <p v-else-if="owner.isError.value" role="alert" class="break-keep text-error">{{
      loadErrorMessage(owner.error.value)
    }}</p>
  </section>
  <section
    v-if="visibleOwner?.legacyOpenIds.length"
    class="flex flex-col gap-2 border-b border-default pb-3 text-sm"
    data-testid="timer-legacy-owner"
  >
    <p class="break-keep">이전 미종료 기록 때문에 새 측정을 시작할 수 없습니다.</p>
    <p class="break-keep"
      >측정 제한을 해제해도 기존 기록과 메모는 보존합니다. 종료 시각을 알 수 없는 기록은 측정 합계에
      포함하지 않습니다.</p
    >
    <UButton
      v-for="entryId in visibleOwner.legacyOpenIds"
      :key="entryId"
      type="button"
      size="sm"
      color="neutral"
      variant="outline"
      class="self-start whitespace-normal break-keep text-start"
      :disabled="pending || owner.isError.value"
      @click="releaseLegacy(entryId)"
      >{{
        replayAllowed && legacyRetry?.body.timeEntryId === entryId
          ? "같은 제한 해제 요청 다시 보내기"
          : "미종료 기록의 측정 제한 해제"
      }}</UButton
    >
    <p v-if="error" role="alert" class="break-keep text-error">{{ error }}</p>
    <p v-else-if="owner.isError.value" role="alert" class="break-keep text-error">{{
      loadErrorMessage(owner.error.value)
    }}</p>
  </section>
</template>
