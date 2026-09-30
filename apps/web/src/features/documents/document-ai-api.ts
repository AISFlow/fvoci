import type { I18nKey } from "@fvoci/i18n";
import { api, ensureOk } from "@/lib/api";
import type { TaskApplyState } from "./document-ai-apply";

// Document AI requests shared by the React and Vue AI menus (source
// `DocumentAiMenu`); the apply rules are in document-ai-apply.ts.

export type AiAction = "summarize" | "generateTasks" | "suggestLinks";

export type AiResult =
  | { action: "summarize"; lines: string[] }
  | { action: "generateTasks"; titles: string[]; states: TaskApplyState[] }
  | { action: "suggestLinks"; documents: Array<{ id: string; title: string }> };

export const AI_ACTIONS: readonly AiAction[] = ["summarize", "generateTasks", "suggestLinks"];

export const AI_MENU_LABEL: Record<AiAction, I18nKey> = {
  summarize: "ai.summarize",
  generateTasks: "ai.generateTasks",
  suggestLinks: "ai.suggestLinks",
};

export const AI_APPLY_LABEL: Record<AiAction, I18nKey> = {
  summarize: "ai.apply.summarize",
  generateTasks: "ai.apply.generateTasks",
  suggestLinks: "ai.apply.suggestLinks",
};

/** Runs one AI action against the saved document. */
export async function runAiAction(
  workspaceId: string,
  documentId: string,
  action: AiAction,
): Promise<AiResult> {
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
  // WHY: labels come from the server's permission-filtered titles, never from the wiki tree —
  // project documents are not in it, and a raw id would persist as the mention label.
  const { documents } = await ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/ai/suggest-links", init),
  );
  return { action, documents };
}

/** Creates one confirmed task title in the document's project. */
export async function createAiTask(workspaceId: string, projectId: string, title: string) {
  await ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks", {
      params: { path: { workspace_id: workspaceId, project_id: projectId } },
      body: { title, type: "task", priority: "none" },
    }),
  );
}
