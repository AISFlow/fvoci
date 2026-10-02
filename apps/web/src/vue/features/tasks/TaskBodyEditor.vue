<script setup lang="ts">
import { FvociEditor, type TiptapEditor } from "@fvoci/editor/vue";
import { onBeforeRouteLeave, onBeforeRouteUpdate } from "vue-router";
import "@fvoci/editor/styles.css";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import {
  computed,
  type FunctionalComponent,
  h,
  markRaw,
  onScopeDispose,
  ref,
  shallowRef,
  watch,
} from "vue";
import { useQuery } from "@tanstack/vue-query";
import { meQuery } from "@/lib/queries";
import { ProblemError } from "@/lib/api";
import { collabBadge, collabRefusalNote } from "@/features/documents/collab-badge";
import type { CollabUser } from "@/features/documents/collab-model";
import type { CollabRoomSession } from "../../collab/useCollabRoom";
import QueryLoading from "../../components/QueryLoading.vue";
import NativeModal from "../../components/NativeModal.vue";
import { useSourceDraftGuard } from "../../composables/useSourceDraftGuard";
import { useReadonlyCommittedBody } from "../../composables/useReadonlyCommittedBody";
import EditorControls from "../editor/EditorControls.vue";
import TemplateToolbar from "../editor/TemplateToolbar.vue";
import { useEditorEntities } from "../editor/useEditorEntities";
import UnfurlCard from "../editor/UnfurlCard.vue";
import CollabPresence from "../documents/CollabPresence.vue";
import RevisionPanel from "../documents/RevisionPanel.vue";
import "@/features/documents/document-shell.css";

// The task body is the `${ws}:task:${id}` collab room (React task-body-editor).
// The parent owns the single useCollabRoom (TaskDetailView), keyed by the room name.
const props = defineProps<{
  workspaceId: string;
  slug: string;
  taskId: string;
  readOnly: boolean;
  session: CollabRoomSession | null;
  collabUser: CollabUser | null;
}>();

const { mentionItems, entityResolver } = useEditorEntities(
  () => props.workspaceId,
  () =>
    `${props.taskId}:${String(props.session?.generation ?? "")}:${props.collabUser?.id ?? ""}:${String(props.session?.status === "unauthorized")}`,
);

const persisting = ref(false);
const persistError = ref<string | null>(null);

const readOnly = computed(() => props.readOnly || (props.session?.readOnly ?? false));
const ready = computed(() => Boolean(props.session?.synced && props.collabUser));
const refusalNote = computed(() => collabRefusalNote(props.session?.status, ready.value));
const badge = computed(() =>
  props.session
    ? collabBadge(
        props.session.status,
        props.session.pending || persisting.value,
        props.session.durableSaved,
      )
    : null,
);
const canPersist = computed(
  () =>
    ready.value &&
    !readOnly.value &&
    props.session !== null &&
    props.session.status === "connected" &&
    !persisting.value,
);

const UrlEmbed: FunctionalComponent<{ url: string }> = markRaw((embed: { url: string }) =>
  h(UnfurlCard, { workspaceId: props.workspaceId, url: embed.url }),
);
UrlEmbed.props = ["url"];

const me = useQuery(meQuery);
const persistLifecycle = ref(0);
watch(
  [
    () => props.workspaceId,
    () => props.taskId,
    () => props.collabUser?.id,
    () => me.data.value?.sessionId,
    () => me.error.value instanceof ProblemError && me.error.value.status === 401,
    () => props.session?.doc,
    () => props.session?.provider,
    () => props.session?.generation,
    () => props.session?.status,
    readOnly,
  ],
  () => {
    persistLifecycle.value++;
    persisting.value = false;
    persistError.value = null;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  persistLifecycle.value++;
});

const richEditor = shallowRef<TiptapEditor | null>(null);
const sourceEditor = shallowRef<InstanceType<typeof FvociEditor> | null>(null);
const sourceDraftDialogId = computed(() => `source-draft-leave-${props.taskId}`);
const {
  open: sourceLeaveOpen,
  authRetired: sourceAuthRetired,
  draft: sourceDraft,
  receive: onSourceDraft,
  requestLeave,
  keepEditing,
  discardAndLeave,
} = useSourceDraftGuard({
  scope: () => persistLifecycle.value,
  identity: () =>
    `${props.workspaceId}:${props.taskId}:${me.data.value?.userId ?? ""}:${me.data.value?.sessionId ?? ""}`,
  authorized: () =>
    !!me.data.value?.userId &&
    !!me.data.value.sessionId &&
    props.session?.status !== "unauthorized" &&
    !(me.error.value instanceof ProblemError && me.error.value.status === 401),
  editor: () => sourceEditor.value,
});
onBeforeRouteLeave(() => requestLeave());
onBeforeRouteUpdate((to, from) => (to.path === from.path ? true : requestLeave()));

/** W3 copy/mode barrier uses the existing matched persist owner, then checks
 * the same live resource/actor/provider generation; ACK snapshots may replace. */
function readSaveSession() {
  return props.session;
}
function readAuthRetired() {
  return sourceAuthRetired.value;
}
function readSaveActor() {
  return me.data.value;
}
const readonlyCommittedBody = useReadonlyCommittedBody(() => {
  const current = readSaveSession();
  const actor = readSaveActor();
  if (!current || !actor?.userId || !actor.sessionId) return null;
  return {
    workspaceId: props.workspaceId,
    targetId: props.taskId,
    kind: "task",
    schema: richEditor.value?.schema ?? null,
    projectId: null,
    actorId: actor.userId,
    credentialId: actor.sessionId,
    lifetime: persistLifecycle.value,
    doc: current.doc,
    provider: current.provider,
    generation: current.generation,
    connected: current.status === "connected",
    synced: current.synced,
    pending: current.pending,
    allowed:
      readOnly.value &&
      !sourceAuthRetired.value &&
      !(me.error.value instanceof ProblemError && me.error.value.status === 401),
  };
});
async function waitForEditorSave(): Promise<boolean> {
  const before = readSaveSession();
  const lifetime = persistLifecycle.value;
  const target = `${props.workspaceId}:${props.taskId}`;
  const actor = me.data.value?.userId;
  const credential = me.data.value?.sessionId;
  if (
    !before ||
    before.status !== "connected" ||
    !before.synced ||
    !actor ||
    sourceAuthRetired.value
  )
    return false;
  try {
    const readonly = readOnly.value;
    const committedRead = readonly ? await readonlyCommittedBody() : false;
    if (readonly && !committedRead) return false;
    if (!readonly) await persistBody();
    const current = readSaveSession();
    const currentActor = readSaveActor();
    return (
      lifetime === persistLifecycle.value &&
      target === `${props.workspaceId}:${props.taskId}` &&
      actor === currentActor?.userId &&
      credential === currentActor.sessionId &&
      !!current &&
      current.status === "connected" &&
      current.synced &&
      current.doc === before.doc &&
      current.provider === before.provider &&
      current.generation === before.generation &&
      (readonly ? committedRead : current.durableSaved) &&
      !current.pending &&
      !readAuthRetired() &&
      !(me.error.value instanceof ProblemError && me.error.value.status === 401)
    );
  } catch {
    return false;
  }
}

async function persistBody(): Promise<void> {
  const current = props.session;
  if (!current || !canPersist.value) throw new Error("collab persist unavailable");
  const lifetime = persistLifecycle.value;
  persistError.value = null;
  persisting.value = true;
  try {
    await current.persistNow();
    if (lifetime !== persistLifecycle.value) throw new Error("collab persist scope retired");
  } catch (error) {
    if (lifetime === persistLifecycle.value) {
      const timedOut = error instanceof Error && error.message.includes("timed out");
      persistError.value = timedOut ? t("collab timeout — retry") : t("collab unavailable");
    }
    throw error;
  } finally {
    if (lifetime === persistLifecycle.value) persisting.value = false;
  }
}
</script>

<template>
  <section class="task-detail__body" :aria-label="t('doc.body.a11y')" data-testid="task-body">
    <div class="document-page__collab">
      <span
        v-if="badge"
        :class="`document-page__collab-status document-page__collab-status--${badge.tone}`"
        :data-collab-status="session?.status"
        :data-collab-pending="session?.pending ? 'true' : 'false'"
        :data-collab-persisted="session?.durableSaved ? 'true' : 'false'"
      >
        {{ t(badge.label) }}
      </span>
      <span
        v-else
        class="document-page__collab-status document-page__collab-status--wait"
        data-collab-persisted="false"
      >
        {{ t("doc.collab.connecting") }}
      </span>
      <UButton size="sm" :disabled="!canPersist" @click="persistBody().catch(() => undefined)">
        {{ persisting ? t("doc.title.saving") : t("doc.title.save") }}
      </UButton>
      <CollabPresence v-if="session" :peers="session.peers" />
      <RevisionPanel
        :workspace-id="workspaceId"
        :document-id="taskId"
        :project-id="null"
        target-kind="task"
        :read-only="readOnly"
        :persist-now="canPersist ? persistBody : undefined"
      />
    </div>
    <p v-if="persistError" role="alert" class="document-page__error">{{ persistError }}</p>
    <p v-if="session?.status === 'unauthorized'" class="document-page__body-note" role="alert">
      {{ t("task.collab.unauthorized") }}
    </p>
    <p v-if="refusalNote" class="document-page__body-note" role="status">{{ t(refusalNote) }}</p>
    <QueryLoading v-if="!ready && session?.status !== 'unauthorized' && !refusalNote" />
    <div
      v-if="ready && session && collabUser"
      class="document-page__body document-page__body--editor"
    >
      <FvociEditor
        ref="sourceEditor"
        :key="session.generation"
        :mode-scope="persistLifecycle"
        :wait-for-save="waitForEditorSave"
        :ydoc="session.doc"
        :provider="session.provider"
        :user="collabUser"
        :editable="!readOnly"
        :aria-label="t('doc.body.a11y')"
        :workspace-slug="slug"
        :mention-items="mentionItems"
        :entity-resolver="entityResolver"
        :url-embed="UrlEmbed"
        @source-dirty="onSourceDraft"
        @ready="richEditor = $event"
      >
        <template #toolbar="{ editor: live }">
          <TemplateToolbar :editor="live" mode="fixed" />
        </template>
        <template #bubble="{ editor: live }">
          <TemplateToolbar :editor="live" mode="selection" />
        </template>
        <template #controls="{ editor: live, gutter, editable }">
          <EditorControls :editor="live" :gutter="gutter" :editable="editable" />
        </template>
      </FvociEditor>
    </div>
  </section>
  <NativeModal :open="sourceLeaveOpen" :labelled-by="sourceDraftDialogId" @close="keepEditing">
    <h2 :id="sourceDraftDialogId">{{ t("editor.mode.leaveTitle") }}</h2>
    <p>{{ t("editor.mode.leaveDescription") }}</p>
    <div class="project-dialog__actions">
      <UButton color="neutral" variant="outline" @click="keepEditing">{{
        t("editor.mode.keepEditing")
      }}</UButton>
      <UButton :disabled="sourceDraft?.composing" @click="discardAndLeave">{{
        t("editor.mode.discardDraft")
      }}</UButton>
    </div>
  </NativeModal>
</template>
