import { useQuery } from "@tanstack/react-query";
import { useParams } from "react-router-dom";
import { workspacesQuery } from "@/lib/queries";

export function useWorkspaceContext() {
  const { slug } = useParams<{ slug: string }>();
  const workspaces = useQuery(workspacesQuery);
  const current = workspaces.data?.items.find((item) => item.slug === slug);
  return {
    slug: slug ?? "",
    workspace: current,
    workspaces,
  };
}
