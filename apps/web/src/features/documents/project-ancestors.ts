/** Tree node fields needed to walk a project document's ancestors. */
export interface AncestorNode {
  id: string;
  parentId?: string | null;
}

/** Ancestors of a project document from the project tree, root-first, excluding the project root. */
export function projectAncestors<T extends AncestorNode>(
  nodes: readonly T[],
  documentId: string,
  rootDocumentId: string | null,
): T[] {
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const chain: T[] = [];
  let parentId = byId.get(documentId)?.parentId ?? null;
  while (parentId && parentId !== rootDocumentId && chain.length < nodes.length) {
    const parent = byId.get(parentId);
    if (!parent) break;
    chain.unshift(parent);
    parentId = parent.parentId ?? null;
  }
  return chain;
}
