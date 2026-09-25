import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { ApiTokenCreateInput, ApiTokenOutput } from "@/lib/contracts";
import { formFieldMessage } from "@/lib/form-issues";
import { workspaceApiTokensQuery } from "@/lib/queries";
import { apiTokenCreateInput, apiTokenScope } from "@/lib/validators";
import type { z } from "zod";
import "./settings-shell.css";

type CreateValues = z.infer<typeof apiTokenCreateInput>;
type TokenScope = z.infer<typeof apiTokenScope>;

const SCOPE_LABEL: Record<TokenScope, Parameters<typeof t>[0]> = {
  "documents.read": "token.scope.documents.read",
  "documents.write": "token.scope.documents.write",
  "tasks.read": "token.scope.tasks.read",
  "tasks.write": "token.scope.tasks.write",
  "projects.read": "token.scope.projects.read",
  "projects.manage": "token.scope.projects.manage",
  "share.manage": "token.scope.share.manage",
  "workspace.manage": "token.scope.workspace.manage",
};

function scopeDomId(scope: TokenScope): string {
  return scope.replace(".", "-");
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

function toggleItem<T>(items: T[], item: T, include: boolean): T[] {
  if (include) {
    return items.includes(item) ? items : [...items, item];
  }
  return items.filter((value) => value !== item);
}

function formatExpiry(expiresAt: string | null): string {
  if (expiresAt === null) {
    return t("token.unlimited");
  }
  return new Date(expiresAt).toLocaleDateString("ko-KR", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

function RevealedToken({ token }: { token: string }) {
  const [copyStatus, setCopyStatus] = useState<"copied" | "failed" | null>(null);
  return (
    <div className="flex flex-col gap-2 rounded-md border border-border p-3">
      <p role="status" className="text-ui">
        {t("token.once")}
      </p>
      <div className="flex flex-wrap items-end gap-2">
        <Input readOnly value={token} className="font-mono" aria-label={t("token.once")} />
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => {
            void copyText(token).then(
              () => setCopyStatus("copied"),
              () => setCopyStatus("failed"),
            );
          }}
        >
          {copyStatus === "copied" ? t("token.copied") : t("token.copy")}
        </Button>
      </div>
      {copyStatus === "failed" ? (
        <p role="alert" className="settings-notice settings-notice--danger">
          {t("workspace.invite.copyLink.failed")}
        </p>
      ) : null}
    </div>
  );
}

export function WorkspaceTokensSection({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient();
  const formId = useId();
  const tokens = useQuery(workspaceApiTokensQuery(workspaceId));
  const [revealed, setRevealed] = useState<{ id: string; token: string } | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<ApiTokenOutput | null>(null);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const confirmRevokeRef = useRef<HTMLButtonElement>(null);
  const cancelRevokeRef = useRef<HTMLButtonElement>(null);

  const form = useForm<CreateValues>({
    resolver: zodResolver(apiTokenCreateInput),
    defaultValues: { name: "", scopes: [], unlimited: false, service: false },
  });
  const scopes = form.watch("scopes");
  const unlimited = form.watch("unlimited") === true;
  const service = form.watch("service") === true;
  const nameError = formFieldMessage(form.formState.errors.name, "name");
  const scopesError = form.formState.errors.scopes ? t("token.scopes.required") : null;

  useEffect(() => {
    if (!revokeTarget) {
      return;
    }
    setRevokeError(null);
    confirmRevokeRef.current?.focus();
  }, [revokeTarget]);

  const create = useMutation({
    mutationFn: async (input: ApiTokenCreateInput) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/api-tokens", {
          params: { path: { workspace_id: workspaceId } },
          body: input,
        }),
      ),
    onSuccess: async (created) => {
      setRevealed({ id: created.id, token: created.token });
      await queryClient.invalidateQueries({ queryKey: ["workspaces", workspaceId, "api-tokens"] });
    },
  });

  const revoke = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/api-tokens/{id}", {
          params: { path: { workspace_id: workspaceId, id } },
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["workspaces", workspaceId, "api-tokens"] });
    },
  });

  const items = tokens.data?.items ?? [];
  const listError =
    tokens.error instanceof ProblemError ? tokens.error.title : tokens.error ? t("error.network") : null;

  return (
    <details className="settings-disclosure">
      <summary className="settings-disclosure__summary">{t("settings.tokens")}</summary>
      <div className="settings-disclosure__body">
        <form
          className="flex flex-col gap-2"
          noValidate
          onSubmit={form.handleSubmit(async (values) => {
            setFormError(null);
            try {
              await create.mutateAsync(values);
              form.reset({ name: "", scopes: [], unlimited: false, service: false });
            } catch (err) {
              setFormError(err instanceof ProblemError ? err.title : t("error.network"));
            }
          })}
        >
          <p className="text-ui font-medium">{t("token.add")}</p>
          <div className="flex flex-wrap items-end gap-2">
            <div className="flex min-w-40 flex-1 flex-col gap-1.5">
              <Label htmlFor={`${formId}-name`}>{t("token.name")}</Label>
              <Input
                id={`${formId}-name`}
                disabled={create.isPending}
                aria-invalid={nameError ? true : undefined}
                aria-describedby={nameError ? `${formId}-name-error` : undefined}
                {...form.register("name")}
              />
              {nameError ? (
                <p id={`${formId}-name-error`} className="text-destructive" role="alert">
                  {nameError}
                </p>
              ) : null}
            </div>
            <Button type="submit" size="sm" disabled={create.isPending}>
              {t("token.create")}
            </Button>
          </div>
          <fieldset className="flex flex-col gap-1.5">
            <legend className="text-ui font-medium">{t("token.scopes")}</legend>
            {apiTokenScope.options.map((scope) => (
              <div key={scope} className="flex min-h-11 items-center gap-2">
                <input
                  id={`${formId}-${scopeDomId(scope)}`}
                  type="checkbox"
                  className="size-4"
                  checked={scopes.includes(scope)}
                  disabled={create.isPending}
                  onChange={(event) => {
                    form.setValue("scopes", toggleItem(scopes, scope, event.target.checked), {
                      shouldValidate: true,
                    });
                  }}
                />
                <Label htmlFor={`${formId}-${scopeDomId(scope)}`}>{t(SCOPE_LABEL[scope])}</Label>
              </div>
            ))}
            {scopesError ? (
              <p className="text-destructive" role="alert">
                {scopesError}
              </p>
            ) : null}
          </fieldset>
          <div className="flex flex-wrap gap-4">
            <div className="flex min-h-11 items-center gap-2">
              <input
                id={`${formId}-unlimited`}
                type="checkbox"
                className="size-4"
                checked={unlimited}
                disabled={create.isPending}
                onChange={(event) => {
                  form.setValue("unlimited", event.target.checked, { shouldDirty: true });
                }}
              />
              <Label htmlFor={`${formId}-unlimited`}>{t("token.unlimited")}</Label>
            </div>
            <div className="flex min-h-11 items-center gap-2">
              <input
                id={`${formId}-service`}
                type="checkbox"
                className="size-4"
                checked={service}
                disabled={create.isPending}
                onChange={(event) => {
                  form.setValue("service", event.target.checked, { shouldDirty: true });
                }}
              />
              <Label htmlFor={`${formId}-service`}>{t("token.service")}</Label>
            </div>
          </div>
          {formError ? (
            <p role="alert" className="settings-notice settings-notice--danger">
              {formError}
            </p>
          ) : null}
        </form>
        {revealed ? <RevealedToken key={revealed.id} token={revealed.token} /> : null}
        {tokens.isPending ? <p role="status">{t("load.loading")}</p> : null}
        {listError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {listError}
          </p>
        ) : null}
        {!tokens.isPending && !listError && items.length === 0 ? (
          <p className="text-ui text-muted-foreground">{t("token.empty")}</p>
        ) : null}
        {items.length > 0 ? (
          <ul className="flex flex-col divide-y text-ui">
            {items.map((row) => (
              <li
                key={row.id}
                className="flex min-w-0 flex-col gap-2 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between"
              >
                <div className="min-w-0">
                  <p className="font-medium break-keep">
                    {row.name}
                    {row.userId === null ? (
                      <span className="ml-2 text-muted-foreground">{t("token.service")}</span>
                    ) : null}
                  </p>
                  <p className="text-muted-foreground break-keep">
                    {row.scopes.map((scope) => t(SCOPE_LABEL[scope as TokenScope] ?? "token.scopes")).join(", ")}
                  </p>
                  <p className="text-muted-foreground">
                    {t("token.expires")}: {formatExpiry(row.expiresAt ?? null)}
                  </p>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={revoke.isPending}
                  onClick={() => setRevokeTarget(row)}
                >
                  {t("token.revoke")}
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      {revokeTarget ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="token-revoke-title"
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/40 p-4"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              setRevokeTarget(null);
              setRevokeError(null);
            }
          }}
        >
          <div className="max-w-md rounded-md border border-border bg-background p-4">
            <h2 id="token-revoke-title" className="text-title">
              {t("token.revoke.confirm.title")}
            </h2>
            <p className="mt-2 text-ui text-muted-foreground">
              {t("token.revoke.confirm.body", { name: revokeTarget.name })}
            </p>
            {revokeError ? (
              <p role="alert" className="settings-notice settings-notice--danger mt-2">
                {revokeError}
              </p>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              <Button
                ref={cancelRevokeRef}
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  setRevokeTarget(null);
                  setRevokeError(null);
                }}
              >
                {t("common.dismiss")}
              </Button>
              <Button
                ref={confirmRevokeRef}
                type="button"
                size="sm"
                onClick={() => {
                  const target = revokeTarget;
                  void revoke.mutateAsync(target.id).then(
                    () => {
                      if (revealed?.id === target.id) {
                        setRevealed(null);
                      }
                      setRevokeTarget(null);
                    },
                    (err: unknown) => {
                      setRevokeError(err instanceof ProblemError ? err.title : t("error.network"));
                    },
                  );
                }}
              >
                {t("token.revoke")}
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </details>
  );
}
