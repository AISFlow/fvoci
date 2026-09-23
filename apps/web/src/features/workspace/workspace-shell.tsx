import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { settingsPath, wikiPath } from "@/lib/href";
import { api, ProblemError, problemMessage } from "@/lib/api";
import { workspacesQuery } from "@/lib/queries";
import "@/features/workspace/workspace-aux.css";

export type WorkspaceNav = "wiki" | "settings";

interface WorkspaceShellProps {
  slug: string;
  workspaceId: string;
  workspaceName: string;
  activeNav: WorkspaceNav;
  children: React.ReactNode;
}

export function WorkspaceShell({
  slug,
  workspaceId,
  workspaceName,
  activeNav,
  children,
}: WorkspaceShellProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const workspaces = useQuery(workspacesQuery);
  const items = workspaces.data?.items ?? [];
  const [logoutError, setLogoutError] = useState<string | null>(null);

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

  return (
    <div className="app-shell">
      {logoutError ? (
        <div
          role="alert"
          className="border-b border-border bg-muted px-4 py-2 text-ui text-muted-foreground"
        >
          {logoutError}
        </div>
      ) : null}
      <header className="app-shell__header workspace-shell__header">
        <div className="workspace-shell__brand">
          <Link to="/" className="text-ui underline underline-offset-2">
            {t("nav.backHome")}
          </Link>
          <nav className="workspace-shell__nav" aria-label={t("nav.workspace")}>
            <Link
              to={wikiPath(slug)}
              className={activeNav === "wiki" ? "workspace-shell__nav-link is-active" : "workspace-shell__nav-link"}
              aria-current={activeNav === "wiki" ? "page" : undefined}
            >
              {t("nav.wiki")}
            </Link>
            <Link
              to={settingsPath(slug)}
              className={activeNav === "settings" ? "workspace-shell__nav-link is-active" : "workspace-shell__nav-link"}
              aria-current={activeNav === "settings" ? "page" : undefined}
            >
              {t("nav.settings")}
            </Link>
          </nav>
        </div>
        <div className="workspace-shell__controls">
          {items.length > 1 ? (
            <div className="workspace-shell__switch">
              <Label htmlFor="workspace-switch" className="sr-only">
                {t("workspace.switch")}
              </Label>
              <select
                id="workspace-switch"
                className="workspace-shell__select"
                value={workspaceId}
                onChange={(event) => {
                  const next = items.find((item) => item.id === event.target.value);
                  if (!next) return;
                  void navigate(activeNav === "settings" ? settingsPath(next.slug) : wikiPath(next.slug));
                }}
              >
                {items.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name}
                  </option>
                ))}
              </select>
            </div>
          ) : (
            <span className="workspace-shell__name">{workspaceName}</span>
          )}
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => {
              void logout();
            }}
          >
            {t("nav.logout")}
          </Button>
        </div>
      </header>
      <main className="app-shell__main">{children}</main>
    </div>
  );
}
