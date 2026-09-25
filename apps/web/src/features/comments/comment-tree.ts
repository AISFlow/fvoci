import type { components } from "@/generated/api";

export type CommentOutput = components["schemas"]["CommentOutput"];

export type CommentNode = { comment: CommentOutput; children: CommentNode[] };

export function buildCommentTree(items: readonly CommentOutput[]): CommentNode[] {
  const nodes = new Map<string, CommentNode>();
  for (const comment of items) nodes.set(comment.id, { comment, children: [] });
  const roots: CommentNode[] = [];
  for (const node of nodes.values()) {
    const parentId = node.comment.parentId;
    const parent = parentId == null ? undefined : nodes.get(parentId);
    (parent?.children ?? roots).push(node);
  }
  return roots;
}
