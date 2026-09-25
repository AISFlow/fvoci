import { formatPersonName, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { MemberOutput } from "@/lib/contracts";
import { membersQuery } from "@/lib/queries";
import "./settings-shell.css";

type GroupOutput = {
  id: string;
  workspaceId: string;
  name: string;
};

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

export function WorkspaceGroupsSection({
  workspaceId,
  canManage,
}: {
  workspaceId: string;
  canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

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
  const members = useQuery(membersQuery(workspaceId));
  const groupMembersQuery = useQuery({
    queryKey: ["workspaces", workspaceId, "groups", selectedGroupId, "members"],
    enabled: Boolean(selectedGroupId),
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members", {
          params: { path: { workspace_id: workspaceId, group_id: selectedGroupId! } },
        }),
      ),
    retry: false,
  });

  const groups = groupsQuery.data?.items ?? [];
  const selected = groups.find((group) => group.id === selectedGroupId) ?? null;
  const memberItems = members.data?.items ?? [];
  const selectedMemberIds = new Set(
    (groupMembersQuery.data?.items ?? []).map((row) => row.userId),
  );
  const candidates = memberItems.filter((member) => !selectedMemberIds.has(member.userId));

  async function refreshGroups() {
    await queryClient.invalidateQueries({ queryKey: ["workspaces", workspaceId, "groups"] });
  }

  async function refreshMembers() {
    await queryClient.invalidateQueries({
      queryKey: ["workspaces", workspaceId, "groups", selectedGroupId, "members"],
    });
  }

  const create = useMutation({
    mutationFn: async (groupName: string) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/groups", {
          params: { path: { workspace_id: workspaceId } },
          body: { name: groupName },
        }),
      ),
    onSuccess: async (created) => {
      setActionError(null);
      setName("");
      setSelectedGroupId(created.id);
      await refreshGroups();
    },
    onError: (err) => setActionError(failMessage(err)),
  });

  async function run(action: () => Promise<void>) {
    setPending(true);
    try {
      await action();
    } finally {
      setPending(false);
    }
  }

  const groupsError =
    groupsQuery.error instanceof ProblemError
      ? groupsQuery.error.title
      : groupsQuery.error
        ? t("group.loadFailed")
        : null;
  const membersError =
    groupMembersQuery.error instanceof ProblemError
      ? groupMembersQuery.error.title
      : groupMembersQuery.error
        ? t("group.membersLoadFailed")
        : null;
  const candidatesError =
    members.error instanceof ProblemError
      ? members.error.title
      : members.error
        ? t("group.candidatesLoadFailed")
        : null;

  return (
    <details className="settings-disclosure">
      <summary className="settings-disclosure__summary">{t("group.management.title")}</summary>
      <div className="settings-disclosure__body">
        {groupsQuery.isPending ? <p role="status">{t("load.loading")}</p> : null}
        {groupsError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {groupsError}
            <Button type="button" variant="outline" size="sm" onClick={() => void groupsQuery.refetch()}>
              {t("load.retry")}
            </Button>
          </p>
        ) : null}
        {groups.length === 0 && !groupsQuery.isPending ? (
          <p className="text-ui text-muted-foreground">{t("group.empty")}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {groups.map((group: GroupOutput) => (
              <li key={group.id}>
                <Button
                  type="button"
                  variant={group.id === selectedGroupId ? "default" : "outline"}
                  size="sm"
                  aria-pressed={group.id === selectedGroupId}
                  disabled={pending}
                  onClick={() => {
                    setSelectedGroupId(group.id);
                    setActionError(null);
                    setConfirmDelete(false);
                  }}
                >
                  {group.name}
                </Button>
              </li>
            ))}
          </ul>
        )}
        {canManage ? (
          <form
            className="settings-form mt-3"
            noValidate
            onSubmit={(event) => {
              event.preventDefault();
              const trimmed = name.trim();
              if (trimmed === "") {
                setActionError(t("group.name.required"));
                return;
              }
              void create.mutateAsync(trimmed);
            }}
          >
            <p className="text-ui font-medium">{t("group.add")}</p>
            <div className="settings-form__row">
              <div>
                <Label htmlFor="group-name">{t("group.name")}</Label>
                <Input
                  id="group-name"
                  value={name}
                  disabled={pending || create.isPending}
                  onChange={(event) => setName(event.target.value)}
                />
              </div>
              <Button type="submit" size="sm" disabled={pending || create.isPending}>
                {t("group.create")}
              </Button>
            </div>
          </form>
        ) : null}
        {selected ? (
          <section className="mt-4 flex flex-col gap-3">
            <h2 className="text-ui font-medium">{selected.name}</h2>
            {membersError ? (
              <p role="alert" className="settings-notice settings-notice--danger">
                {membersError}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => void groupMembersQuery.refetch()}
                >
                  {t("load.retry")}
                </Button>
              </p>
            ) : null}
            {(groupMembersQuery.data?.items ?? []).length === 0 && !groupMembersQuery.isPending ? (
              <p className="text-ui text-muted-foreground">{t("group.emptyMembers")}</p>
            ) : (
              <ul className="flex flex-col divide-y text-ui">
                {(groupMembersQuery.data?.items ?? []).map((row) => {
                  const member = memberItems.find((item: MemberOutput) => item.userId === row.userId);
                  const label = member
                    ? `${formatPersonName(member)} (${member.email})`
                    : t("group.memberUnavailable");
                  return (
                    <li key={row.userId} className="flex items-center justify-between gap-2 py-2">
                      <span>{label}</span>
                      {canManage ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={pending}
                          onClick={() =>
                            void run(async () => {
                              setActionError(null);
                              await ensureOk(
                                await api.DELETE(
                                  "/api/v1/workspaces/{workspace_id}/groups/{group_id}/members",
                                  {
                                    params: {
                                      path: { workspace_id: workspaceId, group_id: selected.id },
                                    },
                                    body: { userId: row.userId },
                                  },
                                ),
                              );
                              await refreshMembers();
                            }).catch((err) => setActionError(failMessage(err)))
                          }
                        >
                          {t("group.removeMember")}
                        </Button>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            )}
            {canManage ? (
              candidatesError ? (
                <p role="alert" className="settings-notice settings-notice--danger">
                  {candidatesError}
                  <Button type="button" variant="outline" size="sm" onClick={() => void members.refetch()}>
                    {t("load.retry")}
                  </Button>
                </p>
              ) : (
                <form
                  className="settings-form__row"
                  onSubmit={(event) => {
                    event.preventDefault();
                    const form = event.currentTarget;
                    const value = new FormData(form).get("userId");
                    if (typeof value !== "string" || value === "") return;
                    void run(async () => {
                      setActionError(null);
                      await ensureOk(
                        await api.POST("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members", {
                          params: { path: { workspace_id: workspaceId, group_id: selected.id } },
                          body: { userId: value },
                        }),
                      );
                      form.reset();
                      await refreshMembers();
                    }).catch((err) => setActionError(failMessage(err)));
                  }}
                >
                  <div>
                    <Label htmlFor="group-member">{t("group.members")}</Label>
                    <select
                      id="group-member"
                      name="userId"
                      className="h-11 min-w-56 rounded-md border border-input bg-background px-3 text-ui"
                      disabled={pending || candidates.length === 0}
                      defaultValue=""
                    >
                      <option value="">{t("group.members")}</option>
                      {candidates.map((member) => (
                        <option key={member.userId} value={member.userId}>
                          {formatPersonName(member)} ({member.email})
                        </option>
                      ))}
                    </select>
                  </div>
                  <Button type="submit" size="sm" disabled={pending || candidates.length === 0}>
                    {t("group.addMember")}
                  </Button>
                </form>
              )
            ) : null}
            {canManage ? (
              confirmDelete ? (
                <div className="flex flex-col gap-2">
                  <p className="text-ui">{t("group.delete.confirm.body", { name: selected.name })}</p>
                  <div className="flex gap-2">
                    <Button type="button" variant="outline" size="sm" onClick={() => setConfirmDelete(false)}>
                      {t("common.dismiss")}
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      disabled={pending}
                      onClick={() =>
                        void run(async () => {
                          setActionError(null);
                          await ensureOk(
                            await api.DELETE("/api/v1/workspaces/{workspace_id}/groups/{group_id}", {
                              params: { path: { workspace_id: workspaceId, group_id: selected.id } },
                            }),
                          );
                          setSelectedGroupId(null);
                          setConfirmDelete(false);
                          await refreshGroups();
                        }).catch((err) => setActionError(failMessage(err)))
                      }
                    >
                      {t("group.delete")}
                    </Button>
                  </div>
                </div>
              ) : (
                <Button type="button" variant="outline" size="sm" onClick={() => setConfirmDelete(true)}>
                  {t("group.delete")}
                </Button>
              )
            ) : null}
          </section>
        ) : null}
        {actionError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {actionError}
          </p>
        ) : null}
      </div>
    </details>
  );
}
