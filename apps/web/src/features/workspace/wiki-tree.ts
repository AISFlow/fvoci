export type WikiTreeNode = {
  id: string;
  parentId?: string | null;
};

export type ChildrenByParent<T extends WikiTreeNode> = Map<string | null, T[]>;

export function childrenByParent<T extends WikiTreeNode>(nodes: readonly T[]): ChildrenByParent<T> {
  const index: ChildrenByParent<T> = new Map();
  for (const node of nodes) {
    const bucket = index.get(node.parentId ?? null);
    if (bucket) bucket.push(node);
    else index.set(node.parentId ?? null, [node]);
  }
  return index;
}

export function childrenOf<T extends WikiTreeNode>(
  index: ChildrenByParent<T>,
  parentId: string | null,
): T[] {
  return index.get(parentId) ?? [];
}
