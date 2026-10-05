<script setup lang="ts">
import { FvociEditor, type TiptapEditor } from "@fvoci/editor/vue";
import { onBeforeRouteLeave, onBeforeRouteUpdate } from "vue-router";
import { UNIQUE_ID_NODE_TYPES } from "@fvoci/editor/extract";
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
import { projectDocumentsQuery } from "@/features/projects/queries";
import { compareRevisionProjections } from "@/features/documents/revision-diff";
import { documentPath, wikiDisplayId } from "@/lib/href";
import type { OffDraftCreateResponse } from "@/features/documents/document-api";
import type { useOffWikiBody } from "../../composables/useOffWikiBody";
import { loadErrorMessage, ProblemError } from "@/lib/api";
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
  offBody: ReturnType<typeof useOffWikiBody> | null;
  collabUser: CollabUser | null;
}>();

const realtimeOff = computed(() => !!props.offBody);
const offBody = computed(() => props.offBody);
const bodyDoc = computed(() => (props.offBody ? props.offBody.doc.value : props.session?.doc));
const bodyGeneration = computed(() =>
  props.offBody ? `off:${props.offBody.generation.value}` : props.session?.generation,
);
const offComparisons = computed(() => {
  const bodies = props.offBody?.comparison.value;
  if (!bodies) return [];
  const projection = (id: string, contentJson: unknown) => ({
    id,
    targetKind: "task",
    targetId: props.taskId,
    contentJson,
  });
  return [
    compareRevisionProjections(
      projection("start", bodies.start),
      projection("mine", bodies.mine),
      UNIQUE_ID_NODE_TYPES,
    ),
    compareRevisionProjections(
      projection("start", bodies.start),
      projection("current", bodies.current),
      UNIQUE_ID_NODE_TYPES,
    ),
  ];
});

const { mentionItems, entityResolver } = useEditorEntities(
  () => props.workspaceId,
  () =>
    `${props.taskId}:${String(props.session?.generation ?? "")}:${props.collabUser?.id ?? ""}:${String(props.session?.status === "unauthorized")}`,
);

const persisting = ref(false);
const persistError = ref<string | null>(null);

const readOnly = computed(
  () =>
    props.readOnly ||
    (props.offBody
      ? !props.offBody.doc.value || !props.offBody.writable.value
      : (props.session?.readOnly ?? false)),
);
const ready = computed(() =>
  Boolean(props.collabUser && (props.offBody ? props.offBody.doc.value : props.session?.synced)),
);
const refusalNote = computed(() =>
  props.offBody ? null : collabRefusalNote(props.session?.status, ready.value),
);
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
    (props.offBody
      ? !props.offBody.saving.value && !props.offBody.conflict.value
      : props.session !== null && props.session.status === "connected") &&
    !persisting.value,
);

const UrlEmbed: FunctionalComponent<{ url: string }> = markRaw((embed: { url: string }) =>
  h(UnfurlCard, { workspaceId: props.workspaceId, url: embed.url }),
);
UrlEmbed.props = ["url"];

const me = useQuery(meQuery);
const persistLifecycle = ref(0);
const copyTitle = ref("");
const copiedDraft = shallowRef<OffDraftCreateResponse | null>(null);
const copyDestination = ref<"wiki" | "project">("project");
const copyParentId = ref("");
const copyProjectId = computed(() => props.offBody?.draft.value?.owner.projectId ?? "");
const copyTree = useQuery(() => ({
  ...projectDocumentsQuery(props.workspaceId, copyProjectId.value),
  enabled:
    !!props.offBody?.comparison.value &&
    copyDestination.value === "project" &&
    !!copyProjectId.value &&
    !props.offBody.pendingDistinct.value,
}));
const copyTargets = computed(() =>
  (copyTree.data.value?.items ?? []).filter((node) => node.projectId === copyProjectId.value),
);
watch(
  persistLifecycle,
  () => {
    copyTitle.value = "";
    copyDestination.value = "project";
    copyParentId.value = "";
    copiedDraft.value = null;
  },
  { flush: "sync" },
);
watch(copyTargets, (nodes) => {
  if (!copyParentId.value && !props.offBody?.pendingDistinct.value)
    copyParentId.value = nodes.find((node) => node.parentId === null)?.id ?? "";
});
watch(
  () => props.offBody?.pendingDistinct.value,
  (pending) => {
    if (!pending) return;
    copyDestination.value = pending.projectId ? "project" : "wiki";
    copyParentId.value = pending.body.parentId ?? "";
    copyTitle.value = pending.body.title;
  },
  { immediate: true },
);
async function copyOffDraft(): Promise<void> {
  const lifetime = persistLifecycle.value;
  const current = props.offBody;
  if (
    !current ||
    (!current.pendingDistinct.value &&
      (sourceDraft.value?.dirty || sourceDraft.value?.composing)) ||
    (copyDestination.value === "project" &&
      (!copyProjectId.value || !copyParentId.value) &&
      !current.pendingDistinct.value)
  )
    return;
  const copied = await current.createDistinct({
    projectId: copyDestination.value === "project" ? copyProjectId.value : null,
    parentId: copyDestination.value === "project" ? copyParentId.value || null : null,
    title: copyTitle.value.trim() || t("doc.duplicate.title"),
  });
  if (lifetime !== persistLifecycle.value || props.offBody !== current || !copied) return;
  copiedDraft.value = copied;
}

watch(
  [
    () => props.workspaceId,
    () => props.taskId,
    () => props.collabUser?.id,
    () => me.data.value?.sessionId,
    () => me.error.value instanceof ProblemError && me.error.value.status === 401,
    () => bodyDoc.value,
    () => props.session?.provider,
    () => bodyGeneration.value,
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
  if (props.offBody) {
    const current = props.offBody.draft.value;
    if (!current || sourceAuthRetired.value) return false;
    if (!readOnly.value) await persistBody();
    return (
      (await props.offBody.verifyCommitted()) &&
      current === props.offBody.draft.value &&
      current.active &&
      !sourceAuthRetired.value
    );
  }
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
  if (props.offBody) {
    if (!canPersist.value || sourceAuthRetired.value) throw new Error("Body save unavailable");
    if (!(await props.offBody.save())) throw new Error("Body save unconfirmed");
    return;
  }
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
        v-if="offBody"
        class="document-page__collab-status"
        data-body-mode="off"
        :data-body-persisted="offBody.durable.value ? 'true' : 'false'"
      >
        {{
          !offBody.doc.value
            ? t("load.loading")
            : offBody.saving.value
              ? t("version.saving")
              : offBody.dirty.value || offBody.sourceBuffer.value
                ? t("doc.off.draft")
                : t("doc.off.saved")
        }}
      </span>
      <span
        v-else-if="badge"
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
        :after-restore="offBody?.load"
        :source-dirty="!!sourceDraft?.dirty || !!sourceDraft?.composing || !!offBody?.dirty.value"
      />
    </div>
    <p v-if="persistError" role="alert" class="document-page__error">{{ persistError }}</p>
    <p v-if="session?.status === 'unauthorized'" class="document-page__body-note" role="alert">
      {{ t("task.collab.unauthorized") }}
    </p>
    <p v-if="refusalNote" class="document-page__body-note" role="status">{{ t(refusalNote) }}</p>
    <p v-if="offBody && offBody.error.value" role="alert">{{
      loadErrorMessage(offBody.error.value)
    }}</p>
    <p v-if="offBody && offBody.storageError.value" role="alert">{{
      t("doc.off.storageFailed")
    }}</p>
    <UButton
      v-if="offBody && (!offBody.doc.value || !offBody.writable.value) && !offBody.loading.value"
      @click="offBody.load"
      >{{ t("load.retry") }}</UButton
    >
    <section
      v-if="offBody && offBody.comparison.value"
      :aria-label="t('version.compare')"
      data-testid="off-body-conflict"
    >
      <p role="alert">{{ t("doc.off.conflict") }}</p>
      <div v-for="(difference, index) in offComparisons" :key="difference.afterId">
        <h3>{{ index === 0 ? t("doc.off.mine") : t("doc.off.current") }}</h3>
        <p v-for="limit in difference.limits" :key="limit">{{ t(`version.diff.${limit}`) }}</p>
        <div v-for="change in difference.changes" :key="change.key">
          <p
            >{{ t(`version.diff.${change.kind}`) }} ·
            {{ (change.after ?? change.before)?.blockId }}</p
          >
          <del>{{ change.before?.text }}</del> → <ins>{{ change.after?.text }}</ins>
          <pre v-if="change.values">{{ JSON.stringify(change.values, null, 2) }}</pre>
        </div>
      </div>
      <details
        ><summary>{{ t("doc.off.start") }}</summary
        ><pre>{{ JSON.stringify(offBody.comparison.value.start, null, 2) }}</pre>
      </details>
      <details
        ><summary>{{ t("doc.off.mine") }}</summary
        ><pre>{{ JSON.stringify(offBody.comparison.value.mine, null, 2) }}</pre>
      </details>
      <details
        ><summary>{{ t("doc.off.current") }}</summary
        ><pre>{{ JSON.stringify(offBody.comparison.value.current, null, 2) }}</pre>
      </details>
      <UButton
        v-if="offBody.conflict.value"
        :disabled="offBody.saving.value || offBody.creating.value"
        @click="offBody.editCurrent"
        >{{ t("doc.off.editCurrent") }}</UButton
      >
      <label>
        {{ t("doc.duplicate.title") }}
        <input
          v-model="copyTitle"
          maxlength="300"
          :disabled="offBody.creating.value || !!offBody.pendingDistinct.value"
          :placeholder="t('doc.duplicate.title')"
        />
      </label>
      <label>
        {{ t("doc.move.parentLabel") }}
        <select
          v-model="copyDestination"
          :disabled="offBody.creating.value || !!offBody.pendingDistinct.value"
          :aria-label="t('doc.move.parentLabel')"
        >
          <option value="project">{{ t("wiki.projectWikis") }}</option>
          <option value="wiki">{{ t("nav.wiki") }}</option>
        </select>
        <select
          v-if="copyDestination === 'project'"
          v-model="copyParentId"
          :disabled="offBody.creating.value || !!offBody.pendingDistinct.value"
          :aria-label="t('doc.move.parentLabel')"
        >
          <option value="">{{ t("doc.move.parentLabel") }}</option>
          <option v-for="node in copyTargets" :key="node.id" :value="node.id">{{
            node.title
          }}</option>
        </select>
      </label>
      <p v-if="copyDestination === 'project' && copyTree.error.value" role="alert">{{
        loadErrorMessage(copyTree.error.value)
      }}</p>
      <UButton
        :disabled="
          offBody.creating.value ||
          offBody.saving.value ||
          (copyDestination === 'project' &&
            (!copyProjectId || !copyParentId) &&
            !offBody.pendingDistinct.value) ||
          (!offBody.pendingDistinct.value && (!!sourceDraft?.dirty || !!sourceDraft?.composing))
        "
        @click="copyOffDraft"
      >
        {{ offBody.creating.value ? t("doc.duplicate.pending") : t("doc.duplicate.submit") }}
      </UButton>
      <p>{{ t("doc.off.referenceAccess") }}</p>
      <p v-if="copiedDraft" role="status">
        {{ t("doc.duplicate.done") }}
        <a
          :href="
            documentPath(
              slug,
              copiedDraft.document.displayId || wikiDisplayId(copiedDraft.document.number),
            )
          "
          >{{ copiedDraft.document.title }}</a
        >
      </p>
    </section>
    <section
      v-if="offBody && !offBody.comparison.value && (offBody.pendingDistinct.value || copiedDraft)"
      class="document-off-compare"
      role="status"
    >
      <template v-if="offBody.pendingDistinct.value">
        <p>{{ t("doc.duplicate.pending") }}: {{ offBody.pendingDistinct.value.body.title }}</p>
        <UButton :disabled="offBody.creating.value || offBody.saving.value" @click="copyOffDraft">{{
          t("load.retry")
        }}</UButton>
      </template>
      <p v-if="copiedDraft">
        {{ t("doc.duplicate.done") }}
        <a
          :href="
            documentPath(
              slug,
              copiedDraft.document.displayId || wikiDisplayId(copiedDraft.document.number),
            )
          "
          >{{ copiedDraft.document.title }}</a
        >
      </p>
    </section>
    <QueryLoading v-if="!ready && session?.status !== 'unauthorized' && !refusalNote" />
    <div
      v-if="ready && bodyDoc && collabUser"
      class="document-page__body document-page__body--editor"
    >
      <FvociEditor
        ref="sourceEditor"
        :key="bodyGeneration"
        :mode-scope="persistLifecycle"
        :wait-for-save="waitForEditorSave"
        :ydoc="bodyDoc"
        :provider="session?.provider"
        :source-buffer="offBody?.sourceBuffer.value ?? null"
        :user="collabUser"
        :editable="!readOnly"
        :aria-label="t('doc.body.a11y')"
        :workspace-slug="slug"
        :mention-items="mentionItems"
        :entity-resolver="entityResolver"
        :url-embed="UrlEmbed"
        @source-dirty="onSourceDraft"
        @source-buffer="offBody?.receiveSourceBuffer($event)"
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
