<script setup lang="ts">
import { FvociEditor, type TiptapEditor } from "@fvoci/editor/vue";
import "@fvoci/editor/styles.css";
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UCollapsible from "@nuxt/ui/components/Collapsible.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import {
  computed,
  type FunctionalComponent,
  h,
  markRaw,
  nextTick,
  onScopeDispose,
  ref,
  shallowRef,
  watch,
} from "vue";
import { bindBlockPresence, isBlockPresenceAwareness } from "@/features/documents/block-presence";
import { collabBadge, collabRefusalNote } from "@/features/documents/collab-badge";
import { collabUserOf, setTitleEditing } from "@/features/documents/collab-model";
import {
  type DocumentScope,
  moveDocument,
  type PatchDocumentBody,
  patchDocument,
  trashDocument,
} from "@/features/documents/document-api";
import { projectAncestors } from "@/features/documents/project-ancestors";
import { projectDocumentsQuery } from "@/features/projects/queries";
import { createProjectDocumentAttachmentBridge } from "@/features/workspace/attachment-upload";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { documentPath, formatDisplayId, projectPath, trashPath } from "@/lib/href";
import { meQuery } from "@/lib/queries";
import { projectDocumentMetaQuery } from "@/lib/queries/documents";
import { collabRoomName, useCollabRoom } from "../../collab/useCollabRoom";
import AppLink from "../../components/AppLink.vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import CommentPanel from "../comments/CommentPanel.vue";
import EditorControls from "../editor/EditorControls.vue";
import TemplateToolbar from "../editor/TemplateToolbar.vue";
import { useEditorEntities } from "../editor/useEditorEntities";
import UnfurlCard from "../editor/UnfurlCard.vue";
import CollabPresence from "./CollabPresence.vue";
import DocumentAiMenu from "./DocumentAiMenu.vue";
import DocumentExportMenu from "./DocumentExportMenu.vue";
import DocumentTagsBar from "./DocumentTagsBar.vue";
import OriginPanel from "./OriginPanel.vue";
import RevisionPanel from "./RevisionPanel.vue";
import ShareDialog from "./ShareDialog.vue";
import StarToggle from "./StarToggle.vue";
import "@/features/documents/document-shell.css";

/** The project's page context (React DocumentView's ProjectDocumentContext). */
export type ProjectDocumentContext = {
  id: string;
  key: string;
  rootDocumentId: string | null;
  canEdit: boolean;
  archived: boolean;
};

// A project document (the React document-view.tsx, project branch). Same
// collab room as wiki (`kind: "document"`); the parent keys this component
// by the room name so there is never a second provider.
const props = defineProps<{
  workspaceId: string;
  slug: string;
  documentId: string;
  project: ProjectDocumentContext;
}>();

const STATUSES = ["draft", "published", "archived"] as const;
const STATUS_LABEL = {
  draft: "doc.status.draft",
  published: "doc.status.published",
  archived: "doc.status.archived",
} as const;
const TITLE_MAX = 300;
const ICON_MAX = 50;

const queryClient = useQueryClient();
const me = useQuery(meQuery);
const projectId = computed(() => props.project.id);
const metaQuery = useQuery(() =>
  projectDocumentMetaQuery(props.workspaceId, projectId.value, props.documentId),
);
const tree = useQuery(() => projectDocumentsQuery(props.workspaceId, projectId.value));
const scope = computed<DocumentScope>(() => ({
  workspaceId: props.workspaceId,
  documentId: props.documentId,
  projectId: projectId.value,
}));

const collabUser = computed(() => {
  const data = me.data.value;
  return data ? collabUserOf(data.userId, formatPersonName(data, data.locale)) : null;
});
const room = useCollabRoom(
  collabRoomName(props.workspaceId, "document", props.documentId),
  collabUser,
);
const session = room.session;
const { mentionItems, entityResolver } = useEditorEntities(
  () => props.workspaceId,
  () =>
    `${props.documentId}:${session.value?.generation ?? ""}:${collabUser.value?.id ?? ""}:${session.value?.status === "unauthorized"}`,
);

const optionsOpen = ref(false);
const optionsButton = ref<InstanceType<typeof UButton> | null>(null);
function onOptionsKeydown(event: KeyboardEvent): void {
  if (event.key !== "Escape" || !optionsOpen.value) return;
  event.preventDefault();
  optionsOpen.value = false;
  optionsButton.value?.$el.focus();
}

const title = ref("");
const titleInput = ref<HTMLTextAreaElement | null>(null);
// Keep long titles readable at the current width, including readonly titles.
watch(
  [titleInput, title],
  async ([input], _previous, onCleanup) => {
    if (!input) return;
    let width = 0;
    const resize = () => {
      input.style.height = "auto";
      input.style.height = `${input.scrollHeight + 2}px`;
    };
    const observer = new ResizeObserver(([entry]) => {
      if (entry && entry.contentRect.width !== width) {
        width = entry.contentRect.width;
        resize();
      }
    });
    observer.observe(input);
    onCleanup(() => observer.disconnect());
    await nextTick();
    if (titleInput.value === input) resize();
  },
  { flush: "post" },
);
const icon = ref("");
const status = ref<string>("draft");
const saveError = ref<string | null>(null);
const persistError = ref<string | null>(null);
const persisting = ref(false);
const moveParentId = ref("");
const lifecycleError = ref<string | null>(null);
const editor = shallowRef<TiptapEditor | null>(null);

watch(
  () => metaQuery.data.value,
  (data) => {
    if (!data) return;
    title.value = data.title;
    icon.value = data.icon ?? "";
    status.value = data.status;
  },
  { immediate: true },
);

const attachmentBridge = markRaw(
  createProjectDocumentAttachmentBridge(props.workspaceId, props.project.id, props.documentId),
);
const UrlEmbed: FunctionalComponent<{ url: string }> = markRaw((embed: { url: string }) =>
  h(UnfurlCard, { workspaceId: props.workspaceId, url: embed.url }),
);
UrlEmbed.props = ["url"];

watch(
  [editor, () => session.value?.provider.awareness],
  ([current, awareness], _previous, onCleanup) => {
    if (!current || current.isDestroyed || !isBlockPresenceAwareness(awareness)) return;
    onCleanup(bindBlockPresence(current, awareness));
  },
);

type DocumentOperation = { scope: DocumentScope; slug: string; lifecycle: number };
let operationLifecycle = 0;
// The computed session is a snapshot: peers, pending and ACKs replace it.
// Only the actual room/provider generation, actor and route retire operations.
watch(
  [
    () => scope.value.workspaceId,
    () => scope.value.documentId,
    () => scope.value.projectId,
    () => props.slug,
    () => collabUser.value?.id,
    () => session.value?.doc,
    () => session.value?.provider,
    () => session.value?.generation,
  ],
  () => {
    operationLifecycle += 1;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  operationLifecycle += 1;
});
function captureOperation(): DocumentOperation {
  return { scope: { ...scope.value }, slug: props.slug, lifecycle: operationLifecycle };
}
function currentOperation(operation: DocumentOperation): boolean {
  return operation.lifecycle === operationLifecycle && session.value !== null;
}

const trashDoc = useMutation({
  mutationFn: (operation: DocumentOperation) => trashDocument(operation.scope),
  onSuccess: async (_result, operation) => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["projects", operation.scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["wiki-discovery", operation.scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] }),
    ]);
    if (!currentOperation(operation)) return;
    lifecycleError.value = null;
    window.location.assign(trashPath(operation.slug));
  },
  onError: (error: unknown, operation) => {
    if (currentOperation(operation)) lifecycleError.value = loadErrorMessage(error);
  },
});

const moveDoc = useMutation({
  mutationFn: (operation: DocumentOperation & { newParentId: string }) =>
    moveDocument(operation.scope, operation.newParentId),
  onSuccess: async (_result, operation) => {
    const { workspaceId, documentId, projectId } = operation.scope;
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["wiki-discovery", workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] }),
      queryClient.invalidateQueries({ queryKey: ["project-documents", workspaceId, projectId] }),
      queryClient.invalidateQueries({
        queryKey: ["project-document", workspaceId, projectId, documentId],
      }),
    ]);
    if (!currentOperation(operation)) return;
    lifecycleError.value = null;
    moveParentId.value = "";
  },
  onError: (error: unknown, operation) => {
    if (currentOperation(operation)) lifecycleError.value = loadErrorMessage(error);
  },
});

function move(newParentId: string): void {
  moveDoc.mutate({ ...captureOperation(), newParentId });
}

const patchMeta = useMutation({
  mutationFn: (operation: DocumentOperation & { body: PatchDocumentBody }) =>
    patchDocument(operation.scope, operation.body),
  onSuccess: async (_result, operation) => {
    const { workspaceId, documentId, projectId } = operation.scope;
    await Promise.all([
      queryClient.invalidateQueries({
        queryKey: ["project-document", workspaceId, projectId, documentId],
      }),
      queryClient.invalidateQueries({ queryKey: ["project-documents", workspaceId, projectId] }),
      queryClient.invalidateQueries({ queryKey: ["wiki-discovery", workspaceId] }),
    ]);
    if (currentOperation(operation)) saveError.value = null;
  },
  onError: (error: unknown, operation) => {
    if (currentOperation(operation)) saveError.value = loadErrorMessage(error);
  },
});

const notFound = computed(
  () => metaQuery.error.value instanceof ProblemError && metaQuery.error.value.status === 404,
);
const meta = computed(() => metaQuery.data.value);
const displayRef = computed(() =>
  meta.value ? formatDisplayId(props.project.key, meta.value.number) : "",
);
const crumbAncestors = computed(() =>
  projectAncestors(tree.data.value?.items ?? [], props.documentId, props.project.rootDocumentId),
);
const saving = computed(() => patchMeta.isPending.value);
const archived = computed(() => meta.value?.status === "archived");
const projectReadOnly = computed(() => !props.project.canEdit || props.project.archived);
const readOnly = computed(
  () => archived.value || projectReadOnly.value || (session.value?.readOnly ?? false),
);
const ready = computed(() => Boolean(session.value?.synced && collabUser.value));
const refusalNote = computed(() => collabRefusalNote(session.value?.status, ready.value));
const badge = computed(() =>
  session.value
    ? collabBadge(
        session.value.status,
        session.value.pending || persisting.value,
        session.value.durableSaved,
      )
    : null,
);
const canPersist = computed(
  () =>
    ready.value &&
    !readOnly.value &&
    session.value !== null &&
    session.value.status === "connected" &&
    !persisting.value,
);
const moveTargets = computed(() => {
  const docPath = meta.value?.path ?? "";
  const docPathPrefix = `${docPath}.`;
  return (tree.data.value?.items ?? []).filter(
    (node) =>
      node.id !== props.documentId &&
      node.projectId === projectId.value &&
      node.path !== docPath &&
      !node.path.startsWith(docPathPrefix),
  );
});
const awareness = computed(() => session.value?.provider.awareness);

async function saveTitle(): Promise<void> {
  const current = meta.value;
  if (!current) return;
  const next = title.value.trim();
  if (!next || next === current.title) return;
  const operation = { ...captureOperation(), body: { title: next } };
  try {
    await patchMeta.mutateAsync(operation);
  } catch {
    if (currentOperation(operation)) title.value = current.title;
  }
}

async function saveIcon(): Promise<void> {
  const current = meta.value?.icon ?? "";
  if (icon.value === current) return;
  const nextIcon = icon.value.trim() === "" ? null : icon.value.trim();
  const operation = { ...captureOperation(), body: { icon: nextIcon } };
  try {
    await patchMeta.mutateAsync(operation);
  } catch {
    if (currentOperation(operation)) icon.value = current;
  }
}

async function saveStatus(next: string): Promise<void> {
  const current = meta.value;
  if (!current || next === current.status) return;
  const previous = current.status;
  const operation = { ...captureOperation(), body: { status: next } };
  try {
    await patchMeta.mutateAsync(operation);
  } catch {
    if (currentOperation(operation)) status.value = previous;
  }
}

function onStatusChange(event: Event): void {
  const next = (event.target as HTMLSelectElement).value;
  status.value = next;
  void saveStatus(next);
}

async function persistBody(): Promise<void> {
  const current = session.value;
  if (!current || !canPersist.value) return;
  persistError.value = null;
  persisting.value = true;
  try {
    await current.persistNow();
  } catch (error) {
    const timedOut = error instanceof Error && error.message.includes("timed out");
    persistError.value = timedOut ? t("collab timeout — retry") : t("collab unavailable");
    throw error;
  } finally {
    persisting.value = false;
  }
}

function onTitleFocus(): void {
  if (!readOnly.value && isBlockPresenceAwareness(awareness.value))
    setTitleEditing(awareness.value, true);
}

function onTitleBlur(): void {
  if (isBlockPresenceAwareness(awareness.value)) setTitleEditing(awareness.value, false);
  void saveTitle();
}

function onTitleInput(event: Event): void {
  const input = event.target as HTMLTextAreaElement & { composing?: boolean };
  if (input.composing || (event as InputEvent).isComposing) return;
  // Match the previous single-line input's paste behavior.
  title.value = input.value.replace(/[\r\n]/g, "");
}

function onTitleKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter" && !event.isComposing) {
    event.preventDefault();
    (event.target as HTMLTextAreaElement).blur();
  }
}

function trash(): void {
  if (!window.confirm(`${t("doc.trash.confirm.title")}\n${t("doc.trash.confirm.body")}`)) return;
  trashDoc.mutate(captureOperation());
}

function flashBlock(id: string): void {
  const element = document.querySelector<HTMLElement>(
    `.fvoci-editor [data-id="${CSS.escape(id)}"]`,
  );
  if (!element) return;
  element.setAttribute("data-afn-flash", "");
  window.setTimeout(() => element.removeAttribute("data-afn-flash"), 800);
}

function refOf(number: number): string {
  return formatDisplayId(props.project.key, number);
}
</script>

<template>
  <div v-if="notFound" class="document-page">
    <p>{{ t("doc.error.notFound") }}</p>
    <AppLink :to="projectPath(slug, project.key)">{{ t("nav.projects") }}</AppLink>
  </div>
  <QueryError
    v-else-if="metaQuery.isError.value"
    :message="loadErrorMessage(metaQuery.error.value)"
    @retry="metaQuery.refetch()"
  />
  <QueryLoading v-else-if="!meta" />
  <article v-else class="document-page" :data-testid="`document-${displayRef}`">
    <header class="document-page__head">
      <nav class="document-page__breadcrumb" :aria-label="t('breadcrumb.ancestors')">
        <AppLink :to="projectPath(slug, project.key)">{{ project.key }}</AppLink>
        <span v-for="item in crumbAncestors" :key="item.id">
          <span aria-hidden="true"> / </span>
          <AppLink :to="documentPath(slug, refOf(item.number))">{{ item.title }}</AppLink>
        </span>
        <span aria-hidden="true"> / </span>
        <span>{{ displayRef }}</span>
      </nav>
      <div class="document-page__meta">
        <textarea
          ref="titleInput"
          v-model="title"
          rows="1"
          class="document-page__title"
          :aria-label="t('doc.title')"
          :maxlength="TITLE_MAX"
          :disabled="saving || readOnly"
          @focus="onTitleFocus"
          @blur="onTitleBlur"
          @keydown="onTitleKeydown"
          @input="onTitleInput"
        />
        <div class="document-page__fields">
          <span class="document-page__badge">{{ displayRef }}</span>
          <span class="document-page__badge">{{
            t(
              status === "published"
                ? "doc.status.published"
                : status === "archived"
                  ? "doc.status.archived"
                  : "doc.status.draft",
            )
          }}</span>
          <span v-if="readOnly" class="document-page__badge">{{ t("doc.readOnly") }}</span>
          <StarToggle :workspace-id="workspaceId" type="document" :target-id="documentId" />
          <ShareDialog
            v-if="!readOnly"
            :workspace-id="workspaceId"
            :target="{ documentId, projectId: project.id }"
          />
        </div>
        <DocumentTagsBar
          :workspace-id="workspaceId"
          :document-id="documentId"
          :project-id="project.id"
          :read-only="readOnly"
        />
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
          <CollabPresence v-if="session" :peers="session.peers" @jump="flashBlock" />
          <RevisionPanel
            :workspace-id="workspaceId"
            :document-id="documentId"
            :project-id="project.id"
            :read-only="readOnly"
            :persist-now="canPersist ? persistBody : undefined"
          />
        </div>
        <UCollapsible
          v-model:open="optionsOpen"
          :unmount-on-hide="false"
          class="document-page__options"
          :ui="{ content: 'data-[state=open]:animate-none data-[state=closed]:animate-none' }"
          @keydown="onOptionsKeydown"
        >
          <UButton
            ref="optionsButton"
            size="sm"
            variant="outline"
            color="neutral"
            :trailing-icon="optionsOpen ? 'i-lucide-chevron-up' : 'i-lucide-chevron-down'"
            >{{ t("doc.options") }}</UButton
          >
          <template #content>
            <div class="document-page__options-content">
              <div class="document-page__fields">
                <div class="document-page__field">
                  <label for="project-document-icon" class="text-sm font-medium">{{
                    t("project.icon")
                  }}</label>
                  <input
                    id="project-document-icon"
                    v-model="icon"
                    class="document-page__field-input"
                    :maxlength="ICON_MAX"
                    :disabled="saving || readOnly"
                    @blur="saveIcon"
                  />
                </div>
                <div class="document-page__field">
                  <label for="project-document-status" class="text-sm font-medium">{{
                    t("doc.status.a11y")
                  }}</label>
                  <select
                    id="project-document-status"
                    class="document-page__field-select"
                    :value="status"
                    :aria-label="t('doc.status.a11y')"
                    :disabled="saving || readOnly"
                    @change="onStatusChange"
                  >
                    <option v-for="value in STATUSES" :key="value" :value="value">{{
                      t(STATUS_LABEL[value])
                    }}</option>
                  </select>
                </div>
              </div>
              <DocumentExportMenu
                :workspace-id="workspaceId"
                :document-id="documentId"
                :title="title"
                :project-id="project.id"
                :persist-now="canPersist ? persistBody : undefined"
              />
              <div
                v-if="!readOnly"
                class="document-page__lifecycle"
                :aria-label="t('doc.move.title')"
              >
                <label class="document-page__field">
                  <span class="sr-only">{{ t("doc.move.parentLabel") }}</span>
                  <select
                    v-model="moveParentId"
                    class="document-page__field-select"
                    :aria-label="t('doc.move.parentLabel')"
                    :disabled="moveDoc.isPending.value || trashDoc.isPending.value"
                  >
                    <option value="">{{ t("doc.move.parentLabel") }}</option>
                    <option v-for="node in moveTargets" :key="node.id" :value="node.id">{{
                      node.title
                    }}</option>
                  </select>
                </label>
                <UButton
                  size="sm"
                  variant="outline"
                  color="neutral"
                  :disabled="!moveParentId || moveDoc.isPending.value || trashDoc.isPending.value"
                  @click="moveParentId && move(moveParentId)"
                >
                  {{ moveDoc.isPending.value ? t("doc.move.pending") : t("doc.move.submit") }}
                </UButton>
                <UButton
                  size="sm"
                  variant="outline"
                  color="neutral"
                  :disabled="trashDoc.isPending.value || moveDoc.isPending.value"
                  :aria-label="t('doc.trash.action')"
                  @click="trash"
                >
                  {{ trashDoc.isPending.value ? t("doc.trash.pending") : t("doc.trash.action") }}
                </UButton>
              </div>
            </div>
          </template>
        </UCollapsible>
        <p v-if="lifecycleError" role="alert" class="document-page__error">{{ lifecycleError }}</p>
        <p v-if="saveError" role="alert" class="document-page__error">{{ saveError }}</p>
        <p v-if="persistError" role="alert" class="document-page__error">{{ persistError }}</p>
      </div>
    </header>
    <section
      class="document-page__body document-page__body--editor"
      :aria-label="t('doc.body.a11y')"
    >
      <p v-if="session?.status === 'unauthorized'" class="document-page__body-note" role="alert">
        {{ t("doc.collab.unauthorized") }}
      </p>
      <p v-if="refusalNote" class="document-page__body-note" role="status">{{ t(refusalNote) }}</p>
      <QueryLoading v-if="!ready && session?.status !== 'unauthorized' && !refusalNote" />
      <FvociEditor
        v-if="ready && session && collabUser"
        :key="session.generation"
        :ydoc="session.doc"
        :provider="session.provider"
        :user="collabUser"
        :editable="!readOnly"
        :aria-label="t('doc.body.a11y')"
        :workspace-slug="slug"
        :mention-items="mentionItems"
        :entity-resolver="entityResolver"
        :attachment-bridge="attachmentBridge"
        :url-embed="UrlEmbed"
        @ready="editor = $event"
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
    </section>
    <DocumentAiMenu
      :workspace-id="workspaceId"
      :slug="slug"
      :document-id="documentId"
      :project="{ id: project.id, canCreateTasks: project.canEdit && !project.archived }"
      :editor="ready ? editor : null"
      :insert-blocked-reason="
        readOnly
          ? t('doc.readOnly')
          : session?.status === 'connected'
            ? null
            : t('ai.document.loading')
      "
    />
    <OriginPanel :workspace-id="workspaceId" :slug="slug" :document-id="documentId" />
    <CommentPanel
      v-if="me.data.value"
      kind="document"
      :workspace-id="workspaceId"
      :target-id="documentId"
      :project-id="project.id"
      :current-user-id="me.data.value.userId"
      :read-only="readOnly"
    />
  </article>
</template>
