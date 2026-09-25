import { t, formatPersonName } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import { membersQuery } from "@/lib/queries";
import type { MemberOutput, WorkspaceRole } from "@/lib/contracts";
import { invitationCreateInput } from "@/lib/validators";
import type { z } from "zod";

type InviteFormValues = z.infer<typeof invitationCreateInput>;

const ROLES: WorkspaceRole[] = ["owner", "admin", "member", "guest"];
const ROLE_ORDER: WorkspaceRole[] = ["guest", "member", "admin", "owner"];

function roleRank(role: string): number {
  return ROLE_ORDER.indexOf(role as WorkspaceRole);
}

function roleAtLeast(role: string, minimum: string): boolean {
  return roleRank(role) >= roleRank(minimum);
}

function roleLabel(role: string): string {
  if (role === "owner") return t("role.owner");
  if (role === "admin") return t("role.admin");
  if (role === "guest") return t("role.guest");
  return t("role.member");
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

function InviteResult({
  acceptUrl,
  mailDelayed,
}: {
  acceptUrl: string;
  mailDelayed?: boolean | null;
}) {
  const [copyStatus, setCopyStatus] = useState<"copied" | "failed" | null>(null);
  return (
    <div className="flex flex-col gap-2">
      <p role="status" className="text-ui">
        {t("workspace.invite.created")}
        {mailDelayed ? ` ${t("workspace.invite.mailDelayed")}` : ""}
      </p>
      <p className="text-ui wrap-anywhere">
        <a href={acceptUrl} className="underline underline-offset-2">
          {acceptUrl}
        </a>
      </p>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="w-fit"
        onClick={() => {
          void copyText(acceptUrl).then(
            () => setCopyStatus("copied"),
            () => setCopyStatus("failed"),
          );
        }}
      >
        {t("workspace.invite.copyLink")}
      </Button>
      {copyStatus ? (
        <p role={copyStatus === "failed" ? "alert" : "status"} className="text-ui">
          {t(copyStatus === "copied" ? "workspace.invite.copyLink.done" : "workspace.invite.copyLink.failed")}
        </p>
      ) : null}
    </div>
  );
}

function InviteForm({
  pending,
  roles,
  onInvite,
}: {
  pending: boolean;
  roles: readonly WorkspaceRole[];
  onInvite: (input: InviteFormValues) => Promise<{ acceptUrl: string; mailDelayed?: boolean | null }>;
}) {
  const [result, setResult] = useState<{
    acceptUrl: string;
    mailDelayed?: boolean | null;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const form = useForm<InviteFormValues>({
    resolver: zodResolver(invitationCreateInput),
    defaultValues: { email: "", role: "member" },
  });

  return (
    <form
      onSubmit={form.handleSubmit(async (values) => {
        setResult(null);
        setError(null);
        try {
          const created = await onInvite(values);
          setResult(created);
          form.reset({ email: "", role: values.role });
        } catch (err) {
          setError(err instanceof ProblemError ? err.title : t("error.network"));
        }
      })}
      noValidate
      className="flex flex-col gap-1.5"
    >
      <div className="flex flex-wrap items-end gap-2">
        <div className="flex min-w-40 flex-1 flex-col gap-1.5">
          <Label htmlFor="workspace-invite-email">{t("workspace.invite.email")}</Label>
          <Input
            id="workspace-invite-email"
            type="email"
            autoComplete="email"
            disabled={pending || form.formState.isSubmitting}
            aria-invalid={form.formState.errors.email ? true : undefined}
            {...form.register("email")}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="workspace-invite-role">{t("group.role")}</Label>
          <select
            id="workspace-invite-role"
            className="h-11 min-w-28 rounded-md border border-input bg-background px-3 text-ui"
            disabled={pending || form.formState.isSubmitting}
            {...form.register("role")}
          >
            {roles.map((role) => (
              <option key={role} value={role}>
                {roleLabel(role)}
              </option>
            ))}
          </select>
        </div>
        <Button type="submit" size="sm" disabled={pending || form.formState.isSubmitting}>
          {t("auth.invite.send")}
        </Button>
      </div>
      {form.formState.errors.email ? (
        <p role="alert" className="settings-notice settings-notice--danger">
          {formFieldMessage(form.formState.errors.email, "email") ?? t("form.email")}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="settings-notice settings-notice--danger">
          {error}
        </p>
      ) : null}
      {result ? (
        <InviteResult
          key={result.acceptUrl}
          acceptUrl={result.acceptUrl}
          mailDelayed={result.mailDelayed}
        />
      ) : null}
    </form>
  );
}

export function WorkspaceMembersSection({
  workspaceId,
  currentUserId,
  currentUserRole,
}: {
  workspaceId: string;
  currentUserId: string | null;
  currentUserRole: string;
}) {
  const queryClient = useQueryClient();
  const headingRef = useRef<HTMLElement>(null);
  const confirmRemoveRef = useRef<HTMLButtonElement>(null);
  const cancelRemoveRef = useRef<HTMLButtonElement>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const members = useQuery(membersQuery(workspaceId));
  const canManage = roleAtLeast(currentUserRole, "admin");
  const inviteRoles = ROLES.filter((role) => roleAtLeast(currentUserRole, role));
  const [pendingIds, setPendingIds] = useState<ReadonlySet<string>>(new Set());
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  const [status, setStatus] = useState<string | null>(null);
  const [removeTarget, setRemoveTarget] = useState<MemberOutput | null>(null);

  useEffect(() => {
    if (!removeTarget) {
      return;
    }
    setRemoveError(null);
    confirmRemoveRef.current?.focus();
  }, [removeTarget]);

  const invite = useMutation({
    mutationFn: async (input: InviteFormValues) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/invitations", {
          params: { path: { workspace_id: workspaceId } },
          body: input,
        }),
      ),
  });
  const patchRole = useMutation({
    mutationFn: async (input: { userId: string; role: WorkspaceRole }) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/members/{user_id}", {
          params: { path: { workspace_id: workspaceId, user_id: input.userId } },
          body: { role: input.role },
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["workspaces", workspaceId, "members"] });
    },
  });
  const removeMember = useMutation({
    mutationFn: async (userId: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/members/{user_id}", {
          params: { path: { workspace_id: workspaceId, user_id: userId } },
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["workspaces", workspaceId, "members"] });
    },
  });

  async function run(userId: string, action: () => Promise<void>): Promise<string | null> {
    setPendingIds((current) => new Set(current).add(userId));
    setRowErrors((current) => {
      const next = { ...current };
      delete next[userId];
      return next;
    });
    setStatus(null);
    try {
      await action();
      return null;
    } catch (err) {
      const message =
        err instanceof ProblemError ? err.title : t("workspace.member.action.failed");
      setRowErrors((current) => ({
        ...current,
        [userId]: message,
      }));
      return message;
    } finally {
      setPendingIds((current) => {
        const next = new Set(current);
        next.delete(userId);
        return next;
      });
    }
  }

  const items = members.data?.items ?? [];
  const membersError =
    members.error instanceof ProblemError ? members.error.title : members.error ? t("error.network") : null;

  return (
    <details className="settings-disclosure">
      <summary ref={headingRef} tabIndex={-1} className="settings-disclosure__summary">
        {t("workspace.members")}
      </summary>
      <div className="settings-disclosure__body">
        {members.isPending ? <p role="status">{t("load.loading")}</p> : null}
        {items.length > 0 ? (
          <ul className="flex flex-col divide-y text-ui">
            {items.map((member) => {
              const name = formatPersonName({
                givenName: member.givenName,
                familyName: member.familyName,
              });
              const isSelf = member.userId === currentUserId;
              const canManageTarget =
                canManage && !isSelf && roleAtLeast(currentUserRole, member.role);
              const isPending = pendingIds.has(member.userId);
              return (
                <li
                  key={member.userId}
                  className="flex min-w-0 flex-col gap-2 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between"
                >
                  <div className="min-w-0">
                    <p className="font-medium text-ui break-keep">
                      {name}
                      {isSelf ? (
                        <span className="ml-2 text-muted-foreground">{t("workspace.member.self")}</span>
                      ) : null}
                    </p>
                    <p className="text-muted-foreground wrap-anywhere">{member.email}</p>
                    {rowErrors[member.userId] ? (
                      <p role="alert" className="mt-1 text-destructive break-keep">
                        {rowErrors[member.userId]}
                      </p>
                    ) : null}
                  </div>
                  <div className="flex min-w-0 flex-wrap items-center gap-2 sm:justify-end">
                    {canManageTarget ? (
                      <>
                        <label className="sr-only" htmlFor={`member-role-${member.userId}`}>
                          {t("workspace.member.roleLabel", { name })}
                        </label>
                        <select
                          id={`member-role-${member.userId}`}
                          aria-label={t("workspace.member.roleLabel", { name })}
                          className="h-11 min-w-28 rounded-md border border-input bg-background px-3 text-ui"
                          value={member.role}
                          disabled={isPending}
                          onChange={(event) => {
                            const next = event.target.value as WorkspaceRole;
                            if (next === member.role) return;
                            void run(member.userId, async () => {
                              await patchRole.mutateAsync({ userId: member.userId, role: next });
                              setStatus(t("workspace.member.roleChanged", { name }));
                            });
                          }}
                        >
                          {inviteRoles.map((role) => (
                            <option key={role} value={role}>
                              {roleLabel(role)}
                            </option>
                          ))}
                        </select>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={isPending}
                          onClick={() => setRemoveTarget(member)}
                        >
                          {t("workspace.member.remove.action")}
                        </Button>
                      </>
                    ) : (
                      <span className="text-muted-foreground">{roleLabel(member.role)}</span>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        ) : null}
        {status ? <p role="status">{status}</p> : null}
        {membersError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {membersError}
          </p>
        ) : null}
        {canManage ? (
          <InviteForm
            pending={invite.isPending}
            roles={inviteRoles}
            onInvite={async (input) => invite.mutateAsync(input)}
          />
        ) : null}
      </div>
      {removeTarget ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="member-remove-title"
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/40 p-4"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              setRemoveTarget(null);
              setRemoveError(null);
              return;
            }
            if (event.key !== "Tab") {
              return;
            }
            const first = cancelRemoveRef.current;
            const last = confirmRemoveRef.current;
            if (!first || !last) {
              return;
            }
            if (event.shiftKey && document.activeElement === first) {
              event.preventDefault();
              last.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first.focus();
            }
          }}
        >
          <div className="max-w-md rounded-md border border-border bg-background p-4">
            <h2 id="member-remove-title" className="text-title">
              {t("workspace.member.remove.title", {
                name: formatPersonName({
                  givenName: removeTarget.givenName,
                  familyName: removeTarget.familyName,
                }),
              })}
            </h2>
            <p className="mt-2 text-ui text-muted-foreground">
              {t("workspace.member.remove.description", {
                name: formatPersonName({
                  givenName: removeTarget.givenName,
                  familyName: removeTarget.familyName,
                }),
              })}
            </p>
            {removeError ? (
              <p role="alert" className="settings-notice settings-notice--danger mt-2">
                {removeError}
              </p>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              <Button
                ref={cancelRemoveRef}
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  setRemoveTarget(null);
                  setRemoveError(null);
                }}
              >
                {t("common.dismiss")}
              </Button>
              <Button
                ref={confirmRemoveRef}
                type="button"
                size="sm"
                onClick={() => {
                  const member = removeTarget;
                  const name = formatPersonName({
                    givenName: member.givenName,
                    familyName: member.familyName,
                  });
                  void run(member.userId, async () => {
                    await removeMember.mutateAsync(member.userId);
                    setStatus(t("workspace.member.removed", { name }));
                    setRemoveTarget(null);
                    setRemoveError(null);
                    requestAnimationFrame(() => {
                      headingRef.current?.focus();
                    });
                  }).then((error) => {
                    if (error) {
                      setRemoveError(error);
                    }
                  });
                }}
              >
                {t("workspace.member.remove.confirm")}
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </details>
  );
}
