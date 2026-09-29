<script setup lang="ts">
import type { TiptapEditor } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, shallowRef } from "vue";
import { RouterLink } from "vue-router";
import {
  aiInsertNodes,
  appendRange,
  applyTaskTitles,
  hasPendingTask,
  isDefiniteStatus,
  type TaskApplyState,
} from "@/features/documents/document-ai-apply";
import {
  type AiAction,
  AI_ACTIONS,
  AI_APPLY_LABEL,
  AI_MENU_LABEL,
  type AiResult,
  createAiTask,
  runAiAction,
} from "@/features/documents/document-ai-api";
import { ProblemError, problemMessage } from "@/lib/api";
import { documentPath, wikiDisplayId } from "@/lib/href";
import { publicInstanceQuery } from "@/lib/queries/admin";
import { treeQuery } from "@/lib/queries/documents";
import { selectAiEnabled } from "@/lib/queries/instance-settings";

// The document AI actions (features/documents/document-ai-menu.tsx): run
// against the saved document, reviewed, and applied only on confirmation —
// summaries and link mentions are appended to the live collaborative editor
// (never a body rewrite), task titles become tasks in the document's
// project. Shown only when the public features.ai setting is true. The page
// mounts it inside the collab room, so a result belongs to its document.
const props = defineProps<{
  workspaceId: string;
  slug: string;
  documentId: string;
  /** The document's project; wiki documents have none, so they cannot make tasks. */
  project: { id: string; canCreateTasks: boolean } | null;
  /** The live body editor; null until it is attached. */
  editor: TiptapEditor | null;
  /** Why inserting is not possible now (read-only, not connected yet); null when it is. */
  insertBlockedReason: string | null;
}>();
const queryClient = useQueryClient();
const aiEnabled = useQuery({ ...publicInstanceQuery, select: selectAiEnabled });
const tree = useQuery(() => treeQuery(props.workspaceId));
const result = shallowRef<AiResult | null>(null);
const error = ref<string | null>(null);
const notice = ref<string | null>(null);
// WHY: the public flag is only a UI gate; the server answers 503 ai_unavailable when its own
// gate is off — lock the buttons from then on.
const unavailable = ref(false);
// WHY: a second click can land before the pending state renders; this closes that window.
let applying = false;

const run = useMutation({
  mutationFn: (action: AiAction) => runAiAction(props.workspaceId, props.documentId, action),
  onMutate: () => {
    error.value = null;
    notice.value = null;
    result.value = null;
  },
  onSuccess: (next) => {
    result.value = next;
  },
  onError: (err: unknown) => {
    if (err instanceof ProblemError && (err.status === 503 || err.code === "ai_unavailable")) {
      unavailable.value = true;
      error.value = t("ai unavailable");
      return;
    }
    error.value = problemMessage(err, "ai.failed");
  },
});

const createTasks = useMutation({
  mutationFn: async (current: Extract<AiResult, { action: "generateTasks" }>) => {
    const project = props.project;
    if (!project) throw new Error("project document required");
    try {
      return await applyTaskTitles(
        current.titles,
        current.states,
        (title) => createAiTask(props.workspaceId, project.id, title),
        (err) => isDefiniteStatus(err instanceof ProblemError ? err.status : null),
        (states) => {
          result.value = { ...current, states };
        },
      );
    } finally {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["tasks", props.workspaceId, project.id] }),
        queryClient.invalidateQueries({ queryKey: ["projects", props.workspaceId] }),
      ]);
    }
  },
  onSettled: () => {
    applying = false;
  },
  onSuccess: (outcome, current) => {
    const createdTotal = outcome.states.filter((state) => state === "created").length;
    if (outcome.failure === null) {
      result.value = null;
      error.value = null;
      notice.value = t("ai.tasks.done", { count: createdTotal });
      return;
    }
    result.value = { ...current, states: outcome.states };
    notice.value = createdTotal > 0 ? t("ai.tasks.done", { count: createdTotal }) : null;
    error.value =
      outcome.failure.kind === "unknown" ? t("error.network") : problemMessage(outcome.failure.error, "ai.tasks.failed");
  },
  onError: (err: unknown) => {
    error.value = problemMessage(err, "ai.tasks.failed");
  },
});

// The wiki tree only adds a preview link for wiki documents; the label is the server's title.
const links = computed(() => {
  const current = result.value;
  if (current?.action !== "suggestLinks") return [];
  const nodes = new Map((tree.data.value?.items ?? []).map((node) => [node.id, node]));
  return current.documents.map(({ id, title }) => {
    const node = nodes.get(id);
    return {
      id,
      label: title,
      href: node && node.projectId === null ? documentPath(props.slug, wikiDisplayId(node.number)) : undefined,
    };
  });
});

type Item = { key: string; label: string; href?: string; state?: TaskApplyState };
const items = computed<Item[]>(() => {
  const current = result.value;
  if (current?.action === "summarize") return current.lines.map((line, index) => ({ key: `${index}`, label: line }));
  if (current?.action === "generateTasks") {
    return current.titles.map((title, index) => ({ key: `${index}`, label: title, state: current.states[index] }));
  }
  if (current?.action === "suggestLinks") return links.value.map((link) => ({ key: link.id, ...link }));
  return [];
});

const taskReason = computed(() => (props.project === null ? t("ai.tasks.noProject") : null));
const applyReason = computed(() => {
  const current = result.value;
  if (!current) return null;
  if (current.action === "generateTasks") {
    return taskReason.value ?? (props.project?.canCreateTasks ? null : t("doc.readOnly"));
  }
  return props.editor === null ? t("ai.document.loading") : props.insertBlockedReason;
});
const canApply = computed(() => {
  const current = result.value;
  if (!current || applyReason.value !== null) return false;
  if (current.action === "generateTasks") return hasPendingTask(current.states);
  return items.value.length > 0;
});

function apply(): void {
  const current = result.value;
  if (!current || !canApply.value || applying) return;
  error.value = null;
  notice.value = null;
  if (current.action === "generateTasks") {
    applying = true;
    createTasks.mutate(current);
    return;
  }
  const editor = props.editor;
  if (!editor || editor.isDestroyed) {
    error.value = t("ai.document.loading");
    return;
  }
  const content =
    current.action === "summarize"
      ? aiInsertNodes({ action: "summarize", lines: current.lines })
      : aiInsertNodes({ action: "suggestLinks", links: links.value });
  // WHY: after the last block, not at the caret — it never splits the sentence being edited.
  // A normal editor transaction, so Yjs syncs it like typing.
  applying = true;
  let inserted = false;
  try {
    inserted = editor.chain().insertContentAt(appendRange(editor.state.doc), content).focus("end").run();
  } finally {
    applying = false;
  }
  if (!inserted) {
    error.value = t("ai.failed");
    return;
  }
  result.value = null;
  notice.value = t("ai.insert.done");
}
</script>

<template>
  <section v-if="aiEnabled.data.value === true" class="document-ai-menu mt-6 flex flex-col gap-2" :aria-label="t('ai.menu')">
    <div role="group" :aria-label="t('ai.menu')" class="flex flex-wrap items-center gap-2">
      <span class="text-sm font-medium">{{ t("ai.menu") }}</span>
      <UButton
        v-for="action in AI_ACTIONS"
        :key="action"
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="
          run.isPending.value || createTasks.isPending.value || unavailable || (action === 'generateTasks' && taskReason !== null)
        "
        @click="run.mutate(action)"
      >
        {{ t(AI_MENU_LABEL[action]) }}
      </UButton>
    </div>
    <p v-if="taskReason && !unavailable" class="text-xs text-muted break-keep">{{ taskReason }}</p>
    <p v-if="run.isPending.value" role="status" class="text-sm text-muted">{{ t("ai.pending") }}</p>
    <p v-if="notice" role="status" class="text-sm text-muted break-keep">{{ notice }}</p>
    <p v-if="error" role="alert" class="text-sm text-error">{{ error }}</p>
    <div v-if="result" class="rounded-md border border-default p-3" role="region" :aria-label="t('ai.preview.title')">
      <h2 class="text-sm font-medium break-keep">{{ t("ai.preview.title") }} · {{ t(AI_MENU_LABEL[result.action]) }}</h2>
      <ul v-if="items.length > 0" class="mt-2 flex list-disc flex-col gap-1 pl-5 text-sm">
        <li v-for="item in items" :key="item.key" class="break-keep" :data-ai-task-state="item.state">
          <RouterLink v-if="item.href" :to="item.href">{{ item.label }}</RouterLink>
          <template v-else>{{ item.label }}</template>
          <span v-if="item.state === 'created'" class="text-muted"> · {{ t("common.saved") }}</span>
        </li>
      </ul>
      <p v-else class="mt-2 text-sm text-muted break-keep">{{ t("ai.preview.empty") }}</p>
      <p v-if="applyReason && items.length > 0" class="mt-2 text-xs text-muted break-keep">{{ applyReason }}</p>
      <div class="mt-2 flex flex-wrap justify-end gap-2">
        <UButton size="sm" variant="outline" color="neutral" :disabled="createTasks.isPending.value" @click="result = null">
          {{ t("ai.preview.cancel") }}
        </UButton>
        <UButton v-if="items.length > 0" size="sm" :disabled="!canApply || createTasks.isPending.value" @click="apply">
          {{ t(AI_APPLY_LABEL[result.action]) }}
        </UButton>
      </div>
    </div>
  </section>
</template>
