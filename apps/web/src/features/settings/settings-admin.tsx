// Adapted from source apps/web/src/features/settings/settings-admin.tsx.
// Account erasure (admin erase / cancel-erase) belongs to the account
// lifecycle slice and is not wired here yet; its controls are left out.
import { formatPersonName, t } from "@fvoci/i18n";
import { useState } from "react";
import { ConfirmActionButton } from "@/components/confirm-action";
import { QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import type { components } from "@/generated/api";
import { ProblemError } from "@/lib/api";
import type { BrandingAssetKind } from "./settings-catalog";
import { InstanceSettingsView } from "./settings-instance";
import "./settings-shell.css";

type AdminUser = components["schemas"]["AdminUserItemOutput"];
type AdminWorkspace = components["schemas"]["AdminWorkspaceItemOutput"];
type AdminSystem = components["schemas"]["AdminSystemOutput"];
type AdminInstanceSettings = components["schemas"]["AdminInstanceSettingsOutput"];

export type AdminUserPatch = { instanceAdmin?: boolean; suspended?: boolean };

/** Codes the shared problem table does not title; this screen owns their wording. */
export function adminActionMessage(err: unknown): string {
  if (err instanceof ProblemError) {
    if (err.code === "last_instance_admin") return t("admin.lastAdmin");
    if (err.code === "self_suspension") return t("self_suspension");
    return err.title;
  }
  return t("error.network");
}

const cellClass = "border-b border-border px-2 py-2 align-top text-ui";
const headClass = "border-b border-border px-2 py-2 text-left text-caption font-medium text-muted-foreground";

export function AdminSettingsView({
  users,
  workspaces,
  system,
  loading,
  error,
  pending,
  onPatchUser,
  settings,
  settingsPending,
  settingsLoading = false,
  settingsError = null,
  onRetrySettings,
  onSaveSettings,
  onSettingsAsset,
}: {
  users: AdminUser[];
  workspaces: AdminWorkspace[];
  system: AdminSystem | null;
  loading: boolean;
  error: string | null;
  pending: boolean;
  onPatchUser: (userId: string, patch: AdminUserPatch) => Promise<void>;
  settings: AdminInstanceSettings | null;
  settingsPending: boolean;
  settingsLoading?: boolean;
  settingsError?: string | null;
  onRetrySettings?: () => void;
  onSaveSettings: (patch: Record<string, unknown>) => Promise<void>;
  onSettingsAsset: (kind: BrandingAssetKind, file: File | null) => Promise<void>;
}) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);
  const busy = pending || changing;

  async function patchUser(userId: string, patch: AdminUserPatch): Promise<void> {
    if (busy) return;
    setActionError(null);
    setChanging(true);
    try {
      await onPatchUser(userId, patch);
    } catch (err) {
      setActionError(adminActionMessage(err));
    } finally {
      setChanging(false);
    }
  }

  return (
    <div className="settings-stack">
      <section className="settings-section" aria-labelledby="admin-system-title">
        <h2 className="settings-section__title text-title" id="admin-system-title">
          {t("admin.system")}
        </h2>
        <div className="text-ui">
          {loading ? <QueryLoading /> : null}
          {system ? (
            <ul className="flex flex-col gap-1 settings-tabular">
              <li>
                {t("admin.users")}: {system.users}
              </li>
              <li>
                {t("admin.workspaces")}: {system.workspaces}
              </li>
              <li>
                {t("admin.documents")}: {system.documents}
              </li>
              <li>
                {t("admin.tasks")}: {system.tasks}
              </li>
            </ul>
          ) : null}
          {error ? (
            <p role="alert" className="text-destructive">
              {error}
            </p>
          ) : null}
        </div>
      </section>
      <section className="settings-section" aria-labelledby="admin-users-title">
        <h2 className="settings-section__title text-title" id="admin-users-title">
          {t("admin.users")}
        </h2>
        <div className="flex flex-col gap-2 overflow-x-auto">
          {actionError ? (
            <p role="alert" className="settings-notice settings-notice--danger">
              {actionError}
            </p>
          ) : null}
          <table className="w-full border-collapse">
            <thead>
              <tr>
                <th className={headClass}>{t("admin.email")}</th>
                <th className={headClass}>{t("admin.instanceAdmin")}</th>
                <th className={headClass}>{t("admin.suspended")}</th>
                <th className={headClass} />
              </tr>
            </thead>
            <tbody>
              {users.map((u) => {
                const deleted = u.deletedAt != null;
                const name = formatPersonName(u);
                return (
                  <tr key={u.id}>
                    <td className={cellClass}>
                      {name} ({u.email})
                    </td>
                    <td className={cellClass}>{u.instanceAdmin ? t("admin.yes") : t("admin.no")}</td>
                    <td className={cellClass}>{u.suspendedAt ? t("admin.yes") : t("admin.no")}</td>
                    <td className={`${cellClass} text-right`}>
                      <div className="flex flex-wrap justify-end gap-2">
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          aria-pressed={u.instanceAdmin}
                          aria-label={`${t("admin.instanceAdmin")}: ${u.email}`}
                          disabled={busy || deleted}
                          onClick={() => void patchUser(u.id, { instanceAdmin: !u.instanceAdmin })}
                        >
                          {t("admin.instanceAdmin")}
                        </Button>
                        <ConfirmActionButton
                          title={
                            u.suspendedAt ? t("admin.restore.confirm.title") : t("admin.suspend.confirm.title")
                          }
                          description={
                            u.suspendedAt ? t("admin.restore.confirm.body") : t("admin.suspend.confirm.body")
                          }
                          actionLabel={u.suspendedAt ? t("admin.restore") : t("admin.suspend")}
                          disabled={busy || deleted}
                          destructive={!u.suspendedAt}
                          onConfirm={() => patchUser(u.id, { suspended: !u.suspendedAt })}
                        >
                          {u.suspendedAt ? t("admin.restore") : t("admin.suspend")}
                        </ConfirmActionButton>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </section>
      <section className="settings-section" aria-labelledby="admin-workspaces-title">
        <h2 className="settings-section__title text-title" id="admin-workspaces-title">
          {t("admin.workspaces")}
        </h2>
        <div className="overflow-x-auto">
          <table className="w-full border-collapse">
            <thead>
              <tr>
                <th className={headClass}>{t("workspace.name")}</th>
                <th className={headClass}>{t("admin.slug")}</th>
              </tr>
            </thead>
            <tbody>
              {workspaces.map((w) => (
                <tr key={w.id}>
                  <td className={cellClass}>{w.name}</td>
                  <td className={`${cellClass} settings-tabular`}>{w.slug}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
      <InstanceSettingsView
        data={settings}
        error={settingsError}
        loading={settingsLoading}
        onRetry={onRetrySettings}
        onAsset={onSettingsAsset}
        onSave={onSaveSettings}
        pending={settingsPending}
      />
    </div>
  );
}
