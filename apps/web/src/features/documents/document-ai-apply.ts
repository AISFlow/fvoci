/**
 * Source `DocumentAiMenu` apply step, kept free of React so the rules are unit-tested: a confirmed
 * summary or link suggestion becomes editor nodes appended at the end of the live body, and a
 * confirmed task list is created one title at a time without recreating titles that already
 * succeeded (the source loop restarted from the first title on retry).
 */

export type InsertNode = {
  type: "paragraph";
  content: Array<
    | { type: "text"; text: string }
    | { type: "mention"; attrs: { entity: "document"; id: string; label: string } }
  >;
};

export type InsertPreview =
  | { action: "summarize"; lines: readonly string[] }
  | { action: "suggestLinks"; links: ReadonlyArray<{ id: string; label: string }> };

/** Summary lines become paragraphs; link suggestions one paragraph of document mentions. */
export function aiInsertNodes(preview: InsertPreview): InsertNode[] {
  if (preview.action === "summarize") {
    return preview.lines
      .filter((line) => line.length > 0)
      .map((line) => ({ type: "paragraph", content: [{ type: "text", text: line }] }));
  }
  if (preview.links.length === 0) return [];
  return [
    {
      type: "paragraph",
      content: preview.links.flatMap((link, index) => [
        ...(index === 0 ? [] : [{ type: "text" as const, text: " " }]),
        {
          type: "mention" as const,
          attrs: { entity: "document" as const, id: link.id, label: link.label },
        },
      ]),
    },
  ];
}

/** The slice of a ProseMirror document `appendRange` reads. */
export interface AppendTarget {
  content: { size: number };
  lastChild: { type: { name: string }; content: { size: number }; nodeSize: number } | null;
}

/**
 * Where appended blocks go: after the last top-level block, so they never land inside a list,
 * code block or table and never split or merge the last paragraph. An empty last paragraph (the
 * editor's trailing placeholder) is replaced instead of leaving a blank line before the result.
 */
export function appendRange(doc: AppendTarget): { from: number; to: number } {
  const end = doc.content.size;
  const last = doc.lastChild;
  if (last && last.type.name === "paragraph" && last.content.size === 0) {
    return { from: end - last.nodeSize, to: end };
  }
  return { from: end, to: end };
}

/**
 * `pending` — not created yet (retry creates it); `created` — the server answered success;
 * `unknown` — the request may have committed (no answer or a server error), so a retry must not
 * send it again.
 */
export type TaskApplyState = "pending" | "created" | "unknown";

export type TaskApplyFailure = { kind: "definite" | "unknown"; error: unknown };

/**
 * Creates the still-pending titles in order and stops at the first failure. A 4xx answer is a
 * definite rejection and stays `pending`; anything else may have been committed and becomes
 * `unknown`.
 */
export async function applyTaskTitles(
  titles: readonly string[],
  states: readonly TaskApplyState[],
  create: (title: string) => Promise<void>,
  isDefiniteRejection: (error: unknown) => boolean,
  onProgress?: (states: TaskApplyState[]) => void,
): Promise<{ states: TaskApplyState[]; created: number; failure: TaskApplyFailure | null }> {
  const next = titles.map((_, index) => states[index] ?? "pending");
  let created = 0;
  for (let index = 0; index < titles.length; index += 1) {
    if (next[index] !== "pending") continue;
    try {
      await create(titles[index]!);
    } catch (error) {
      const definite = isDefiniteRejection(error);
      if (!definite) next[index] = "unknown";
      onProgress?.([...next]);
      return { states: next, created, failure: { kind: definite ? "definite" : "unknown", error } };
    }
    next[index] = "created";
    created += 1;
    onProgress?.([...next]);
  }
  return { states: next, created, failure: null };
}

export function hasPendingTask(states: readonly TaskApplyState[]): boolean {
  return states.includes("pending");
}

/** 4xx answers are rejections before commit; 5xx, missing bodies and network errors are not. */
export function isDefiniteStatus(status: number | null): boolean {
  return status !== null && status >= 400 && status < 500;
}
