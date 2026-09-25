import { useQuery } from "@tanstack/react-query";
import { Navigate, useParams } from "react-router-dom";
import { DocumentAiMenu } from "@/features/documents/document-ai-menu";
import { DocumentView } from "@/features/documents/document-view";
import { CollabRoom } from "@/features/documents/collab-session";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { parseWikiRef, wikiPath } from "@/lib/href";
import { treeQuery } from "@/lib/queries/documents";

export function DocumentPage() {
  const { ref } = useParams<{ ref: string }>();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = ref ? parseWikiRef(ref) : null;
  const tree = useQuery(treeQuery(workspace?.id ?? ""));

  if (!workspace) return null;

  if (!parsed || parsed.prefix !== "WIKI") {
    return <Navigate to={wikiPath(slug)} replace />;
  }

  const node = tree.data?.items.find(
    (item) => item.projectId === null && item.number === parsed.number,
  );

  if (tree.isLoading) {
    return (
      <WorkspaceShell
        slug={slug}
        workspaceId={workspace.id}
        workspaceName={workspace.name}
        activeNav="wiki"
      >
        <p className="text-muted-foreground">…</p>
      </WorkspaceShell>
    );
  }

  if (!node) {
    return <Navigate to={wikiPath(slug)} replace />;
  }

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="wiki"
    >
      <CollabRoom workspaceId={workspace.id} kind="document" id={node.id}>
        <DocumentView workspaceId={workspace.id} slug={slug} documentId={node.id} />
      </CollabRoom>
      <DocumentAiMenu workspaceId={workspace.id} slug={slug} documentId={node.id} />
    </WorkspaceShell>
  );
}
