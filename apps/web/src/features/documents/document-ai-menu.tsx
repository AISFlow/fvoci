import { type I18nKey, t } from "@fvoci/i18n";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { documentPath, wikiDisplayId } from "@/lib/href";
import { treeQuery } from "@/lib/queries/documents";

type AiAction = "summarize" | "generateTasks" | "suggestLinks";

type AiResult =
  | { action: "summarize"; lines: string[] }
  | { action: "generateTasks"; titles: string[] }
  | { action: "suggestLinks"; documentIds: string[] };

const ACTIONS: readonly AiAction[] = ["summarize", "generateTasks", "suggestLinks"];
const MENU_LABEL: Record<AiAction, I18nKey> = {
  summarize: "ai.summarize",
  generateTasks: "ai.generateTasks",
  suggestLinks: "ai.suggestLinks",
};

/**
 * Source `DocumentAiMenu`, reduced to a read-only preview: the three AI actions run against the
 * saved document and the result is shown in a panel. Applying the result (inserting into the
 * body or creating tasks) is not ported yet.
 */
export function DocumentAiMenu({
  workspaceId,
  slug,
  documentId,
}: {
  workspaceId: string;
  slug: string;
  documentId: string;
}) {
  const tree = useQuery(treeQuery(workspaceId));
  const [result, setResult] = useState<AiResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  // WHY: the server answers 503 ai_unavailable when AI is off — lock the buttons from then on.
  const [unavailable, setUnavailable] = useState(false);

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
        return { action, titles };
      }
      const { documentIds } = await ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/ai/suggest-links", init),
      );
      return { action, documentIds };
    },
    onMutate: () => {
      setError(null);
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

  const nodes = new Map((tree.data?.items ?? []).map((node) => [node.id, node]));
  const items: Array<{ key: string; label: string; href?: string }> = [];
  if (result?.action === "summarize") {
    result.lines.forEach((line, index) => items.push({ key: `${index}`, label: line }));
  } else if (result?.action === "generateTasks") {
    result.titles.forEach((title, index) => items.push({ key: `${index}`, label: title }));
  } else if (result?.action === "suggestLinks") {
    for (const id of result.documentIds) {
      const node = nodes.get(id);
      items.push({
        key: id,
        label: node?.title || id,
        href:
          node && node.projectId === null
            ? documentPath(slug, wikiDisplayId(node.number))
            : undefined,
      });
    }
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
            disabled={run.isPending || unavailable}
            onClick={() => run.mutate(action)}
          >
            {t(MENU_LABEL[action])}
          </Button>
        ))}
      </div>
      {run.isPending ? (
        <p role="status" className="text-ui text-muted-foreground">
          {t("ai.pending")}
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
            <Button type="button" variant="outline" size="sm" onClick={() => setResult(null)}>
              {t("common.dismiss")}
            </Button>
          </div>
          {items.length > 0 ? (
            <ul className="mt-2 flex list-disc flex-col gap-1 pl-5 text-ui">
              {items.map((item) => (
                <li key={item.key} className="break-keep">
                  {item.href ? <Link to={item.href}>{item.label}</Link> : item.label}
                </li>
              ))}
            </ul>
          ) : (
            <p className="mt-2 text-ui text-muted-foreground break-keep">{t("ai.preview.empty")}</p>
          )}
        </div>
      ) : null}
    </section>
  );
}
