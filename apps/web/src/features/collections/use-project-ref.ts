import { useQuery } from "@tanstack/react-query";
import { useParams } from "react-router-dom";
import { findProjectByKey, projectsQuery } from "@/features/projects/queries";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { ProblemError } from "@/lib/api";
import { parseRef } from "@/lib/href";

/** Resolve `/w/:slug/:ref/...` to the workspace project (same rules as the task list page). */
export function useProjectRef() {
  const { ref } = useParams<{ ref: string }>();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");
  const projectKey = parsed?.kind === "project" ? parsed.key : null;
  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const project = findProjectByKey(projects.data?.items, projectKey ?? "");
  const notFound =
    projectKey === null ||
    (projects.isSuccess && project === undefined) ||
    (projects.isError && projects.error instanceof ProblemError && projects.error.status === 404);
  return { slug, workspace, projects, project, notFound };
}
