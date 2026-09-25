import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import "../settings/settings-shell.css";

const ROLES = ["lead", "member", "viewer"] as const;

function roleLabel(role: string): string {
  if (role === "lead") return t("projectRole.lead");
  if (role === "member") return t("projectRole.member");
  return t("projectRole.viewer");
}

export function ProjectGroupsSection({
  workspaceId,
  projectId,
}: {
  workspaceId: string;
  projectId: string;
}) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const groupsQuery = useQuery({
    queryKey: ["workspaces", workspaceId, "groups"],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/groups", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
  });
  const grantsQuery = useQuery({
    queryKey: ["project-group-grants", workspaceId, projectId],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    retry: false,
  });

  const groups = groupsQuery.data?.items ?? [];
  const grants = grantsQuery.data?.items ?? [];
  const grantedIds = new Set(grants.map((grant) => grant.groupId));
  const available = groups.filter((group) => !grantedIds.has(group.id));
  const nameById = new Map(groups.map((group) => [group.id, group.name]));

  async function refresh() {
    await queryClient.invalidateQueries({
      queryKey: ["project-group-grants", workspaceId, projectId],
    });
  }

  const grant = useMutation({
    mutationFn: async (input: { groupId: string; role: string }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
          body: input,
        }),
      ),
    onSuccess: async () => {
      setError(null);
      await refresh();
    },
    onError: (err) => setError(err instanceof ProblemError ? err.title : t("error.network")),
  });
  const revoke = useMutation({
    mutationFn: async (groupId: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
          body: { groupId },
        }),
      ),
    onSuccess: async () => {
      setError(null);
      await refresh();
    },
    onError: (err) => setError(err instanceof ProblemError ? err.title : t("error.network")),
  });

  return (
    <details className="settings-disclosure mt-6">
      <summary className="settings-disclosure__summary">{t("group.grant")}</summary>
      <div className="settings-disclosure__body">
        {grantsQuery.isPending ? <p role="status">{t("load.loading")}</p> : null}
        {grants.length === 0 && !grantsQuery.isPending ? (
          <p className="text-ui text-muted-foreground">
            {groups.length === 0 ? t("project.groups.empty") : t("group.grants.empty")}
          </p>
        ) : (
          <ul className="flex flex-col divide-y text-ui">
            {grants.map((row) => (
              <li key={row.groupId} className="flex items-center justify-between gap-2 py-2">
                <span>
                  {nameById.get(row.groupId) ?? row.groupId} · {roleLabel(row.role)}
                </span>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={revoke.isPending}
                  onClick={() => void revoke.mutateAsync(row.groupId)}
                >
                  {t("project.groups.remove")}
                </Button>
              </li>
            ))}
          </ul>
        )}
        {available.length === 0 ? (
          groups.length === 0 ? null : (
            <p className="text-ui text-muted-foreground">{t("group.grant.noneLeft")}</p>
          )
        ) : (
          <form
            className="mt-3 flex flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              const data = new FormData(event.currentTarget);
              const groupId = data.get("groupId");
              const role = data.get("role");
              if (typeof groupId !== "string" || groupId === "" || typeof role !== "string") return;
              void grant.mutateAsync({ groupId, role }).then(() => event.currentTarget.reset());
            }}
          >
            <div>
              <Label htmlFor="project-group">{t("project.groups.label")}</Label>
              <select
                id="project-group"
                name="groupId"
                className="h-11 min-w-48 rounded-md border border-input bg-background px-3 text-ui"
                defaultValue=""
              >
                <option value="">{t("project.groups.label")}</option>
                {available.map((group) => (
                  <option key={group.id} value={group.id}>
                    {group.name}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <Label htmlFor="project-group-role">{t("project.groups.role")}</Label>
              <select
                id="project-group-role"
                name="role"
                aria-label={t("project.groups.role")}
                className="h-11 min-w-28 rounded-md border border-input bg-background px-3 text-ui"
                defaultValue="member"
              >
                {ROLES.map((role) => (
                  <option key={role} value={role}>
                    {roleLabel(role)}
                  </option>
                ))}
              </select>
            </div>
            <Button type="submit" size="sm" disabled={grant.isPending}>
              {t("group.grant")}
            </Button>
          </form>
        )}
        {error ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {error}
          </p>
        ) : null}
      </div>
    </details>
  );
}
