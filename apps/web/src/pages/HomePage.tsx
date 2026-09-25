import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { EmptyWorkspace } from "@/features/workspace/empty-workspace";
import { WorkspaceCreateDialog } from "@/features/workspace/workspace-create-dialog";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { wikiPath } from "@/lib/href";
import { meQuery, workspacesQuery } from "@/lib/queries";
import { useState } from "react";

function DeniedBanner() {
  const [params, setParams] = useSearchParams();
  if (params.get("denied") !== "workspace") return null;
  return (
    <div role="alert" className="border-b border-border bg-muted px-4 py-2 text-ui text-muted-foreground">
      <span className="break-keep">{t("error.denied")}</span>
      <button
        type="button"
        className="ml-3 underline underline-offset-2"
        onClick={() => {
          params.delete("denied");
          setParams(params, { replace: true });
        }}
      >
        {t("common.dismiss")}
      </button>
    </div>
  );
}

export function HomePage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [logoutError, setLogoutError] = useState<string | null>(null);
  const me = useQuery(meQuery);
  const workspaces = useQuery(workspacesQuery);

  if (me.isError) {
    return <Navigate to="/login" replace />;
  }

  async function logout() {
    setLogoutError(null);
    let result;
    try {
      result = await api.POST("/api/v1/auth/logout");
    } catch {
      setLogoutError(t("error.network"));
      return;
    }
    if (!result.response.ok) {
      setLogoutError(
        problemMessage(new ProblemError(result.response.status), "error.auth.logout"),
      );
      return;
    }
    await queryClient.resetQueries();
    await navigate("/login", { replace: true });
  }

  if (workspaces.isLoading || me.isLoading) {
    return <p className="p-8 text-muted-foreground">{t("load.loading")}</p>;
  }

  const items = workspaces.data?.items ?? [];

  if (items.length === 0) {
    return (
      <div className="app-shell">
        <DeniedBanner />
        <main className="app-shell__main">
          {me.data?.isInstanceAdmin ? (
            <Link to="/settings/admin" className="text-ui underline underline-offset-2">
              {t("admin.console")}
            </Link>
          ) : null}
          <EmptyWorkspace
            isAdmin={me.data?.isInstanceAdmin === true}
            onCreate={async (input) => {
              await ensureOk(await api.POST("/api/v1/workspaces", { body: input }));
              await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
            }}
            onLogout={() => {
              void logout();
            }}
            error={logoutError ?? (workspaces.isError ? t("load.listFailed") : null)}
            onRetry={() => {
              void workspaces.refetch();
            }}
          />
        </main>
      </div>
    );
  }

  return (
    <div className="app-shell">
      <DeniedBanner />
      {logoutError ? (
        <div role="alert" className="border-b border-border bg-muted px-4 py-2 text-ui text-muted-foreground">
          {logoutError}
        </div>
      ) : null}
      <header className="app-shell__header">
        <h1 className="text-title font-semibold">{t("dashboard.title")}</h1>
        <div className="flex gap-2">
          {me.data?.isInstanceAdmin ? (
            <Link
              to="/settings/admin"
              className="inline-flex h-8 items-center rounded-md border border-border px-3 text-sm font-medium hover:bg-accent"
            >
              {t("admin.console")}
            </Link>
          ) : null}
          {me.data?.isInstanceAdmin ? (
            <Button type="button" size="sm" onClick={() => setCreateOpen(true)}>
              {t("workspace.create")}
            </Button>
          ) : null}
          <Button type="button" size="sm" variant="outline" onClick={() => void logout()}>
            {t("nav.logout")}
          </Button>
        </div>
      </header>
      <main className="app-shell__main">
        <div className="workspace-list">
          {items.map((workspace) => (
            <Link
              key={workspace.id}
              className="workspace-list__item"
              to={wikiPath(workspace.slug)}
            >
              <div>
                <strong>{workspace.name}</strong>
                <p className="text-dense text-muted-foreground">{workspace.slug}</p>
                <p className="text-dense text-muted-foreground">
                  {t("dashboard.workspace.documentCount", { count: workspace.documentCount })}
                  {" · "}
                  {t("dashboard.workspace.assignedCount", { count: workspace.assignedCount })}
                </p>
              </div>
              <span className="text-caption text-muted-foreground">{workspace.role}</span>
            </Link>
          ))}
        </div>
      </main>
      <WorkspaceCreateDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreate={async (input) => {
          await ensureOk(await api.POST("/api/v1/workspaces", { body: input }));
          await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
        }}
      />
    </div>
  );
}
