<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { onScopeDispose, ref, useId, watch } from "vue";
import { useRouter } from "vue-router";
import NativeModal from "../../components/NativeModal.vue";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { itemPath } from "@/lib/href";
import { meQuery } from "@/lib/queries";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import { createPersonalInput, ensurePersonalWorkspace } from "./personal-input-api";
import {
  forgetCommand,
  inputScope,
  recoverCommand,
  rememberCommand,
  type InputIntent,
  type PendingInputCommand,
  type PersonalInputResult,
} from "./capture-command";
const props = defineProps<{ workspaceId: string }>();
const me = useQuery(meQuery);
const client = useQueryClient();
const router = useRouter();
const id = useId();
const open = ref(false);
const intent = ref<InputIntent>("quick");
const title = ref("");
const pending = ref<PendingInputCommand | null>(null);
const result = ref<PersonalInputResult | null>(null);
const personalSlug = ref("");
const busy = ref(false);
const error = ref<string | null>(null);
const scope = inputScope();
let lifetime = 0;
watch(
  [
    () => me.data.value?.userId ?? "",
    () => me.data.value?.sessionId ?? "",
    () => me.error.value instanceof ProblemError && me.error.value.status === 401,
    () => props.workspaceId,
    open,
  ],
  ([actor, credential, retired, workspace]) => {
    scope.bind(retired ? "" : actor, `${workspace}:${String(++lifetime)}`, credential);
    result.value = null;
    error.value = null;
    busy.value = false;
    pending.value = actor && open.value ? recoverCommand(window.sessionStorage, actor) : null;
    if (pending.value) {
      title.value = pending.value.body.title;
      intent.value = pending.value.body.intent;
    }
  },
  { immediate: true, flush: "sync" },
);
onScopeDispose(() => {
  scope.retire();
});
function close(): void {
  open.value = false;
}
function preventIme(event: KeyboardEvent): void {
  if (event.key === "Enter" && event.isComposing) event.preventDefault();
}
async function submit(): Promise<void> {
  if (busy.value || !title.value.trim() || result.value) return;
  const captured = scope.capture();
  if (!captured.actor) return;
  busy.value = true;
  error.value = null;
  try {
    const workspace = await ensurePersonalWorkspace();
    if (!scope.current(captured)) return;
    personalSlug.value = workspace.slug;
    const command = pending.value ?? {
      actorId: captured.actor,
      workspaceId: workspace.id,
      body: { requestId: crypto.randomUUID(), intent: intent.value, title: title.value.trim() },
    };
    if (command.workspaceId !== workspace.id) throw new Error(t("capture.unavailable"));
    // This can throw (storage disabled/full); no request is sent without a
    // durable retry key. Unknown results keep both the key AND immutable payload.
    rememberCommand(window.sessionStorage, command);
    pending.value = command;
    const saved = await createPersonalInput(command.workspaceId, command.body);
    if (!scope.sameActor(captured)) return;
    forgetCommand(window.sessionStorage, command);
    await Promise.all([
      client.invalidateQueries({ queryKey: ["tree", command.workspaceId] }),
      client.invalidateQueries({ queryKey: ["wiki-discovery", command.workspaceId] }),
      client.invalidateQueries({ queryKey: ["me", "workspaces"] }),
      saved.taskId && saved.projectId
        ? invalidateTaskCaches(client, command.workspaceId, saved.projectId, saved.taskId)
        : Promise.resolve(),
    ]);
    if (scope.current(captured)) {
      result.value = saved;
      pending.value = null;
    }
  } catch (failure) {
    if (scope.current(captured)) error.value = loadErrorMessage(failure);
  } finally {
    if (scope.current(captured)) busy.value = false;
  }
}
function abandon(): void {
  if (busy.value || !pending.value || !window.confirm(t("capture.abandonConfirm"))) return;
  forgetCommand(window.sessionStorage, pending.value);
  pending.value = null;
  error.value = null;
}
function visit(kind: "document" | "task"): void {
  const display = kind === "task" ? result.value?.taskDisplayId : result.value?.documentDisplayId;
  if (!display || !personalSlug.value) return;
  open.value = false;
  router.push(itemPath(personalSlug.value, display)).catch((failure: unknown) => {
    error.value = loadErrorMessage(failure);
  });
}
</script>
<template>
  <UButton
    size="sm"
    aria-haspopup="dialog"
    :aria-expanded="open"
    :aria-controls="open ? id : undefined"
    @click="open = true"
    >{{ t("capture.open") }}</UButton
  >
  <NativeModal
    :id="id"
    :open="open"
    :labelled-by="`${id}-title`"
    dialog-class="mx-auto mt-[10vh] w-[min(32rem,calc(100%-2rem))] max-h-[80dvh] overflow-auto rounded-lg border border-default bg-default p-4 text-default backdrop:bg-black/30"
    @close="close"
  >
    <div class="flex flex-col gap-4 break-keep">
      <h2 :id="`${id}-title`" class="text-xl font-semibold">{{ t("capture.open") }}</h2>
      <p class="text-base leading-relaxed">{{ t("capture.private") }}</p>
      <form
        v-if="!result"
        class="flex flex-col gap-4"
        @keydown="preventIme"
        @submit.prevent="submit"
      >
        <fieldset :disabled="busy || !!pending" class="flex flex-wrap gap-4">
          <legend class="mb-2 text-sm font-medium">{{ t("capture.intent") }}</legend>
          <label
            v-for="choice in ['quick', 'task', 'note'] as const"
            :key="choice"
            class="flex items-center gap-2 text-base"
          >
            <input v-model="intent" type="radio" :name="`${id}-intent`" :value="choice" />{{
              t(`capture.${choice}`)
            }}
          </label>
        </fieldset>
        <div class="flex flex-col gap-1">
          <label :for="`${id}-text`" class="text-sm font-medium">{{ t("capture.title") }}</label>
          <textarea
            :id="`${id}-text`"
            v-model="title"
            autofocus
            rows="3"
            maxlength="300"
            :disabled="busy || !!pending"
            class="w-full resize-y rounded-md border border-default bg-default p-2 text-base leading-relaxed"
          />
        </div>
        <p v-if="pending" role="status" class="text-base leading-relaxed">{{
          t("capture.unknown")
        }}</p>
        <p v-if="error" role="alert" class="text-base leading-relaxed">{{ error }}</p>
        <div class="flex flex-wrap gap-2">
          <UButton type="submit" :disabled="busy || !title.trim()">{{
            busy ? t("capture.saving") : pending ? t("capture.retry") : t("capture.save")
          }}</UButton>
          <UButton
            v-if="pending"
            color="neutral"
            variant="outline"
            :disabled="busy"
            @click="abandon"
            >{{ t("capture.abandon") }}</UButton
          >
          <UButton color="neutral" variant="ghost" @click="close">{{ t("capture.close") }}</UButton>
        </div>
      </form>
      <div v-else class="flex flex-col gap-3">
        <p role="status">{{ t("capture.saved") }}</p>
        <UButton v-if="result.taskId" @click="visit('task')">{{ t("capture.openTask") }}</UButton>
        <UButton color="neutral" variant="outline" @click="visit('document')">{{
          t("capture.openNote")
        }}</UButton>
        <UButton color="neutral" variant="ghost" @click="close">{{ t("capture.close") }}</UButton>
      </div>
    </div>
  </NativeModal>
</template>
