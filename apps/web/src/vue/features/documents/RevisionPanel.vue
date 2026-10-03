<script setup lang="ts">
import { UNIQUE_ID_NODE_TYPES } from "@fvoci/editor/extract";
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, shallowRef, watch } from "vue";
import {
  authorLabel,
  createRevision,
  extractPreviewText,
  formatAt,
  getRevision,
  listRevisions,
  REASON_LABEL,
  restoreRevision,
  previewRestoreRevision,
  type RestorePreview,
  type RevisionDetail,
  type RevisionTargetKind,
} from "@/features/documents/revision-api";
import {
  compareRevisionProjections,
  type RevisionDiff,
  type RevisionChange,
  type RevisionProjection,
  type RevisionBlockView,
} from "@/features/documents/revision-diff";
import NativeModal from "../../components/NativeModal.vue";
import { persistThenCreate } from "@/features/documents/revision-persist";
import { ProblemError } from "@/lib/api";
import { meQuery, membersQuery } from "@/lib/queries";

// Server revisions are immutable history. Comparisons are observational;
// restore is one server-authored forward update, distinct from local undo.
const props = withDefaults(
  defineProps<{
    workspaceId: string;
    documentId: string;
    projectId: string | null;
    /** Documents (default) or the task body room. */
    targetKind?: "document" | "task";
    readOnly: boolean;
    persistNow?: (() => Promise<void>) | undefined;
    sourceDirty?: boolean;
  }>(),
  { targetKind: "document", persistNow: undefined, sourceDirty: false },
);
const queryClient = useQueryClient();
const me = useQuery(meQuery);
const timeZone = computed(() => me.data.value?.timezone || "Asia/Seoul");
const open = ref(false);
const notice = ref<string | null>(null);
const pendingRestoreId = ref<string | null>(null);
const preview = shallowRef<RevisionDetail | null>(null);
const correlations = new Map<string, { correlationId: string; expectedTailSeq: string }>();
const beforeId = ref("");
const afterId = ref("");
const comparison = shallowRef<RevisionDiff | null>(null);
const comparisonBodies = shallowRef<{ before: RevisionDetail; after: RevisionDetail } | null>(null);
const comparisonPending = ref(false);
const changeIndex = ref(0);
const restorePreview = shallowRef<RestorePreview | null>(null);
const restorePreviewPending = ref(false);
const restoreChangeIndex = ref(0);
let comparisonVersion = 0;
let dialogVersion = 0;
let dialogAbort = new AbortController();
const scopeProjectId = computed(() => (props.targetKind === "document" ? props.projectId : null));
const queryKey = computed(
  () =>
    [
      "revisions",
      props.workspaceId,
      props.targetKind,
      props.documentId,
      scopeProjectId.value,
      me.data.value?.userId ?? null,
      me.data.value?.sessionId ?? null,
    ] as const,
);

// Each lifetime has its own cancellation signal. Identity equality cannot
// resurrect an operation after A → B → A, even in one synchronous turn.
let epoch = 0;
let alive = true;
let scopeAbort = new AbortController();
let previewVersion = 0;
const savePending = ref(false);
const restorePending = ref(false);
const signedOut = computed(
  () => me.error.value instanceof ProblemError && me.error.value.status === 401,
);
const writable = computed(() => !props.readOnly && !!me.data.value && !signedOut.value);
function retireScope(): void {
  epoch++;
  scopeAbort.abort();
  scopeAbort = new AbortController();
  previewVersion++;
  comparisonVersion++;
  dialogVersion++;
  dialogAbort.abort();
  dialogAbort = new AbortController();
  beforeId.value = "";
  afterId.value = "";
  comparison.value = null;
  comparisonBodies.value = null;
  comparisonPending.value = false;
  restorePreview.value = null;
  restorePreviewPending.value = false;
  changeIndex.value = 0;
  restoreChangeIndex.value = 0;
  savePending.value = false;
  restorePending.value = false;
  pendingRestoreId.value = null;
  preview.value = null;
  notice.value = null;
  correlations.clear();
}
watch(
  [
    () => props.workspaceId,
    () => props.targetKind,
    () => props.documentId,
    scopeProjectId,
    () => props.readOnly,
    () => me.data.value?.userId,
    () => me.data.value?.sessionId,
    signedOut,
  ],
  retireScope,
  { flush: "sync" },
);
onScopeDispose(() => {
  alive = false;
  retireScope();
});
type RevisionScope = {
  workspaceId: string;
  kind: RevisionTargetKind;
  id: string;
  projectId: string | null;
  epoch: number;
  queryKey: readonly unknown[];
  signal: AbortSignal;
};
function captureScope(): RevisionScope {
  return {
    workspaceId: props.workspaceId,
    kind: props.targetKind,
    id: props.documentId,
    projectId: scopeProjectId.value,
    epoch,
    queryKey: queryKey.value,
    signal: scopeAbort.signal,
  };
}
const currentScope = (scope: RevisionScope) => alive && scope.epoch === epoch;
const listQuery = useQuery(() => {
  const scope = captureScope();
  return {
    queryKey: scope.queryKey,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      listRevisions(scope.kind, scope.workspaceId, scope.id, scope.projectId, signal),
    enabled: open.value && !!me.data.value && !signedOut.value,
  };
});
const members = useQuery(() => ({ ...membersQuery(props.workspaceId), enabled: open.value }));
const authorById = computed<Record<string, string>>(() =>
  Object.fromEntries(
    (members.data.value?.items ?? []).map((member) => [
      member.userId,
      formatPersonName(member, me.data.value?.locale),
    ]),
  ),
);
const items = computed(() => (signedOut.value ? [] : (listQuery.data.value?.items ?? [])));

function projection(detail: RevisionDetail): RevisionProjection {
  return {
    id: detail.id,
    targetId: detail.targetId,
    targetKind: detail.targetKind,
    contentJson: detail.contentJson,
  };
}
function assertTarget(detail: RevisionDetail, scope: RevisionScope, id: string): void {
  if (detail.id !== id || detail.targetId !== scope.id || detail.targetKind !== scope.kind)
    throw new Error("Revision target changed");
}
const restoreComparison = computed(() => {
  const current = restorePreview.value;
  if (!current) return null;
  return compareRevisionProjections(
    {
      id: `current:${current.currentTailSeq}`,
      targetId: current.source.targetId,
      targetKind: current.source.targetKind,
      contentJson: current.currentContentJson,
    },
    projection(current.source),
    UNIQUE_ID_NODE_TYPES,
  );
});
const activeChange = computed(() => comparison.value?.changes[changeIndex.value] ?? null);
const activeRestoreChange = computed(
  () => restoreComparison.value?.changes[restoreChangeIndex.value] ?? null,
);
const restorable = computed(
  () =>
    !!restorePreview.value &&
    !!restoreComparison.value &&
    !restoreComparison.value.limits.some((limit) =>
      ["invalid-content", "size-limit"].includes(limit),
    ),
);

watch([items, open], async ([revisions, expanded]) => {
  if (!expanded) {
    previewVersion++;
    comparisonVersion++;
    preview.value = null;
    comparison.value = null;
    comparisonBodies.value = null;
    comparisonPending.value = false;
    cancelRestore();
    return;
  }
  if (!revisions.length) return;
  const priorBefore = beforeId.value,
    priorAfter = afterId.value;
  if (!revisions.some((revision) => revision.id === beforeId.value))
    beforeId.value = revisions[1]?.id ?? revisions[0]?.id ?? "";
  if (!revisions.some((revision) => revision.id === afterId.value))
    afterId.value = revisions[0]?.id ?? "";
  if (priorBefore === beforeId.value && priorAfter === afterId.value) await compareSelected();
});
watch([beforeId, afterId], async () => {
  await compareSelected();
});
async function compareSelected(): Promise<void> {
  const scope = captureScope();
  const before = beforeId.value,
    after = afterId.value;
  const version = ++comparisonVersion;
  comparison.value = null;
  comparisonBodies.value = null;
  changeIndex.value = 0;
  if (!before || !after || !alive || signedOut.value) {
    comparisonPending.value = false;
    return;
  }
  comparisonPending.value = true;
  try {
    const [left, right] = await Promise.all([
      getRevision(scope.kind, scope.workspaceId, scope.id, scope.projectId, before, scope.signal),
      getRevision(scope.kind, scope.workspaceId, scope.id, scope.projectId, after, scope.signal),
    ]);
    assertTarget(left, scope, before);
    assertTarget(right, scope, after);
    if (!currentScope(scope) || version !== comparisonVersion) return;
    comparison.value = compareRevisionProjections(
      projection(left),
      projection(right),
      UNIQUE_ID_NODE_TYPES,
    );
    comparisonBodies.value = { before: left, after: right };
  } catch {
    if (currentScope(scope) && version === comparisonVersion)
      notice.value = t("version.compare.failed");
  } finally {
    if (currentScope(scope) && version === comparisonVersion) comparisonPending.value = false;
  }
}
function moveChange(direction: number, restoreDialog = false): void {
  const diff = restoreDialog ? restoreComparison.value : comparison.value;
  if (!diff?.changes.length) return;
  const index = restoreDialog ? restoreChangeIndex : changeIndex;
  index.value = Math.max(0, Math.min(diff.changes.length - 1, index.value + direction));
}
function blockPosition(block: RevisionBlockView | null): string {
  return block?.path.map((index) => String(index + 1)).join(" › ") ?? "";
}
function changeValues(change: RevisionChange, side: "before" | "after"): string {
  const values = change.values?.[side];
  if (change.kind === "checkbox")
    return Array.isArray(values) && values[0] === true
      ? t("version.diff.checked")
      : t("version.diff.unchecked");
  if (!Array.isArray(values)) return values === undefined ? "" : JSON.stringify(values);
  if (change.kind === "text" || change.kind === "table" || change.kind === "structure") return "";
  return values
    .map((value) => {
      if (!value || typeof value !== "object") return String(value);
      const entry = value as Record<string, unknown>;
      if (change.kind === "link" || change.kind === "reference" || change.kind === "attributes")
        return JSON.stringify(entry.attrs ?? value);
      if (change.kind === "format") return JSON.stringify(entry.marks ?? value);
      return JSON.stringify(value);
    })
    .join("\n");
}
function cancelRestore(): void {
  dialogVersion++;
  dialogAbort.abort();
  dialogAbort = new AbortController();
  pendingRestoreId.value = null;
  restorePreview.value = null;
  restorePreviewPending.value = false;
  // An enqueued server operation may still commit. Keep its correlation for
  // the existing dialog retry; cancellation never claims rollback or success.
}
watch(
  () => props.sourceDirty,
  (dirty) => {
    if (!dirty) return;
    cancelRestore();
    notice.value = t("version.restore.dirty");
  },
  { flush: "sync" },
);
function previewIsCurrent(scope: RevisionScope, version: number, signal: AbortSignal): boolean {
  return currentScope(scope) && version === dialogVersion && !props.sourceDirty && !signal.aborted;
}
async function beginRestore(id: string): Promise<void> {
  if (!alive || !writable.value || savePending.value || restorePending.value) return;
  if (props.sourceDirty || !props.persistNow) {
    notice.value = t("version.restore.dirty");
    return;
  }
  dialogVersion++;
  dialogAbort.abort();
  dialogAbort = new AbortController();
  const scope = captureScope();
  const version = dialogVersion;
  const signal = AbortSignal.any([scope.signal, dialogAbort.signal]);
  pendingRestoreId.value = id;
  restorePreview.value = null;
  restoreChangeIndex.value = 0;
  restorePreviewPending.value = true;
  notice.value = null;
  try {
    const detail = await persistThenCreate(props.persistNow, () => {
      if (!previewIsCurrent(scope, version, signal)) throw new Error("Restore preview retired");
      return previewRestoreRevision(
        scope.kind,
        scope.workspaceId,
        scope.id,
        scope.projectId,
        id,
        signal,
      );
    });
    assertTarget(detail.source, scope, id);
    if (!previewIsCurrent(scope, version, signal)) return;
    restorePreview.value = detail;
    correlations.set(id, {
      correlationId: crypto.randomUUID(),
      expectedTailSeq: detail.currentTailSeq,
    });
  } catch {
    if (currentScope(scope) && version === dialogVersion && !signal.aborted)
      notice.value = t("version.compare.failed");
  } finally {
    if (currentScope(scope) && version === dialogVersion) restorePreviewPending.value = false;
  }
}

const saveRevision = useMutation({
  mutationFn: (scope: RevisionScope & { persist: (() => Promise<void>) | undefined }) => {
    if (!currentScope(scope)) throw new Error("Revision scope retired");
    return persistThenCreate(scope.persist, () => {
      if (!currentScope(scope)) throw new Error("Revision scope retired");
      return createRevision(scope.kind, scope.workspaceId, scope.id, scope.projectId, scope.signal);
    });
  },
  onSuccess: async (_data, scope) => {
    if (!currentScope(scope)) return;
    notice.value = null;
    await queryClient.invalidateQueries({ queryKey: scope.queryKey });
  },
  onError: (_error, scope) => {
    if (currentScope(scope)) notice.value = t("version.save.failed");
  },
  onSettled: (_data, _error, scope) => {
    if (currentScope(scope)) savePending.value = false;
  },
});

type RestoreOperation = RevisionScope & {
  revisionId: string;
  correlationId: string;
  expectedTailSeq: string;
  dialogVersion: number;
  persist: (() => Promise<void>) | undefined;
};
const restore = useMutation({
  mutationFn: (scope: RestoreOperation) =>
    persistThenCreate(scope.persist, () => {
      if (
        !currentScope(scope) ||
        scope.dialogVersion !== dialogVersion ||
        props.sourceDirty ||
        scope.signal.aborted
      )
        throw new Error("Restore scope retired");
      return restoreRevision(
        scope.kind,
        scope.workspaceId,
        scope.id,
        scope.projectId,
        scope.revisionId,
        scope.correlationId,
        scope.expectedTailSeq,
        scope.signal,
      );
    }),
  onSuccess: async (_data, scope) => {
    if (!currentScope(scope)) return;
    if (correlations.get(scope.revisionId)?.correlationId === scope.correlationId)
      correlations.delete(scope.revisionId);
    if (scope.dialogVersion === dialogVersion) {
      cancelRestore();
      notice.value = t("version.restore.done");
    }
    await queryClient.invalidateQueries({ queryKey: scope.queryKey });
  },
  onError: async (err: unknown, scope) => {
    if (!currentScope(scope) || scope.dialogVersion !== dialogVersion || scope.signal.aborted)
      return;
    const timedOut = err instanceof ProblemError && err.status === 504;
    const conflict = err instanceof ProblemError && err.status === 409;
    notice.value = t(
      conflict
        ? "version.restore.conflict"
        : timedOut
          ? "version.restore.timeout"
          : "version.restore.failed",
    );
    if (conflict) {
      correlations.delete(scope.revisionId);
      restorePreview.value = null;
    }
    if (!timedOut) return;
    const invalidations = [queryClient.invalidateQueries({ queryKey: scope.queryKey })];
    if (scope.projectId)
      invalidations.push(
        queryClient.invalidateQueries({
          queryKey: ["project-document", scope.workspaceId, scope.projectId, scope.id],
        }),
      );
    else if (scope.kind === "document")
      invalidations.push(
        queryClient.invalidateQueries({ queryKey: ["document", scope.workspaceId, scope.id] }),
        queryClient.invalidateQueries({ queryKey: ["document-body", scope.workspaceId, scope.id] }),
      );
    await Promise.all(invalidations);
  },
  onSettled: (_data, _error, scope) => {
    if (currentScope(scope)) restorePending.value = false;
  },
});

async function showPreview(id: string): Promise<void> {
  if (!alive || signedOut.value) return;
  const scope = captureScope();
  const selection = ++previewVersion;
  notice.value = null;
  try {
    const detail = await getRevision(
      scope.kind,
      scope.workspaceId,
      scope.id,
      scope.projectId,
      id,
      scope.signal,
    );
    if (currentScope(scope) && selection === previewVersion) preview.value = detail;
  } catch {
    if (currentScope(scope) && selection === previewVersion)
      notice.value = t("version.list.failed");
  }
}

function save(): void {
  if (
    !alive ||
    !writable.value ||
    savePending.value ||
    restorePending.value ||
    restorePreviewPending.value
  )
    return;
  if (props.sourceDirty) {
    notice.value = t("version.restore.dirty");
    return;
  }
  // Acquire synchronously, before Vue Query schedules the mutation or the ACK
  // arrives. Pending covers both the durable barrier and the HTTP create.
  savePending.value = true;
  notice.value = null;
  saveRevision.mutate({ ...captureScope(), persist: props.persistNow });
}

function confirmRestore(): void {
  const revisionId = pendingRestoreId.value;
  const captured = revisionId && correlations.get(revisionId);
  if (
    !alive ||
    !writable.value ||
    !revisionId ||
    !captured ||
    !restorable.value ||
    props.sourceDirty ||
    savePending.value ||
    restorePending.value ||
    restorePreviewPending.value
  )
    return;
  restorePending.value = true;
  notice.value = null;
  restore.mutate({
    ...captureScope(),
    signal: AbortSignal.any([scopeAbort.signal, dialogAbort.signal]),
    revisionId,
    ...captured,
    dialogVersion,
    persist: props.persistNow,
  });
}
</script>

<template>
  <div class="document-revision-host">
    <UButton
      size="sm"
      variant="outline"
      color="neutral"
      :aria-expanded="open"
      aria-controls="document-revision-panel"
      data-testid="revision-history"
      @click="open = !open"
    >
      {{ t("version.history") }}
    </UButton>
    <aside
      v-if="open"
      id="document-revision-panel"
      class="document-revision-panel"
      :aria-label="t('version.historyTitle')"
    >
      <header class="document-revision-panel__head">
        <h2>{{ t("version.historyTitle") }}</h2>
        <UButton
          v-if="!readOnly"
          size="sm"
          :disabled="
            savePending ||
            restorePending ||
            restorePreviewPending ||
            sourceDirty ||
            !persistNow ||
            !writable
          "
          data-testid="revision-save"
          @click="save"
        >
          {{ savePending ? t("version.saving") : t("version.save") }}
        </UButton>
      </header>
      <p v-if="notice" role="status" class="document-revision-panel__notice">{{ notice }}</p>
      <p v-if="listQuery.isLoading.value" class="document-revision-panel__notice">{{
        t("load.loading")
      }}</p>
      <p v-if="listQuery.isError.value" role="alert" class="document-page__error">{{
        t("version.list.failed")
      }}</p>
      <p
        v-if="!listQuery.isLoading.value && !listQuery.isError.value && items.length === 0"
        class="document-revision-panel__notice"
      >
        {{ t("version.empty") }}
      </p>
      <ul class="document-revision">
        <li
          v-for="item in items"
          :key="item.id"
          class="document-revision__item"
          data-testid="revision-item"
        >
          <button type="button" class="document-revision__meta" @click="showPreview(item.id)">
            <span class="document-revision__when">{{ formatAt(item.createdAt, timeZone) }}</span>
            <span class="document-revision__who">
              <span>{{ REASON_LABEL[item.reason] ?? item.reason }}</span
              >{{ " " }}
              <span>{{ authorLabel(item, authorById) }}</span>
              <span v-if="item.restoredFromId"
                >{{ t("version.restore.from") }} {{ item.restoredFromId }}</span
              >
            </span>
          </button>
          <UButton
            v-if="!readOnly"
            size="sm"
            variant="outline"
            color="neutral"
            class="min-h-11"
            :disabled="
              savePending ||
              restorePending ||
              restorePreviewPending ||
              sourceDirty ||
              !persistNow ||
              !writable
            "
            data-testid="revision-restore"
            @click="beginRestore(item.id)"
          >
            {{ t("version.restore") }}
          </UButton>
        </li>
      </ul>
      <section v-if="preview" class="document-revision-preview" :aria-label="t('version.preview')">
        <p data-testid="revision-preview">{{ extractPreviewText(preview.contentJson) || "…" }}</p>
      </section>
      <section v-if="items.length" class="revision-diff" :aria-label="t('version.compare.body')">
        <div class="revision-diff__selectors">
          <label
            >{{ t("version.diff.before") }}
            <select v-model="beforeId" data-testid="revision-before">
              <option v-for="item in items" :key="item.id" :value="item.id"
                >{{ formatAt(item.createdAt, timeZone) }} · {{ authorLabel(item, authorById) }} ·
                {{ item.id.slice(0, 8) }}</option
              >
            </select>
          </label>
          <label
            >{{ t("version.diff.after") }}
            <select v-model="afterId" data-testid="revision-after">
              <option v-for="item in items" :key="item.id" :value="item.id"
                >{{ formatAt(item.createdAt, timeZone) }} · {{ authorLabel(item, authorById) }} ·
                {{ item.id.slice(0, 8) }}</option
              >
            </select>
          </label>
        </div>
        <p v-if="comparisonPending" role="status">{{ t("load.loading") }}</p>
        <div
          v-if="comparison"
          :data-before-revision="comparison.beforeId"
          :data-after-revision="comparison.afterId"
          data-testid="revision-diff"
        >
          <p v-for="limit in comparison.limits" :key="limit" class="revision-diff__limit">{{
            t(`version.diff.${limit}`)
          }}</p>
          <p
            v-if="
              !comparison.changes.length &&
              !comparison.limits.includes('invalid-content') &&
              !comparison.limits.includes('size-limit')
            "
            >{{ t("version.diff.empty") }}</p
          >
          <nav
            v-if="comparison.changes.length"
            class="revision-diff__navigation"
            :aria-label="t('version.compare')"
          >
            <UButton
              variant="outline"
              color="neutral"
              :disabled="changeIndex === 0"
              data-testid="revision-change-previous"
              @click="moveChange(-1)"
              >{{ t("version.diff.previous") }}</UButton
            >
            <output aria-live="polite"
              >{{ changeIndex + 1 }} / {{ comparison.changes.length }}</output
            >
            <UButton
              variant="outline"
              color="neutral"
              :disabled="changeIndex === comparison.changes.length - 1"
              data-testid="revision-change-next"
              @click="moveChange(1)"
              >{{ t("version.diff.next") }}</UButton
            >
          </nav>
          <ol class="revision-diff__changes">
            <li v-for="(change, index) in comparison.changes" :key="change.key">
              <button
                type="button"
                :aria-current="changeIndex === index ? 'step' : undefined"
                :data-change-kind="change.kind"
                @click="changeIndex = index"
                >{{ t(`version.diff.${change.kind}`) }}
                {{ (change.after ?? change.before)?.blockId ?? "" }}</button
              >
            </li>
          </ol>
          <article
            v-if="activeChange"
            data-testid="revision-change"
            :data-change-kind="activeChange.kind"
            :data-before-revision="comparison.beforeId"
            :data-after-revision="comparison.afterId"
          >
            <h3>{{ t(`version.diff.${activeChange.kind}`) }}</h3>
            <p v-if="activeChange.kind === 'moved'"
              >{{ t("version.diff.location") }} {{ blockPosition(activeChange.before) }} →
              {{ blockPosition(activeChange.after) }}</p
            >
            <p class="revision-diff__identity">{{
              t(
                activeChange.identity === "block-id"
                  ? "version.diff.identified"
                  : activeChange.identity === "position"
                    ? "version.diff.position"
                    : "version.diff.unmatched",
              )
            }}</p>
            <div class="revision-diff__sides">
              <section
                ><h4>{{ t("version.diff.before") }}</h4
                ><p
                  ><del v-if="activeChange.before">{{
                    activeChange.before.text || t("version.diff.no-content")
                  }}</del
                  ><span v-else>{{ t("version.diff.no-content") }}</span></p
                ><p class="revision-diff__values">{{
                  changeValues(activeChange, "before")
                }}</p></section
              >
              <section
                ><h4>{{ t("version.diff.after") }}</h4
                ><p
                  ><ins v-if="activeChange.after">{{
                    activeChange.after.text || t("version.diff.no-content")
                  }}</ins
                  ><span v-else>{{ t("version.diff.no-content") }}</span></p
                ><p class="revision-diff__values">{{
                  changeValues(activeChange, "after")
                }}</p></section
              >
            </div>
          </article>
          <details v-if="comparisonBodies"
            ><summary>{{ t("version.diff.full-before") }}</summary
            ><p>{{ extractPreviewText(comparisonBodies.before.contentJson) }}</p></details
          >
          <details v-if="comparisonBodies"
            ><summary>{{ t("version.diff.full-after") }}</summary
            ><p>{{ extractPreviewText(comparisonBodies.after.contentJson) }}</p></details
          >
        </div>
      </section>
      <NativeModal
        :open="!!pendingRestoreId"
        labelled-by="revision-restore-title"
        dialog-class="document-revision-dialog revision-diff-dialog"
        @close="cancelRestore"
      >
        <h3 id="revision-restore-title">{{ t("version.restore.preview") }}</h3>
        <p class="document-revision-dialog__body">{{ t("version.dialog.body") }}</p>
        <p>{{ t("version.restore.history") }}</p>
        <p v-if="notice" role="status">{{ notice }}</p>
        <p v-if="restorePreviewPending" role="status">{{ t("load.loading") }}</p>
        <div
          v-if="restorePreview && restoreComparison"
          data-testid="revision-restore-preview"
          :data-source-revision="restorePreview.source.id"
          :data-preview-tail="restorePreview.currentTailSeq"
        >
          <p v-for="limit in restoreComparison.limits" :key="limit" class="revision-diff__limit">{{
            t(`version.diff.${limit}`)
          }}</p>
          <div class="revision-diff__sides">
            <section
              ><h4>{{ t("version.restore.current") }}</h4
              ><p data-testid="revision-restore-current">{{
                extractPreviewText(restorePreview.currentContentJson)
              }}</p></section
            >
            <section
              ><h4>{{ t("version.restore.source") }}</h4
              ><p
                >{{ formatAt(restorePreview.source.createdAt, timeZone) }}
                {{ authorLabel(restorePreview.source, authorById) }}</p
              ><p data-testid="revision-restore-source">{{
                extractPreviewText(restorePreview.source.contentJson)
              }}</p></section
            >
          </div>
          <nav
            v-if="restoreComparison.changes.length"
            class="revision-diff__navigation"
            :aria-label="t('version.compare')"
          >
            <UButton
              variant="outline"
              color="neutral"
              :disabled="restoreChangeIndex === 0"
              @click="moveChange(-1, true)"
              >{{ t("version.diff.previous") }}</UButton
            >
            <output aria-live="polite"
              >{{ restoreChangeIndex + 1 }} / {{ restoreComparison.changes.length }}</output
            >
            <UButton
              variant="outline"
              color="neutral"
              :disabled="restoreChangeIndex === restoreComparison.changes.length - 1"
              @click="moveChange(1, true)"
              >{{ t("version.diff.next") }}</UButton
            >
          </nav>
          <article
            v-if="activeRestoreChange"
            data-testid="revision-restore-change"
            :data-source-revision="restorePreview.source.id"
            :data-preview-tail="restorePreview.currentTailSeq"
          >
            <h4>{{ t(`version.diff.${activeRestoreChange.kind}`) }}</h4>
            <p v-if="activeRestoreChange.kind === 'moved'"
              >{{ t("version.diff.location") }} {{ blockPosition(activeRestoreChange.before) }} →
              {{ blockPosition(activeRestoreChange.after) }}</p
            >
            <div class="revision-diff__sides">
              <section
                ><h4>{{ t("version.restore.current") }}</h4
                ><p>{{ activeRestoreChange.before?.text || t("version.diff.no-content") }}</p
                ><p class="revision-diff__values">{{
                  changeValues(activeRestoreChange, "before")
                }}</p></section
              >
              <section
                ><h4>{{ t("version.restore.source") }}</h4
                ><p>{{ activeRestoreChange.after?.text || t("version.diff.no-content") }}</p
                ><p class="revision-diff__values">{{
                  changeValues(activeRestoreChange, "after")
                }}</p></section
              >
            </div>
          </article>
        </div>
        <div class="document-revision-dialog__actions revision-diff__actions">
          <UButton
            variant="outline"
            color="neutral"
            data-testid="revision-restore-cancel"
            autofocus
            @click="cancelRestore"
            >{{ t("version.dialog.cancel") }}</UButton
          >
          <UButton
            variant="outline"
            color="neutral"
            :disabled="restorePreviewPending || restorePending || sourceDirty"
            data-testid="revision-restore-refresh"
            @click="pendingRestoreId && beginRestore(pendingRestoreId)"
            >{{ t("version.restore.refresh") }}</UButton
          >
          <UButton
            data-testid="revision-restore-confirm"
            :disabled="
              savePending ||
              restorePending ||
              restorePreviewPending ||
              sourceDirty ||
              !restorable ||
              !writable
            "
            @click="confirmRestore"
            >{{ t("version.restore") }}</UButton
          >
        </div>
      </NativeModal>
    </aside>
  </div>
</template>

<style scoped>
.revision-diff {
  display: grid;
  gap: 1rem;
  margin-block-start: 1rem;
}
.revision-diff__selectors,
.revision-diff__sides {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 1rem;
}
.revision-diff__selectors label {
  display: grid;
  gap: 0.375rem;
  font-size: 0.875rem;
  line-height: 1.5;
}
.revision-diff__selectors select {
  min-width: 0;
  width: 100%;
  min-height: 2.75rem;
  padding: 0.5rem;
  border: 1px solid var(--ui-border);
  border-radius: 0.5rem;
  background: var(--ui-bg);
}
.revision-diff__navigation,
.revision-diff__actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.5rem;
  margin-block: 1rem;
}
.revision-diff__changes {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  padding: 0;
  list-style: none;
}
.revision-diff__changes button {
  overflow-wrap: anywhere;
  min-width: 0;
  text-align: start;
  min-height: 2.75rem;
  padding: 0.375rem 0.5rem;
  border: 1px solid var(--ui-border);
  border-radius: 0.5rem;
  font-size: 0.875rem;
  line-height: 1.5;
}
.revision-diff__changes button[aria-current] {
  border-color: var(--ui-primary);
  background: var(--ui-bg-muted);
}
.revision-diff article,
.revision-diff-dialog {
  font-size: 1rem;
  line-height: 1.6;
  word-break: keep-all;
  overflow-wrap: anywhere;
}
.revision-diff p,
.revision-diff-dialog p {
  white-space: pre-wrap;
}
.revision-diff__sides section {
  min-width: 0;
  border-block-start: 1px solid var(--ui-border);
  padding-block: 0.75rem;
}
.revision-diff__limit {
  color: var(--ui-text-muted);
  font-size: 1rem;
  line-height: 1.6;
}
.revision-diff__identity {
  font-size: 0.875rem;
  line-height: 1.5;
}
.revision-diff__values {
  overflow-wrap: anywhere;
}
.revision-diff-dialog {
  margin: auto;
  padding: 1.5rem;
  border: 1px solid var(--ui-border);
  border-radius: 0.75rem;
  background: var(--ui-bg);
  color: var(--ui-text);
  width: min(48rem, calc(100vw - 2rem));
  max-height: calc(100dvh - 2rem);
  overflow-y: auto;
}
.revision-diff-dialog::backdrop {
  background: rgb(0 0 0 / 40%);
}
.revision-diff :is(button, select):focus-visible {
  outline: 2px solid var(--ui-primary);
  outline-offset: 0.125rem;
}
@media (max-width: 40rem) {
  .revision-diff__selectors,
  .revision-diff__sides {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
