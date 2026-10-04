<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, shallowRef, useId, watch } from "vue";
import { useRouter } from "vue-router";
import NativeModal from "../../components/NativeModal.vue";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { meQuery, workspacesQuery } from "@/lib/queries";
import { taskOriginsQuery } from "@/features/collections/origin-api";
import { projectsQuery, workflowQuery } from "@/features/projects/queries";
import { taskQuery, type TaskDetail } from "@/features/tasks/queries";
import { documentMetaQuery } from "@/lib/queries/documents";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import { inputScope } from "./capture-command";
import { confirmPersonalTransfer, previewPersonalTransfer } from "./personal-transfer-api";
import {
  audienceKey,
  buildSelection,
  forgetTransfer,
  OUTCOME_KEYS,
  recoverTransfer,
  rememberTransfer,
  resultPaths,
  retainedTaskProject,
  settleExactKeepingLoadMore,
  unscopedSourceKeys,
  TRANSFER_COMMAND_EVENT,
  transferFailureKeys,
  type PendingTransferCommand,
  type TransferAction,
  type TransferDisposition,
  type TransferPreview,
  type TransferResult,
  type TransferSelection,
  type TransferSource,
} from "./personal-transfer-command";
const props = defineProps<{
  workspaceId: string;
  /** null mounts recovery only: a lost MOVE success can leave no source route. */
  documentId: string | null;
  /**
   * Host-owned: wait for the matching durable save of every open editor and
   * report false while unsaved drafts (title, dates, body) remain.
   */
  prepare?: () => Promise<boolean>;
}>();
const me = useQuery(meQuery);
const client = useQueryClient();
const router = useRouter();
const id = useId();
const open = ref(false);
const action = ref<TransferAction>("copy");
const teamId = ref("");
const projectId = ref("");
const statusId = ref("");
const preview = shallowRef<{ selection: TransferSelection; value: TransferPreview } | null>(null);
const pending = shallowRef<PendingTransferCommand | null>(null);
const result = shallowRef<{ command: PendingTransferCommand; value: TransferResult } | null>(null);
const busy = ref(false);
const error = ref<string | null>(null);
const scope = inputScope();
let lifetime = 0;
const actor = computed(() => me.data.value?.userId ?? "");
const session = computed(() => me.data.value?.sessionId ?? "");
const retired = computed(
  () => me.error.value instanceof ProblemError && me.error.value.status === 401,
);
const workspaces = useQuery(workspacesQuery);
// Only the actor's own personal workspace offers a transfer; elsewhere the
// mount is recovery-only. Versions come from the host's shared query cache.
const personal = computed(() =>
  (workspaces.data.value?.items ?? []).some(
    (workspace) => workspace.id === props.workspaceId && workspace.kind === "personal",
  ),
);
const sourceWorkspace = computed(() =>
  personal.value && props.documentId ? props.workspaceId : "",
);
const documentMeta = useQuery(() =>
  documentMetaQuery(sourceWorkspace.value, props.documentId ?? ""),
);
const origins = useQuery(() =>
  taskOriginsQuery(sourceWorkspace.value, { documentId: props.documentId ?? undefined }, null),
);
// A pair moves together with its single origin task; several origins are a
// dependent graph the server refuses explicitly, never a silent subset.
const originTask = computed(() => {
  const list = origins.data.value;
  return list?.count === 1 ? (list.items[0]?.taskId ?? null) : null;
});
const originTaskDetail = useQuery(() => taskQuery(sourceWorkspace.value, originTask.value ?? ""));
const source = computed<TransferSource | null>(() => {
  const meta = documentMeta.data.value;
  if (!sourceWorkspace.value || !props.documentId || !meta || !origins.data.value) return null;
  const task = originTask.value ? originTaskDetail.data.value : null;
  if (originTask.value && task?.id !== originTask.value) return null;
  return {
    workspaceId: sourceWorkspace.value,
    documentId: props.documentId,
    documentVersion: meta.version,
    taskId: task?.id ?? null,
    taskVersion: task?.version ?? null,
    taskProjectId: task?.projectId ?? null,
  };
});
const teams = computed(() =>
  (workspaces.data.value?.items ?? []).filter((workspace) => workspace.kind === "team"),
);
const team = computed(() => teams.value.find((workspace) => workspace.id === teamId.value));
const projects = useQuery(() => projectsQuery(open.value ? teamId.value : ""));
const editableProjects = computed(() =>
  (projects.data.value?.items ?? []).filter(
    (project) => project.canEdit && project.status !== "archived" && project.rootDocumentId,
  ),
);
const project = computed(() =>
  editableProjects.value.find((candidate) => candidate.id === projectId.value),
);
const withTask = computed(() => Boolean(source.value?.taskId));
const workflow = useQuery(() =>
  workflowQuery(open.value && withTask.value ? teamId.value : "", projectId.value),
);
// A WIP-limited column needs a reservation the server refuses to skip.
const statuses = computed(() =>
  (workflow.data.value?.statuses ?? []).filter((status) => status.wipLimit === null),
);
const ready = computed(
  () =>
    Boolean(source.value && team.value && project.value) &&
    (!withTask.value || statuses.value.some((status) => status.id === statusId.value)),
);
// Recovery visibility follows the stored command, also outside an open dialog.
const recoverable = ref(false);
function refreshRecoverable(): void {
  recoverable.value =
    !retired.value && recoverTransfer(window.sessionStorage, actor.value, session.value) !== null;
}
window.addEventListener(TRANSFER_COMMAND_EVENT, refreshRecoverable);
watch(
  [
    actor,
    session,
    retired,
    () => props.workspaceId,
    () => props.documentId ?? "",
    () => source.value?.taskId ?? "",
    open,
  ],
  ([nextActor, nextSession, isRetired, workspace, document, task]) => {
    // Actor/credential change retires every captured request; a target
    // change (including A-B-A) settles only the same actor's caches.
    scope.bind(
      isRetired ? "" : nextActor,
      `${workspace}:${document}:${task}:${String(++lifetime)}`,
      nextSession,
    );
    preview.value = null;
    result.value = null;
    error.value = null;
    busy.value = false;
    pending.value =
      open.value && !isRetired
        ? recoverTransfer(window.sessionStorage, nextActor, nextSession)
        : null;
    refreshRecoverable();
  },
  { immediate: true, flush: "sync" },
);
watch(teamId, () => {
  projectId.value = "";
});
// Status IDs belong to one project's workflow, so a choice survives only while
// the loaded workflow still contains it; otherwise default to its backlog.
watch(
  [projectId, statuses],
  ([, next]) => {
    if (!next.some((status) => status.id === statusId.value))
      statusId.value =
        (next.find((status) => status.category === "backlog") ?? next.at(0))?.id ?? "";
  },
  { immediate: true },
);
// Changing a reviewed choice invalidates its digest; a new review is required.
watch([action, teamId, projectId, statusId], () => {
  preview.value = null;
});
onScopeDispose(() => {
  scope.dispose();
  window.removeEventListener(TRANSFER_COMMAND_EVENT, refreshRecoverable);
});
function close(): void {
  open.value = false;
}
function failureMessage(failure: unknown): string {
  const keys = transferFailureKeys(failure);
  return keys ? keys.map((key) => t(key)).join(" ") : loadErrorMessage(failure);
}
/** What each observed part of the disclosure graph is, in the preview's words. */
function dispositionItem(disposition: TransferDisposition, value: TransferPreview): string {
  switch (disposition.item) {
    case "document":
      return value.documentTitle;
    case "task":
      return value.taskTitle ?? "";
    case "activity":
      return t("personalTransfer.activities", { count: disposition.count });
    case "history":
      return t("personalTransfer.history", { count: disposition.count });
    case "attachment":
      return t("personalTransfer.files", { count: disposition.count });
    case "time_entry":
      return t("personalTransfer.timeEntries", { count: disposition.count });
    case "timer":
      return t("personalTransfer.timers", { count: disposition.count });
  }
}
/** A MOVE carries time: entries become visible to the destination, timers stay the actor's own. */
function movesTime(value: TransferPreview): boolean {
  return value.dispositions.some(
    (disposition) =>
      (disposition.item === "time_entry" || disposition.item === "timer") &&
      disposition.outcome === "moved",
  );
}
async function prepared(captured: ReturnType<typeof scope.capture>): Promise<boolean> {
  const clean = props.prepare ? await props.prepare() : true;
  if (!scope.current(captured)) return false;
  if (!clean) error.value = t("personalTransfer.draft");
  return clean;
}
async function review(): Promise<void> {
  const current = source.value;
  if (busy.value || pending.value || !current || !ready.value || !team.value || !project.value)
    return;
  const captured = scope.capture();
  if (!captured.actor) return;
  busy.value = true;
  error.value = null;
  try {
    if (!(await prepared(captured))) return;
    const selection = buildSelection(current, action.value, {
      workspaceId: team.value.id,
      projectId: project.value.id,
      statusId: withTask.value ? statusId.value : null,
    });
    const value = await previewPersonalTransfer(current.workspaceId, selection);
    if (scope.current(captured)) preview.value = { selection, value };
  } catch (failure) {
    if (scope.current(captured)) error.value = failureMessage(failure);
  } finally {
    if (scope.current(captured)) busy.value = false;
  }
}
/** Cancel after a preview: the preview wrote nothing, so nothing is undone. */
function cancelReview(): void {
  if (busy.value) return;
  preview.value = null;
  error.value = null;
}
/**
 * A per-operation fence on the authoritative identity: the app's `me` cache
 * entry, which the shell keeps current after this dialog's own observers stop.
 * From before the request until the operation ends, a 401, the entry's
 * removal, or any value other than the command's actor and session fences the
 * operation for good, even if the original identity later returns
 * (A -> B -> A, or a 401 that is restored).
 */
function identityFence(command: PendingTransferCommand): { fenced(): boolean; stop(): void } {
  let fenced = false;
  const key = JSON.stringify(meQuery.queryKey);
  const check = () => {
    const state = client.getQueryState(meQuery.queryKey);
    const current = client.getQueryData(meQuery.queryKey);
    if (
      !state ||
      (state.error instanceof ProblemError && state.error.status === 401) ||
      current?.userId !== command.actorId ||
      current.sessionId !== command.sessionId
    )
      fenced = true;
  };
  check();
  const stop = client.getQueryCache().subscribe((event) => {
    if (fenced || JSON.stringify(event.query.queryKey) !== key) return;
    if (event.type === "removed") fenced = true;
    else check();
  });
  return { fenced: () => fenced, stop };
}
/**
 * Whether this command's durable bookkeeping (cache settlement, then removing
 * the stored command) may continue: the identity fence has never tripped, and
 * the scope is live or merely disposed (a MOVE can retire the source page that
 * hosts this dialog during settlement; that is not an identity change).
 */
function owns(
  captured: ReturnType<typeof scope.capture>,
  fence: ReturnType<typeof identityFence>,
): boolean {
  return !fence.fenced() && (scope.sameActor(captured) || scope.sameIdentity(captured));
}
async function dispatch(command: PendingTransferCommand): Promise<void> {
  const captured = scope.capture();
  if (!captured.actor || captured.actor !== command.actorId) return;
  const fence = identityFence(command);
  busy.value = true;
  error.value = null;
  try {
    // Throws when storage is unavailable: no request without a durable retry key.
    rememberTransfer(window.sessionStorage, command);
    if (scope.current(captured)) pending.value = command;
    const value = await confirmPersonalTransfer(command.sourceWorkspaceId, command.body);
    if (!owns(captured, fence)) return;
    // Settle the captured scopes first: if that fails the stored command
    // survives, and its identical replay settles them again.
    await settleCaches(command, value);
    if (!owns(captured, fence)) return;
    forgetTransfer(window.sessionStorage, command);
    if (scope.current(captured)) {
      result.value = { command, value };
      pending.value = null;
      preview.value = null;
    }
  } catch (failure) {
    if (scope.current(captured)) error.value = failureMessage(failure);
  } finally {
    fence.stop();
    if (scope.current(captured)) busy.value = false;
  }
}
async function confirm(): Promise<void> {
  const current = source.value;
  const reviewed = preview.value;
  if (busy.value || pending.value || !current || !reviewed || !team.value || !project.value) return;
  const captured = scope.capture();
  if (!captured.actor || !session.value) return;
  busy.value = true;
  error.value = null;
  // Unsaved edits after the review would be lost by a MOVE; a saved change
  // alters the digest and the server answers a conflict instead of publishing.
  const clean = await prepared(captured);
  if (!scope.current(captured)) return;
  busy.value = false;
  if (!clean) return;
  await dispatch({
    actorId: captured.actor,
    sessionId: session.value,
    sourceWorkspaceId: current.workspaceId,
    sourceTaskProjectId: current.taskProjectId,
    destinationSlug: team.value.slug,
    destinationProjectKey: project.value.key,
    body: {
      requestId: crypto.randomUUID(),
      confirmed: true,
      previewDigest: reviewed.value.digest,
      selection: reviewed.selection,
    },
  });
}
async function retry(): Promise<void> {
  if (busy.value || !pending.value) return;
  await dispatch(pending.value);
}
function abandon(): void {
  if (busy.value || !pending.value || !window.confirm(t("personalTransfer.abandonConfirm"))) return;
  forgetTransfer(window.sessionStorage, pending.value);
  pending.value = null;
  error.value = null;
}
async function settleCaches(command: PendingTransferCommand, value: TransferResult): Promise<void> {
  const selection = command.body.selection;
  const from = command.sourceWorkspaceId;
  // Captured at confirmation: a recovery-only replay has no source to read.
  // Commands stored before that field existed use the cached task row or a
  // retained list that positively holds the task. Otherwise no retained
  // project-scoped list shows it, and every project-independent source
  // family is settled by exact key (loaded pages kept); nothing is guessed.
  const retained = client
    .getQueryCache()
    .findAll()
    .map((query) => ({ queryKey: query.queryKey, data: query.state.data }));
  const fromProject = selection.taskId
    ? (command.sourceTaskProjectId ??
      client.getQueryData<TaskDetail>(["task", from, selection.taskId])?.projectId ??
      retainedTaskProject(retained, from, selection.taskId))
    : null;
  await Promise.all([
    client.invalidateQueries({ queryKey: ["tree", value.workspaceId] }),
    client.invalidateQueries({ queryKey: ["wiki-discovery", value.workspaceId] }),
    value.taskId
      ? invalidateTaskCaches(
          client,
          value.workspaceId,
          value.projectId,
          value.taskId,
          value.documentId,
        )
      : Promise.resolve(),
    // COPY leaves the private source untouched; only a MOVE removes it there.
    ...(selection.action === "move"
      ? [
          client.invalidateQueries({ queryKey: ["tree", from] }),
          client.invalidateQueries({ queryKey: ["wiki-discovery", from] }),
          selection.taskId && !fromProject
            ? Promise.all(
                unscopedSourceKeys(retained, from, selection.taskId, selection.documentId).map(
                  (queryKey) => settleExactKeepingLoadMore(client, queryKey),
                ),
              )
            : Promise.resolve(),
          selection.taskId && fromProject
            ? invalidateTaskCaches(
                client,
                from,
                fromProject,
                selection.taskId,
                selection.documentId,
              )
            : Promise.resolve(),
        ]
      : []),
  ]);
}
function visit(path: string | null): void {
  if (!path) return;
  open.value = false;
  router.push(path).catch((failure: unknown) => {
    error.value = loadErrorMessage(failure);
  });
}
const paths = computed(() =>
  result.value ? resultPaths(result.value.command, result.value.value) : null,
);
const fieldClass =
  "w-full rounded-md border border-default bg-default p-2 text-base leading-relaxed";
</script>
<template>
  <UButton
    v-if="source"
    size="sm"
    color="neutral"
    variant="outline"
    aria-haspopup="dialog"
    :aria-expanded="open"
    :aria-controls="open ? id : undefined"
    @click="open = true"
    >{{ t("personalTransfer.open") }}</UButton
  >
  <UButton
    v-else-if="recoverable && documentId === null"
    size="sm"
    color="neutral"
    variant="outline"
    aria-haspopup="dialog"
    :aria-expanded="open"
    :aria-controls="open ? id : undefined"
    @click="open = true"
    >{{ t("personalTransfer.recover") }}</UButton
  >
  <NativeModal
    :id="id"
    :open="open"
    :labelled-by="`${id}-title`"
    dialog-class="mx-auto mt-[10vh] w-[min(36rem,calc(100%-2rem))] max-h-[80dvh] overflow-auto rounded-lg border border-default bg-default p-4 text-default backdrop:bg-black/30"
    @close="close"
  >
    <div class="flex flex-col gap-4 break-keep">
      <h2 :id="`${id}-title`" class="text-xl font-semibold">{{
        pending ? t("personalTransfer.recover") : t("personalTransfer.open")
      }}</h2>
      <div v-if="result" class="flex flex-col gap-3">
        <p role="status" class="text-base leading-relaxed">{{ t("personalTransfer.saved") }}</p>
        <p class="text-base leading-relaxed">{{
          result.command.body.selection.action === "copy"
            ? t("personalTransfer.sourceRetained")
            : t("personalTransfer.sourceRemoved")
        }}</p>
        <div class="flex flex-wrap gap-2">
          <UButton @click="visit(paths?.task ?? paths?.document ?? null)">{{
            t("personalTransfer.visit")
          }}</UButton>
          <UButton color="neutral" variant="ghost" @click="close">{{ t("capture.close") }}</UButton>
        </div>
      </div>
      <div v-else-if="pending" class="flex flex-col gap-3">
        <p role="status" class="text-base leading-relaxed">{{ t("personalTransfer.unknown") }}</p>
        <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-base leading-relaxed">
          <dt class="text-sm font-medium text-muted">{{ t("personalTransfer.action") }}</dt>
          <dd>{{
            pending.body.selection.action === "copy"
              ? t("personalTransfer.copy")
              : t("personalTransfer.move")
          }}</dd>
          <dt class="text-sm font-medium text-muted">{{ t("personalTransfer.workspace") }}</dt>
          <dd class="break-all">{{ pending.destinationSlug }}</dd>
        </dl>
        <p v-if="error" role="alert" class="text-base leading-relaxed">{{ error }}</p>
        <div class="flex flex-wrap gap-2">
          <UButton :disabled="busy" @click="retry">{{
            busy ? t("personalTransfer.executing") : t("personalTransfer.retry")
          }}</UButton>
          <UButton color="neutral" variant="outline" :disabled="busy" @click="abandon">{{
            t("capture.abandon")
          }}</UButton>
          <UButton color="neutral" variant="ghost" @click="close">{{ t("capture.close") }}</UButton>
        </div>
      </div>
      <div v-else-if="preview && source" class="flex flex-col gap-3">
        <h3 class="text-base font-semibold">{{ t("personalTransfer.review") }}</h3>
        <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-base leading-relaxed">
          <dt class="text-sm font-medium text-muted">{{ t("personalTransfer.action") }}</dt>
          <dd>{{
            preview.selection.action === "copy"
              ? t("personalTransfer.copy")
              : t("personalTransfer.move")
          }}</dd>
          <dt class="text-sm font-medium text-muted">{{ t("personalTransfer.workspace") }}</dt>
          <dd>{{ preview.value.workspaceName }}</dd>
          <dt class="text-sm font-medium text-muted">{{ t("personalTransfer.project") }}</dt>
          <dd>{{ preview.value.projectName }}</dd>
        </dl>
        <p class="text-base leading-relaxed">{{
          t(audienceKey(preview.value.projectVisibility))
        }}</p>
        <div class="flex flex-col gap-1">
          <p class="text-sm font-medium">{{ t("personalTransfer.includes") }}</p>
          <!-- The server's observed disclosure graph and what happens to each part. -->
          <dl class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-1 text-base leading-relaxed">
            <template
              v-for="disposition in preview.value.dispositions"
              :key="`${disposition.item}:${disposition.outcome}`"
            >
              <dt class="break-words">{{ dispositionItem(disposition, preview.value) }}</dt>
              <dd class="text-sm text-muted" :data-outcome="disposition.outcome">{{
                t(OUTCOME_KEYS[disposition.outcome])
              }}</dd>
            </template>
          </dl>
        </div>
        <p v-if="movesTime(preview.value)" class="text-base leading-relaxed">{{
          t("personalTransfer.timePrivacy")
        }}</p>
        <p v-if="preview.value.taskTitle" class="text-base leading-relaxed">{{
          t("personalTransfer.preserveAssignment")
        }}</p>
        <p class="text-base leading-relaxed">{{
          preview.value.sourceRetained
            ? t("personalTransfer.sourceRetained")
            : t("personalTransfer.sourceRemoved")
        }}</p>
        <p v-if="error" role="alert" class="text-base leading-relaxed">{{ error }}</p>
        <div class="flex flex-wrap gap-2">
          <UButton :disabled="busy" @click="confirm">{{
            busy
              ? t("personalTransfer.executing")
              : preview.selection.action === "copy"
                ? t("personalTransfer.copy")
                : t("personalTransfer.move")
          }}</UButton>
          <UButton color="neutral" variant="ghost" :disabled="busy" @click="cancelReview">{{
            t("capture.cancel")
          }}</UButton>
        </div>
      </div>
      <form v-else-if="source" class="flex flex-col gap-4" @submit.prevent="review">
        <fieldset :disabled="busy" class="flex flex-col gap-3">
          <legend class="mb-2 text-sm font-medium">{{ t("personalTransfer.action") }}</legend>
          <label
            v-for="choice in ['copy', 'move'] as const"
            :key="choice"
            class="flex items-start gap-2"
          >
            <input
              v-model="action"
              class="mt-1.5"
              type="radio"
              :name="`${id}-action`"
              :value="choice"
              :aria-describedby="`${id}-${choice}-description`"
            />
            <span class="flex flex-col">
              <span class="text-base font-medium">{{ t(`personalTransfer.${choice}`) }}</span>
              <span :id="`${id}-${choice}-description`" class="text-sm leading-normal text-muted">{{
                t(`personalTransfer.${choice}Description`)
              }}</span>
            </span>
          </label>
        </fieldset>
        <div class="flex flex-col gap-1">
          <label :for="`${id}-team`" class="text-sm font-medium">{{
            t("personalTransfer.workspace")
          }}</label>
          <select :id="`${id}-team`" v-model="teamId" :disabled="busy" :class="fieldClass">
            <option value="" disabled></option>
            <option v-for="workspace in teams" :key="workspace.id" :value="workspace.id">{{
              workspace.name
            }}</option>
          </select>
        </div>
        <div class="flex flex-col gap-1">
          <label :for="`${id}-project`" class="text-sm font-medium">{{
            t("personalTransfer.project")
          }}</label>
          <select
            :id="`${id}-project`"
            v-model="projectId"
            :disabled="busy || !teamId"
            :class="fieldClass"
          >
            <option value="" disabled></option>
            <option
              v-for="candidate in editableProjects"
              :key="candidate.id"
              :value="candidate.id"
              >{{ candidate.name }}</option
            >
          </select>
        </div>
        <div v-if="withTask" class="flex flex-col gap-1">
          <label :for="`${id}-status`" class="text-sm font-medium">{{
            t("personalTransfer.status")
          }}</label>
          <select
            :id="`${id}-status`"
            v-model="statusId"
            :disabled="busy || !projectId"
            :class="fieldClass"
          >
            <option v-for="status in statuses" :key="status.id" :value="status.id">{{
              status.name
            }}</option>
          </select>
        </div>
        <p
          v-if="
            (workspaces.isSuccess.value && teams.length === 0) ||
            (teamId && projects.isSuccess.value && editableProjects.length === 0)
          "
          role="status"
          class="text-base leading-relaxed"
          >{{ t("personalTransfer.targetMissing") }}</p
        >
        <p v-if="error" role="alert" class="text-base leading-relaxed">{{ error }}</p>
        <div class="flex flex-wrap gap-2">
          <UButton type="submit" :disabled="busy || !ready">{{
            busy ? t("personalTransfer.loading") : t("personalTransfer.review")
          }}</UButton>
          <UButton color="neutral" variant="ghost" @click="close">{{ t("capture.close") }}</UButton>
        </div>
      </form>
    </div>
  </NativeModal>
</template>
