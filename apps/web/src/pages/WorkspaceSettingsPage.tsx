import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Navigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { WorkspaceIdentitySection } from "@/features/settings/workspace-identity";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { meQuery, workspacesQuery } from "@/lib/queries";
import { useState } from "react";

function roleAtLeast(role: string, minimum: string): boolean {
  const order = ["guest", "member", "admin", "owner"];
  return order.indexOf(role) >= order.indexOf(minimum);
}

export function WorkspaceSettingsPage() {
  const { slug } = useParams<{ slug: string }>();
  const queryClient = useQueryClient();
  const [nameError, setNameError] = useState<string | null>(null);
  const [logoutError, setLogoutError] = useState<string | null>(null);
  const me = useQuery(meQuery);
  const workspaces = useQuery(workspacesQuery);

  if (me.isError) {
    return <Navigate to="/login" replace />;
  }

  const current = workspaces.data?.items.find((item) => item.slug === slug);
  const metaQuery = useQuery({
    queryKey: ["workspaces", current?.id, "meta"],
    enabled: Boolean(current?.id),
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}", {
          params: { path: { workspace_id: current!.id } },
        }),
      ),
    retry: false,
  });

  const rename = useMutation({
    mutationFn: async (name: string) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}", {
          params: { path: { workspace_id: current!.id } },
          body: { name },
        }),
      ),
    onSuccess: async () => {
      setNameError(null);
      await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
      await queryClient.invalidateQueries({ queryKey: ["workspaces", current?.id] });
    },
    onError: (err: unknown) => {
      setNameError(err instanceof ProblemError ? err.title : t("error.network"));
    },
  });

  if (workspaces.isLoading) {
    return <p className="p-8 text-muted-foreground">{t("load.loading")}</p>;
  }

  if (!current) {
    return <Navigate to="/?denied=workspace" replace />;
  }

  if (metaQuery.isError) {
    return <Navigate to="/?denied=workspace" replace />;
  }

  const canManage = roleAtLeast(current.role, "admin");

  return (
    <div className="app-shell">
      {logoutError ? (
        <div role="alert" className="border-b border-border bg-muted px-4 py-2 text-ui text-muted-foreground">
          {logoutError}
        </div>
      ) : null}
      <header className="app-shell__header">
        <Link to="/" className="text-ui underline underline-offset-2">{t("nav.backHome")}</Link>
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={async () => {
            setLogoutError(null);
            const result = await api.POST("/api/v1/auth/logout");
            if (!result.response.ok) {
              setLogoutError(
                problemMessage(new ProblemError(result.response.status), "error.auth.logout"),
              );
              return;
            }
            await queryClient.resetQueries();
            window.location.assign("/login");
          }}
        >
          {t("nav.logout")}
        </Button>
      </header>
      <main className="app-shell__main settings-page">
        <WorkspaceIdentitySection
          workspaceName={metaQuery.data?.name ?? current.name}
          workspaceSlug={metaQuery.data?.slug ?? current.slug}
          workspaceKind={current.kind}
          canManage={canManage}
          namePending={rename.isPending}
          nameError={nameError}
          onSaveName={async (name) => {
            await rename.mutateAsync(name);
          }}
        />
      </main>
    </div>
  );
}
