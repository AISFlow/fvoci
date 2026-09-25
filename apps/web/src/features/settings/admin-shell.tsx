// Instance-admin area frame: the source's account settings nav entries for
// instance admins (settings-nav.tsx: legal, audit, admin) under one header.
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Link, Navigate } from "react-router-dom";
import { QueryLoading } from "@/components/query-status";
import { meQuery } from "@/lib/queries";
import "@/features/documents/document-shell.css";
import "./settings-shell.css";

export type AdminNav = "admin" | "audit" | "legal";

const NAV: { key: AdminNav; to: string; label: () => string }[] = [
  { key: "admin", to: "/settings/admin", label: () => t("settings.admin") },
  { key: "audit", to: "/settings/audit", label: () => t("audit.title") },
  { key: "legal", to: "/settings/legal", label: () => t("settings.legal") },
];

/**
 * Renders children only for a signed-in instance admin; everyone else goes
 * home (source `beforeLoad`: `if (!me.isInstanceAdmin) redirect("/")`). The
 * server still answers 404 to non-admins on every /admin route.
 */
export function AdminShell({ active, children }: { active: AdminNav; children: ReactNode }) {
  const me = useQuery(meQuery);
  if (me.isError) return <Navigate to="/login" replace />;
  if (me.isLoading || !me.data) return <QueryLoading />;
  if (!me.data.isInstanceAdmin) return <Navigate to="/" replace />;

  return (
    <div className="app-shell">
      <header className="app-shell__header workspace-shell__header">
        <div className="workspace-shell__brand">
          <Link to="/" className="text-ui underline underline-offset-2">
            {t("nav.backHome")}
          </Link>
          <nav className="workspace-shell__nav" aria-label={t("admin.console")}>
            {NAV.map((item) => (
              <Link
                key={item.key}
                to={item.to}
                className={active === item.key ? "workspace-shell__nav-link is-active" : "workspace-shell__nav-link"}
                aria-current={active === item.key ? "page" : undefined}
              >
                {item.label()}
              </Link>
            ))}
          </nav>
        </div>
      </header>
      <main className="app-shell__main">
        <div className="settings-page">{children}</div>
      </main>
    </div>
  );
}
