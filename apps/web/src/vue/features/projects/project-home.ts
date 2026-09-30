import type { Project, TreeNode } from "@/features/projects/queries";

/** Direct children of the project's root document, as the React home lists them. */
export function projectHomeChildNodes(nodes: readonly TreeNode[], project: Project): TreeNode[] {
  return nodes.filter(
    (node) => node.parentId === project.rootDocumentId && node.projectId === project.id,
  );
}
