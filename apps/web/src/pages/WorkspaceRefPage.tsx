import { Navigate, useParams } from "react-router-dom";
import { DocumentPage } from "@/pages/DocumentPage";
import { TaskDetailPage } from "@/pages/TaskDetailPage";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { parseRef, projectTasksPath, wikiPath } from "@/lib/href";

export function WorkspaceRefPage() {
  const { ref } = useParams<{ ref: string }>();
  const { slug } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");

  if (parsed?.kind === "item" && parsed.prefix === "WIKI") {
    return <DocumentPage />;
  }
  if (parsed?.kind === "item") {
    return <TaskDetailPage />;
  }
  if (parsed?.kind === "project") {
    return <Navigate to={projectTasksPath(slug, parsed.key)} replace />;
  }
  return <Navigate to={wikiPath(slug)} replace />;
}
