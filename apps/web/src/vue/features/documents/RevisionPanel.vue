<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, nextTick, ref, shallowRef, useTemplateRef, watch } from "vue";
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
    persistNow?: () => Promise<void>;
  }>(),
  { targetKind: "document" },
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
  () => ["revisions", props.workspaceId, props.targetKind, props.documentId, scopeProjectId.value] as const,
);

const listQuery = useQuery(() => ({
  queryKey: queryKey.value,
  queryFn: () => listRevisions(props.targetKind, props.workspaceId, props.documentId, scopeProjectId.value),
  enabled: open.value,
}));
const members = useQuery(() => ({ ...membersQuery(props.workspaceId), enabled: open.value }));
const authorById = computed<Record<string, string>>(() =>
  Object.fromEntries(
    (members.data.value?.items ?? []).map((member) => [member.userId, formatPersonName(member, me.data.value?.locale)]),
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
  mutationFn: () => createRevision(props.targetKind, props.workspaceId, props.documentId, scopeProjectId.value),
  onSuccess: async () => {
    notice.value = null;
    await queryClient.invalidateQueries({ queryKey: queryKey.value });
  },
  onError: () => {
    notice.value = t("version.save.failed");
  },
});

const restore = useMutation({
  mutationFn: async (revisionId: string) => {
    let correlationId = correlations.get(revisionId);
    if (!correlationId) {
      correlationId = crypto.randomUUID();
      correlations.set(revisionId, correlationId);
    }
    return restoreRevision(
      props.targetKind,
      props.workspaceId,
      props.documentId,
      scopeProjectId.value,
      revisionId,
      correlationId,
    );
  },
  onSuccess: async (_data, revisionId) => {
    correlations.delete(revisionId);
    pendingRestoreId.value = null;
    notice.value = t("version.restore.done");
    await queryClient.invalidateQueries({ queryKey: queryKey.value });
  },
  onError: (err: unknown) => {
    const timedOut = err instanceof ProblemError && err.status === 504;
    if (timedOut) {
      void queryClient.invalidateQueries({ queryKey: queryKey.value });
        if (scopeProjectId.value) {
          void queryClient.invalidateQueries({
            queryKey: ["project-document", props.workspaceId, scopeProjectId.value, props.documentId],
          });
        } else if (props.targetKind === "document") {
          void queryClient.invalidateQueries({ queryKey: ["document", props.workspaceId, props.documentId] });
          void queryClient.invalidateQueries({ queryKey: ["document-body", props.workspaceId, props.documentId] });
        }
    }
    notice.value = timedOut ? t("version.restore.timeout") : t("version.restore.failed");
  },
});

async function showPreview(id: string): Promise<void> {
  try {
    preview.value = await getRevision(
      props.targetKind,
      props.workspaceId,
      props.documentId,
      scopeProjectId.value,
      id,
    );
  } catch {
    notice.value = t("version.list.failed");
  }
}

function save(): void {
  void persistThenCreate(props.persistNow, () => saveRevision.mutate()).catch(() => {
    notice.value = t("version.save.failed");
  });
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
    <aside v-if="open" id="document-revision-panel" class="document-revision-panel" :aria-label="t('version.historyTitle')">
      <header class="document-revision-panel__head">
        <h2>{{ t("version.historyTitle") }}</h2>
        <UButton v-if="!readOnly" size="sm" :disabled="saveRevision.isPending.value" data-testid="revision-save" @click="save">
          {{ saveRevision.isPending.value ? t("version.saving") : t("version.save") }}
        </UButton>
      </header>
      <p v-if="notice" role="status" class="document-revision-panel__notice">{{ notice }}</p>
      <p v-if="listQuery.isLoading.value" class="document-revision-panel__notice">{{ t("load.loading") }}</p>
      <p v-if="listQuery.isError.value" role="alert" class="document-page__error">{{ t("version.list.failed") }}</p>
      <p
        v-if="!listQuery.isLoading.value && !listQuery.isError.value && items.length === 0"
        class="document-revision-panel__notice"
      >
        {{ t("version.empty") }}
      </p>
      <ul class="document-revision">
        <li v-for="item in items" :key="item.id" class="document-revision__item" data-testid="revision-item">
          <button type="button" class="document-revision__meta" @click="showPreview(item.id)">
            <span class="document-revision__when">{{ formatAt(item.createdAt, timeZone) }}</span>
            <span class="document-revision__who">
              <span>{{ REASON_LABEL[item.reason] ?? item.reason }}</span> <span>{{ authorLabel(item, authorById) }}</span>
            </span>
          </button>
          <UButton
            v-if="!readOnly"
            size="sm"
            variant="outline"
            color="neutral"
            class="min-h-11"
            :disabled="restore.isPending.value"
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
        <p id="revision-restore-body" class="document-revision-dialog__body">{{ t("version.dialog.body") }}</p>
        <div class="document-revision-dialog__actions">
          <UButton ref="cancelButton" variant="outline" color="neutral" @click="pendingRestoreId = null">
            {{ t("version.dialog.cancel") }}
          </UButton>
          <UButton
            ref="confirmButton"
            data-testid="revision-restore-confirm"
            :disabled="restore.isPending.value"
            @click="pendingRestoreId && restore.mutate(pendingRestoreId)"
          >
            {{ t("version.restore") }}
          </UButton>
        </div>
      </div>
    </aside>
  </div>
</template>
