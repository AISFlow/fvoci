<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, watch } from "vue";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import type { components } from "@/generated/api";
import { ownerStopwatchQuery, timerContextChanged } from "./task-stopwatch-queries";

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
let live = true;
let retry:
  | {
      actor: string;
      credential: string;
      generation: number;
      body: components["schemas"]["TimerCleanupBody"];
    }
  | undefined;
function denied(err: unknown): err is ProblemError {
  return (
    (err instanceof ProblemError && [401, 403, 404].includes(err.status)) ||
    timerContextChanged(err)
  );
}
async function retireOwnerState(err: ProblemError): Promise<void> {
  if (revoked.value) return;
  const key = ownerStopwatchQuery(actor.value, credential.value).queryKey;
  revokedUpdate = owner.dataUpdatedAt.value;
  denial.value = err;
  revoked.value = true;
  generation++;
  retry = undefined;
  pending.value = false;
  replayAllowed.value = false;
  error.value = null;
  await client.cancelQueries({ queryKey: key, exact: true });
  client.removeQueries({ queryKey: key, exact: true });
  if (timerContextChanged(err) && live && key[1] === actor.value && key[2] === credential.value) {
    await client.invalidateQueries({ queryKey: meQuery.queryKey, exact: true });
  }
}
watch(
  [() => owner.error.value, () => owner.dataUpdatedAt.value],
  async () => {
    const err = owner.error.value;
    if (denied(err)) await retireOwnerState(err);
    else if (revoked.value && owner.isSuccess.value && owner.dataUpdatedAt.value > revokedUpdate) {
      revoked.value = false;
      denial.value = undefined;
    }
  },
  { flush: "sync" },
);
watch(
  [actor, credential],
  () => {
    generation++;
    retry = undefined;
    pending.value = false;
    error.value = null;
    replayAllowed.value = false;
    revoked.value = false;
    denial.value = undefined;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
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
    capture.generation === generation;
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
      client.invalidateQueries({
        predicate: (query) =>
          query.queryKey[0] === "task-timer" && query.queryKey[1] === capture.actor,
      }),
    ]);
  } catch (err) {
    if (!current()) return;
    error.value = loadErrorMessage(err);
    replayAllowed.value = !(err instanceof ProblemError) || err.status === 429 || err.status >= 500;
    retry = replayAllowed.value ? capture : undefined;
    if (denied(err)) await retireOwnerState(err);
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
</template>
