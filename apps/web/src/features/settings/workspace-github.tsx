import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useForm } from "react-hook-form";
import type { z } from "zod";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { GithubIssueLinkInput } from "@/lib/contracts";
import { formFieldMessage } from "@/lib/form-issues";
import { workspaceGithubQuery } from "@/lib/queries";
import { githubIssueLinkForm } from "@/lib/validators";
import { ConfirmDialog } from "./confirm-dialog";
import "./settings-shell.css";

type LinkValues = z.infer<typeof githubIssueLinkForm>;

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

export function WorkspaceGithubSection({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient();
  const formId = useId();
  const install = useQuery(workspaceGithubQuery(workspaceId));
  const [actionError, setActionError] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [uninstallError, setUninstallError] = useState<string | null>(null);
  const [linkStatus, setLinkStatus] = useState<string | null>(null);
  const [linkError, setLinkError] = useState<string | null>(null);
  const queryKey = workspaceGithubQuery(workspaceId).queryKey;

  const form = useForm<LinkValues>({
    resolver: zodResolver(githubIssueLinkForm),
    defaultValues: { taskId: "", repo: "", issueNumber: "" },
  });
  const fieldErrors = [
    formFieldMessage(form.formState.errors.taskId, "taskId"),
    formFieldMessage(form.formState.errors.repo, "repo"),
    formFieldMessage(form.formState.errors.issueNumber, "issueNumber"),
  ];
  const fieldError = fieldErrors.find((message) => message !== null) ?? null;

  const startInstall = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/github/install", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    onSuccess: ({ url }) => {
      window.location.assign(url);
    },
    onError: (err: unknown) => {
      // Source: without a configured GitHub App the install is invalid input (400).
      setActionError(
        err instanceof ProblemError && err.status === 400 ? t("github.notConfigured") : failMessage(err),
      );
    },
  });

  const uninstall = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/github", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  const link = useMutation({
    mutationFn: async (input: GithubIssueLinkInput) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/github/issue-links", {
          params: { path: { workspace_id: workspaceId } },
          body: input,
        }),
      ),
  });

  const installationId = install.data?.installationId ?? null;
  const connected = installationId !== null;
  const loadError = install.error ? failMessage(install.error) : null;
  const busy = install.isPending || startInstall.isPending || uninstall.isPending;

  return (
    <details className="settings-disclosure">
      <summary className="settings-disclosure__summary">{t("settings.github")}</summary>
      <div className="settings-disclosure__body">
        {install.isPending ? <p role="status">{t("load.loading")}</p> : null}
        {loadError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {loadError}
          </p>
        ) : null}
        {!install.isPending && !loadError ? (
          <p className="text-ui text-muted-foreground">
            {connected ? t("github.connected") : t("github.disconnected")}
            {connected ? ` (${installationId})` : ""}
          </p>
        ) : null}
        {connected ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-fit"
            disabled={busy || Boolean(loadError)}
            onClick={() => {
              setUninstallError(null);
              setConfirmOpen(true);
            }}
          >
            {t("github.uninstall")}
          </Button>
        ) : (
          <Button
            type="button"
            size="sm"
            className="w-fit"
            disabled={busy || Boolean(loadError)}
            onClick={() => {
              setActionError(null);
              startInstall.mutate();
            }}
          >
            {t("github.install")}
          </Button>
        )}
        {actionError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {actionError}
          </p>
        ) : null}
        <form
          className="flex flex-col gap-2"
          noValidate
          onSubmit={form.handleSubmit(async (values) => {
            setLinkError(null);
            setLinkStatus(null);
            try {
              await link.mutateAsync({
                taskId: values.taskId,
                repo: values.repo,
                issueNumber: Number(values.issueNumber),
              });
              form.reset({ taskId: "", repo: "", issueNumber: "" });
              setLinkStatus(t("github.issue.linked"));
            } catch (err) {
              setLinkError(failMessage(err));
            }
          })}
        >
          <p className="text-ui font-medium">{t("github.issue.link")}</p>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${formId}-task`}>{t("github.issue.task")}</Label>
            <Input
              id={`${formId}-task`}
              autoComplete="off"
              disabled={link.isPending}
              aria-invalid={fieldErrors[0] ? true : undefined}
              {...form.register("taskId")}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${formId}-repo`}>{t("github.issue.repo")}</Label>
            <Input
              id={`${formId}-repo`}
              autoComplete="off"
              disabled={link.isPending}
              aria-invalid={fieldErrors[1] ? true : undefined}
              {...form.register("repo")}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${formId}-number`}>{t("github.issue.number")}</Label>
            <Input
              id={`${formId}-number`}
              type="number"
              inputMode="numeric"
              min={1}
              disabled={link.isPending}
              aria-invalid={fieldErrors[2] ? true : undefined}
              {...form.register("issueNumber")}
            />
          </div>
          {fieldError ? (
            <p className="text-destructive" role="alert">
              {fieldError}
            </p>
          ) : null}
          <Button type="submit" size="sm" className="w-fit" disabled={link.isPending}>
            {t("github.issue.link")}
          </Button>
          {linkStatus ? (
            <p role="status" className="settings-notice">
              {linkStatus}
            </p>
          ) : null}
          {linkError ? (
            <p role="alert" className="settings-notice settings-notice--danger">
              {linkError}
            </p>
          ) : null}
        </form>
      </div>
      {confirmOpen ? (
        <ConfirmDialog
          title={t("github.uninstall.confirm.title")}
          body={t("github.uninstall.confirm.body")}
          actionLabel={t("github.uninstall")}
          pending={uninstall.isPending}
          error={uninstallError}
          onCancel={() => setConfirmOpen(false)}
          onConfirm={() => {
            void uninstall.mutateAsync().then(
              () => setConfirmOpen(false),
              (err: unknown) => setUninstallError(failMessage(err)),
            );
          }}
        />
      ) : null}
    </details>
  );
}
