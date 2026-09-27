import type { TiptapEditor } from "@fvoci/editor/fvoci-editor";
import { type I18nKey, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { documentPath, wikiDisplayId } from "@/lib/href";
import { treeQuery } from "@/lib/queries/documents";
import { aiEnabledQuery } from "@/lib/queries/instance-settings";
import {
  aiInsertNodes,
  applyTaskTitles,
  hasPendingTask,
  isDefiniteStatus,
  type TaskApplyState,
} from "./document-ai-apply";

type AiAction = "summarize" | "generateTasks" | "suggestLinks";

type AiResult =
  | { action: "summarize"; lines: string[] }
  | { action: "generateTasks"; titles: string[]; states: TaskApplyState[] }
  | { action: "suggestLinks"; documentIds: string[] };

const ACTIONS: readonly AiAction[] = ["summarize", "generateTasks", "suggestLinks"];
const MENU_LABEL: Record<AiAction, I18nKey> = {
  summarize: "ai.summarize",
  generateTasks: "ai.generateTasks",
  suggestLinks: "ai.suggestLinks",
};
const APPLY_LABEL: Record<AiAction, I18nKey> = {
  summarize: "ai.apply.summarize",
  generateTasks: "ai.apply.generateTasks",
  suggestLinks: "ai.apply.suggestLinks",
};

/** Project whose document this is; tasks are created there. Absent for wiki documents. */
export interface AiTaskProject {
  id: string;
  canCreateTasks: boolean;
}

/**
 * Source `DocumentAiMenu`: the three AI actions run against the saved document, the user reviews
 * the result, and only on confirmation it is applied — summary paragraphs or document mentions are
 * appended at the end of the live collaborative editor (never a body JSON rewrite), and generated
 * titles become tasks through the project task route. Like the source, the menu renders only once
 * the public `features.ai` setting is `true` — while loading, on error or when off it is absent.
 *
 * The owner mounts it inside the collab room, so a document switch or reconnect remounts it and a
 * result can only be applied to the document it was generated for.
 */
export function DocumentAiMenu({
  workspaceId,
  slug,
  documentId,
  project,
  editor,
  insertBlockedReason,
}: {
  workspaceId: string;
  slug: string;
  documentId: string;
  project: AiTaskProject | null;
  /** Live body editor; `null` until it is attached. */
  editor: TiptapEditor | null;
  /** Why inserting is not possible now (read-only, not connected yet); `null` when it is. */
  insertBlockedReason: string | null;
}) {
  const queryClient = useQueryClient();
  const aiEnabled = useQuery(aiEnabledQuery);
  const tree = useQuery(treeQuery(workspaceId));
  const [result, setResult] = useState<AiResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // WHY: the public flag is only a UI gate; the server keeps its own AI gate and answers 503
  // ai_unavailable when that is off — lock the buttons from then on.
  const [unavailable, setUnavailable] = useState(false);
  // WHY: a second click can land before the pending state re-renders; this closes that window.
  const applying = useRef(false);

  const run = useMutation({
    mutationFn: async (action: AiAction): Promise<AiResult> => {
      const init = {
        params: { path: { workspace_id: workspaceId } },
        body: { documentId },
      };
      if (action === "summarize") {
        const { summary } = await ensureOk(
          await api.POST("/api/v1/workspaces/{workspace_id}/ai/summarize", init),
        );
        return {
          action,
          lines: summary
            .split("\n")
            .map((line) => line.trim())
            .filter((line) => line.length > 0),
        };
      }
      if (action === "generateTasks") {
        const { titles } = await ensureOk(
          await api.POST("/api/v1/workspaces/{workspace_id}/ai/generate-tasks", init),
        );
        return { action, titles, states: titles.map(() => "pending") };
      }
      const { documentIds } = await ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/ai/suggest-links", init),
      );
      return { action, documentIds };
    },
    onMutate: () => {
      setError(null);
      setNotice(null);
      setResult(null);
    },
    onSuccess: setResult,
    onError: (err: unknown) => {
      if (err instanceof ProblemError && (err.status === 503 || err.code === "ai_unavailable")) {
        setUnavailable(true);
        setError(t("ai unavailable"));
        return;
      }
      setError(problemMessage(err, "ai.failed"));
    },
  });

  const createTasks = useMutation({
    mutationFn: async (current: Extract<AiResult, { action: "generateTasks" }>) => {
      if (!project) throw new Error("project document required");
      const projectId = project.id;
      try {
        return await applyTaskTitles(
          current.titles,
          current.states,
          async (title) => {
            await ensureOk(
              await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks", {
                params: { path: { workspace_id: workspaceId, project_id: projectId } },
                body: { title, type: "task", priority: "none" },
              }),
            );
          },
          (err) => isDefiniteStatus(err instanceof ProblemError ? err.status : null),
          (states) => setResult({ ...current, states }),
        );
      } finally {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId, projectId] }),
          queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] }),
        ]);
      }
    },
    onSettled: () => {
      applying.current = false;
    },
    onSuccess: (outcome, current) => {
      const createdTotal = outcome.states.filter((state) => state === "created").length;
      if (outcome.failure === null) {
        setResult(null);
        setError(null);
        setNotice(t("ai.tasks.done", { count: createdTotal }));
        return;
      }
      setResult({ ...current, states: outcome.states });
      setNotice(createdTotal > 0 ? t("ai.tasks.done", { count: createdTotal }) : null);
      setError(
        outcome.failure.kind === "unknown"
          ? t("error.network")
          : problemMessage(outcome.failure.error, "ai.tasks.failed"),
      );
    },
    onError: (err: unknown) => {
      setError(problemMessage(err, "ai.tasks.failed"));
    },
  });

  if (aiEnabled.data !== true) return null;

  const nodes = new Map((tree.data?.items ?? []).map((node) => [node.id, node]));
  const links =
    result?.action === "suggestLinks"
      ? result.documentIds.map((id) => {
          const node = nodes.get(id);
          return {
            id,
            label: node?.title || id,
            href:
              node && node.projectId === null
                ? documentPath(slug, wikiDisplayId(node.number))
                : undefined,
          };
        })
      : [];

  const items: Array<{ key: string; label: string; href?: string; state?: TaskApplyState }> = [];
  if (result?.action === "summarize") {
    result.lines.forEach((line, index) => items.push({ key: `${index}`, label: line }));
  } else if (result?.action === "generateTasks") {
    result.titles.forEach((title, index) =>
      items.push({ key: `${index}`, label: title, state: result.states[index] }),
    );
  } else if (result?.action === "suggestLinks") {
    for (const link of links) items.push({ key: link.id, ...link });
  }

  const taskReason = project === null ? t("ai.tasks.noProject") : null;
  const insertReason = editor === null ? t("ai.document.loading") : insertBlockedReason;
  let applyReason: string | null = null;
  let canApply = false;
  if (result?.action === "generateTasks") {
    applyReason = taskReason ?? (project?.canCreateTasks ? null : t("doc.readOnly"));
    canApply = applyReason === null && hasPendingTask(result.states);
  } else if (result) {
    applyReason = insertReason;
    canApply = applyReason === null && items.length > 0;
  }

  function apply() {
    if (!result || !canApply || applying.current) return;
    setError(null);
    setNotice(null);
    if (result.action === "generateTasks") {
      applying.current = true;
      createTasks.mutate(result);
      return;
    }
    if (!editor || editor.isDestroyed) {
      setError(t("ai.document.loading"));
      return;
    }
    const content =
      result.action === "summarize"
        ? aiInsertNodes({ action: "summarize", lines: result.lines })
        : aiInsertNodes({ action: "suggestLinks", links });
    // WHY: the confirmed result goes to the end of the body, not the caret — it never splits the
    // sentence being edited. The insert is a normal editor transaction, so Yjs syncs it like typing.
    applying.current = true;
    try {
      editor.chain().focus("end").insertContent(content).run();
    } finally {
      applying.current = false;
    }
    setResult(null);
    setNotice(t("ai.insert.done"));
  }

  return (
    <section className="document-ai-menu mt-6 flex flex-col gap-2" aria-label={t("ai.menu")}>
      <div role="group" aria-label={t("ai.menu")} className="flex flex-wrap items-center gap-2">
        <span className="text-ui font-medium">{t("ai.menu")}</span>
        {ACTIONS.map((action) => (
          <Button
            key={action}
            type="button"
            variant="outline"
            size="sm"
            disabled={
              run.isPending ||
              createTasks.isPending ||
              unavailable ||
              (action === "generateTasks" && taskReason !== null)
            }
            onClick={() => run.mutate(action)}
          >
            {t(MENU_LABEL[action])}
          </Button>
        ))}
      </div>
      {taskReason && !unavailable ? (
        <p className="text-caption text-muted-foreground break-keep">{taskReason}</p>
      ) : null}
      {run.isPending ? (
        <p role="status" className="text-ui text-muted-foreground">
          {t("ai.pending")}
        </p>
      ) : null}
      {notice ? (
        <p role="status" className="text-ui text-muted-foreground break-keep">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="text-ui text-destructive">
          {error}
        </p>
      ) : null}
      {result ? (
        <div className="rounded-md border border-border p-3" role="region" aria-label={t("ai.preview.title")}>
          <div className="flex items-center justify-between gap-2">
            <h2 className="text-ui font-medium break-keep">
              {t("ai.preview.title")} · {t(MENU_LABEL[result.action])}
            </h2>
          </div>
          {items.length > 0 ? (
            <ul className="mt-2 flex list-disc flex-col gap-1 pl-5 text-ui">
              {items.map((item) => (
                <li key={item.key} className="break-keep" data-ai-task-state={item.state}>
                  {item.href ? <Link to={item.href}>{item.label}</Link> : item.label}
                  {item.state === "created" ? (
                    <span className="text-muted-foreground"> · {t("common.saved")}</span>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : (
            <p className="mt-2 text-ui text-muted-foreground break-keep">{t("ai.preview.empty")}</p>
          )}
          {applyReason && items.length > 0 ? (
            <p className="mt-2 text-caption text-muted-foreground break-keep">{applyReason}</p>
          ) : null}
          <div className="mt-2 flex flex-wrap justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={createTasks.isPending}
              onClick={() => setResult(null)}
            >
              {t("ai.preview.cancel")}
            </Button>
            {items.length > 0 ? (
              <Button
                type="button"
                size="sm"
                disabled={!canApply || createTasks.isPending}
                onClick={apply}
              >
                {t(APPLY_LABEL[result.action])}
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}
    </section>
  );
}
