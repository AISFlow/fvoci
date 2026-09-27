import { t } from "@fvoci/i18n";
import { Button } from "@/components/ui/button";
import { api, ensureOk } from "@/lib/api";
import "../settings/settings-shell.css";

async function downloadWorkspaceExport(workspaceId: string): Promise<void> {
  const blob = await ensureOk(
    await api.GET("/api/v1/workspaces/{workspace_id}/export", {
      params: { path: { workspace_id: workspaceId } },
      parseAs: "blob",
    }),
  );
  const href = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = href;
  link.download = "fvoci-workspace.zip";
  link.rel = "noopener";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(href);
}

export function WorkspaceExportSection({
  workspaceId,
  canManage,
}: {
  workspaceId: string;
  canManage: boolean;
}) {
  if (!canManage) {
    return null;
  }
  return (
    <section className="settings-section">
      <h2 className="settings-section-title">{t("export.workspace")}</h2>
      <Button type="button" onClick={() => downloadWorkspaceExport(workspaceId)}>
        {t("export.workspace")}
      </Button>
    </section>
  );
}
