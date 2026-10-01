<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, nextTick, onScopeDispose, ref, shallowRef, useTemplateRef, watch } from "vue";
import {
  authorLabel,
  createRevision,
  extractPreviewText,
  formatAt,
  getRevision,
  listRevisions,
  REASON_LABEL,
  restoreRevision,
  type RevisionDetail,
  type RevisionTargetKind,
} from "@/features/documents/revision-api";
import { persistThenCreate } from "@/features/documents/revision-persist";
import { ProblemError } from "@/lib/api";
import { meQuery, membersQuery } from "@/lib/queries";

// The document's revisions: save one (after persisting the live body),
// preview and restore (features/documents/revision-panel.tsx). A restore is
// applied to the room by the server, so every editor sees it.
const props = withDefaults(
  defineProps<{
    workspaceId: string;
    documentId: string;
    projectId: string | null;
    /** Documents (default) or the task body room. */
    targetKind?: "document" | "task";
    readOnly: boolean;
    persistNow?: (() => Promise<void>) | undefined;
  }>(),
  { targetKind: "document", persistNow: undefined },
);
const queryClient = useQueryClient();
const me = useQuery(meQuery);
const timeZone = computed(() => me.data.value?.timezone || "Asia/Seoul");
const open = ref(false);
const notice = ref<string | null>(null);
const pendingRestoreId = ref<string | null>(null);
const preview = shallowRef<RevisionDetail | null>(null);
const correlations = new Map<string, string>();
const cancelButton = useTemplateRef<{ $el?: Element }>("cancelButton");
const confirmButton = useTemplateRef<{ $el?: Element }>("confirmButton");
let opener: HTMLElement | null = null;
const scopeProjectId = computed(() => (props.targetKind === "document" ? props.projectId : null));
const queryKey = computed(
  () =>
    [
      "revisions",
      props.workspaceId,
      props.targetKind,
      props.documentId,
      scopeProjectId.value,
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
    enabled: open.value,
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
const items = computed(() => listQuery.data.value?.items ?? []);

function elementOf(target: { $el?: Element } | null): HTMLElement | null {
  const element = target?.$el;
  return element instanceof HTMLElement ? element : null;
}

watch(pendingRestoreId, async (id) => {
  if (!id) {
    opener?.focus();
    opener = null;
    return;
  }
  if (!opener && document.activeElement instanceof HTMLElement) opener = document.activeElement;
  await nextTick();
  elementOf(cancelButton.value)?.focus();
});

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

type RestoreOperation = RevisionScope & { revisionId: string; correlationId: string };
const restore = useMutation({
  mutationFn: (scope: RestoreOperation) => {
    if (!currentScope(scope)) throw new Error("Revision scope retired");
    return restoreRevision(
      scope.kind,
      scope.workspaceId,
      scope.id,
      scope.projectId,
      scope.revisionId,
      scope.correlationId,
      scope.signal,
    );
  },
  onSuccess: async (_data, scope) => {
    if (!currentScope(scope)) return;
    correlations.delete(scope.revisionId);
    pendingRestoreId.value = null;
    notice.value = t("version.restore.done");
    await queryClient.invalidateQueries({ queryKey: scope.queryKey });
  },
  onError: async (err: unknown, scope) => {
    if (!currentScope(scope)) return;
    const timedOut = err instanceof ProblemError && err.status === 504;
    notice.value = timedOut ? t("version.restore.timeout") : t("version.restore.failed");
    if (!timedOut) return;
    const invalidations = [queryClient.invalidateQueries({ queryKey: scope.queryKey })];
    if (scope.projectId) {
      invalidations.push(
        queryClient.invalidateQueries({
          queryKey: ["project-document", scope.workspaceId, scope.projectId, scope.id],
        }),
      );
    } else if (scope.kind === "document") {
      invalidations.push(
        queryClient.invalidateQueries({ queryKey: ["document", scope.workspaceId, scope.id] }),
        queryClient.invalidateQueries({ queryKey: ["document-body", scope.workspaceId, scope.id] }),
      );
    }
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
  if (!alive || !writable.value || savePending.value || restorePending.value) return;
  // Acquire synchronously, before Vue Query schedules the mutation or the ACK
  // arrives. Pending covers both the durable barrier and the HTTP create.
  savePending.value = true;
  notice.value = null;
  saveRevision.mutate({ ...captureScope(), persist: props.persistNow });
}

function confirmRestore(): void {
  const revisionId = pendingRestoreId.value;
  if (!alive || !writable.value || !revisionId || savePending.value || restorePending.value) return;
  let correlationId = correlations.get(revisionId);
  if (!correlationId) {
    correlationId = crypto.randomUUID();
    correlations.set(revisionId, correlationId);
  }
  restorePending.value = true;
  notice.value = null;
  restore.mutate({ ...captureScope(), revisionId, correlationId });
}

function onDialogKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    pendingRestoreId.value = null;
    return;
  }
  if (event.key !== "Tab") return;
  const first = elementOf(cancelButton.value);
  const last = elementOf(confirmButton.value);
  if (!first || !last) return;
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
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
          :disabled="savePending || restorePending || !persistNow || !writable"
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
            </span>
          </button>
          <UButton
            v-if="!readOnly"
            size="sm"
            variant="outline"
            color="neutral"
            class="min-h-11"
            :disabled="savePending || restorePending || !writable"
            data-testid="revision-restore"
            @click="pendingRestoreId = item.id"
          >
            {{ t("version.restore") }}
          </UButton>
        </li>
      </ul>
      <section v-if="preview" class="document-revision-preview" :aria-label="t('version.preview')">
        <p data-testid="revision-preview">{{ extractPreviewText(preview.contentJson) || "…" }}</p>
      </section>
      <div
        v-if="pendingRestoreId"
        class="document-revision-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="revision-restore-title"
        aria-describedby="revision-restore-body"
        @keydown="onDialogKeydown"
      >
        <h3 id="revision-restore-title">{{ t("version.dialog.title") }}</h3>
        <p id="revision-restore-body" class="document-revision-dialog__body">{{
          t("version.dialog.body")
        }}</p>
        <div class="document-revision-dialog__actions">
          <UButton
            ref="cancelButton"
            variant="outline"
            color="neutral"
            @click="pendingRestoreId = null"
          >
            {{ t("version.dialog.cancel") }}
          </UButton>
          <UButton
            ref="confirmButton"
            data-testid="revision-restore-confirm"
            :disabled="savePending || restorePending || !writable"
            @click="confirmRestore"
          >
            {{ t("version.restore") }}
          </UButton>
        </div>
      </div>
    </aside>
  </div>
</template>
