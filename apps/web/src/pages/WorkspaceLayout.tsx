import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { Navigate, Outlet } from "react-router-dom";
import { meQuery } from "@/lib/queries";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";

export function WorkspaceLayout() {
  const me = useQuery(meQuery);
  const { workspaces, workspace } = useWorkspaceContext();

  if (me.isError) {
    return <Navigate to="/login" replace />;
  }

  if (workspaces.isLoading || me.isLoading) {
    return <p className="p-8 text-muted-foreground">{t("load.loading")}</p>;
  }

  if (!workspace) {
    return <Navigate to="/?denied=workspace" replace />;
  }

  return <Outlet context={{ workspace }} />;
}
