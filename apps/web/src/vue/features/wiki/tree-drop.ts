import type { TreeNode } from "@/lib/queries/documents";

export type TreeDrop = { type: "move"; newParentId: string } | { type: "sort"; afterId: string | null };
// Fixed source tree-pdnd resolveTreeDrop semantics: center reparents, edges reorder siblings.
export function resolveTreeDrop(nodes: readonly TreeNode[], sourceId: string, destId: string, position: "top" | "bottom" | "onto"): TreeDrop | null {
  const source = nodes.find(node => node.id === sourceId);
  const dest = nodes.find(node => node.id === destId);
  if (!source || !dest || sourceId === destId || (source.projectId !== null && source.parentId === null)) return null;
  // Reject a cycle before dispatch; the API still authorizes and validates the actual move.
  const seen = new Set<string>();
  let ancestor: TreeNode | undefined = dest;
  while (ancestor && !seen.has(ancestor.id)) {
    if (ancestor.id === sourceId) return null;
    seen.add(ancestor.id);
    ancestor = nodes.find(node => node.id === ancestor?.parentId);
  }
  if (position !== "onto" && source.parentId === dest.parentId && source.projectId === dest.projectId) {
    const siblings = nodes.filter(node => node.parentId === dest.parentId && node.projectId === dest.projectId && node.id !== sourceId);
    const index = siblings.findIndex(node => node.id === destId);
    const afterId = position === "bottom" ? destId : siblings[index - 1]?.id ?? null;
    const oldSiblings = nodes.filter(node => node.parentId === source.parentId && node.projectId === source.projectId);
    const currentAfter = oldSiblings[oldSiblings.findIndex(node => node.id === sourceId) - 1]?.id ?? null;
    return afterId === currentAfter ? null : { type: "sort", afterId };
  }
  return source.parentId === destId ? null : { type: "move", newParentId: destId };
}
