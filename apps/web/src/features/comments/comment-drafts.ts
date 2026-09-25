export type CommentDrafts = { main: string; reply: string };

export function updateIsolatedDraft(
  drafts: CommentDrafts,
  target: "main" | "reply",
  value: string,
): CommentDrafts {
  if (target === "main") return { main: value, reply: drafts.reply };
  return { main: drafts.main, reply: value };
}

export function nextReplyTarget(current: string | null, commentId: string): string | null {
  return current === commentId ? null : commentId;
}
