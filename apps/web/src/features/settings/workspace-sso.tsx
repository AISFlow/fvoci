// Adapted from fvoci/FVOCI apps/web/src/features/settings/settings-sso.tsx and
// routes/w.$slug.settings.sso.tsx. The source keeps SSO on its own settings
// tab; this app keeps workspace settings as sections of one page, so the view
// is a collapsed disclosure (like API tokens) shown to owners/admins only.
// A 404 from the configuration route is the enterprise-license gate.
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { ConfirmActionButton } from "@/components/confirm-action";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { components } from "@/generated/api";
import { formFieldMessage } from "@/lib/form-issues";
import "./settings-shell.css";

type WorkspaceOidcGetOutput = components["schemas"]["WorkspaceOidcGetOutput"];
type WorkspaceOidcInput = components["schemas"]["WorkspaceOidcBody"];

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:";
  } catch {
    return false;
  }
}

// Source `workspaceOidcInput`.
export const workspaceOidcForm = z.object({
  issuer: z
    .string()
    .trim()
    .min(1, "i18n:form.too_small")
    .max(2048, "i18n:form.too_big")
    .refine(isHttpUrl, { message: "i18n:form.invalid" }),
  clientId: z.string().trim().min(1, "i18n:form.too_small").max(256, "i18n:form.too_big"),
  clientSecret: z.string().min(1, "i18n:form.too_small").max(4096, "i18n:form.too_big"),
  label: z.string().trim().max(100, "i18n:form.too_big"),
});

type SsoFormValues = z.infer<typeof workspaceOidcForm>;

function isConfigured(row: WorkspaceOidcGetOutput | null): boolean {
  return row !== null && row.issuer !== null && row.clientId !== null && row.label !== null;
}

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

function workspaceOidcQueryKey(workspaceId: string) {
  return ["workspaces", workspaceId, "oidc"] as const;
}

function SsoForm({
  current,
  pending,
  onSave,
  onRemove,
}: {
  current: WorkspaceOidcGetOutput | null;
  pending: boolean;
  onSave: (input: WorkspaceOidcInput) => Promise<void>;
  onRemove: () => Promise<void>;
}) {
  const id = useId();
  const configured = isConfigured(current);
  const form = useForm<SsoFormValues>({
    resolver: zodResolver(workspaceOidcForm),
    defaultValues: {
      issuer: current?.issuer ?? "",
      clientId: current?.clientId ?? "",
      // The secret is never returned: every save re-enters it.
      clientSecret: "",
      label: current?.label ?? "",
    },
  });
  const errors = form.formState.errors;
  const fields: {
    name: keyof SsoFormValues;
    label: string;
    type?: string;
  }[] = [
    { name: "issuer", label: t("auth.sso.issuer"), type: "url" },
    { name: "clientId", label: t("auth.sso.clientId") },
    { name: "clientSecret", label: t("auth.sso.clientSecret"), type: "password" },
    { name: "label", label: t("auth.sso.label") },
  ];

  return (
    <form
      className="flex flex-col gap-2"
      noValidate
      onSubmit={form.handleSubmit(async (values) => {
        try {
          await onSave({
            issuer: values.issuer,
            clientId: values.clientId,
            clientSecret: values.clientSecret,
            label: values.label === "" ? null : values.label,
          });
          form.setValue("clientSecret", "");
        } catch {
          // The section shows the mutation error.
        }
      })}
    >
      {fields.map((field) => {
        const message = formFieldMessage(errors[field.name], field.name);
        const inputId = `${id}-${field.name}`;
        return (
          <div key={field.name} className="flex flex-col gap-1.5">
            <Label htmlFor={inputId}>{field.label}</Label>
            <Input
              id={inputId}
              type={field.type}
              disabled={pending}
              autoComplete="off"
              aria-invalid={message ? true : undefined}
              aria-describedby={message ? `${inputId}-error` : undefined}
              {...form.register(field.name)}
            />
            {message ? (
              <p id={`${inputId}-error`} className="text-ui text-destructive" role="alert">
                {message}
              </p>
            ) : null}
          </div>
        );
      })}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={pending}>
          {t("auth.sso.save")}
        </Button>
        {configured ? (
          <ConfirmActionButton
            title={t("auth.sso.remove.confirm.title")}
            description={t("auth.sso.remove.confirm.body")}
            actionLabel={t("auth.sso.remove")}
            disabled={pending}
            onConfirm={onRemove}
          >
            {t("auth.sso.remove")}
          </ConfirmActionButton>
        ) : null}
      </div>
    </form>
  );
}

export function WorkspaceSsoSection({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient();
  const [actionError, setActionError] = useState<string | null>(null);
  const queryKey = workspaceOidcQueryKey(workspaceId);

  const oidc = useQuery({
    queryKey,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/oidc", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
  });

  const save = useMutation({
    mutationFn: async (input: WorkspaceOidcInput) =>
      ensureOk(
        await api.PUT("/api/v1/workspaces/{workspace_id}/oidc", {
          params: { path: { workspace_id: workspaceId } },
          body: input,
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (err) => setActionError(failMessage(err)),
  });

  const remove = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/oidc", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (err) => setActionError(failMessage(err)),
  });

  const pending = save.isPending || remove.isPending;
  const current = oidc.data ?? null;
  const eeRequired = oidc.error instanceof ProblemError && oidc.error.status === 404;
  const error = actionError ?? (oidc.error && !eeRequired ? failMessage(oidc.error) : null);

  return (
    <details className="settings-disclosure">
      <summary className="settings-disclosure__summary">{t("auth.sso.title")}</summary>
      <div className="settings-disclosure__body flex flex-col gap-4">
        {!oidc.isLoading && !oidc.isError ? (
          <SsoForm
            key={current?.issuer ?? "empty"}
            current={current}
            pending={pending}
            onSave={async (input) => {
              await save.mutateAsync(input);
            }}
            onRemove={async () => {
              await remove.mutateAsync().catch(() => undefined);
            }}
          />
        ) : null}
        {eeRequired ? (
          <p className="text-ui text-muted-foreground">{t("ee.required")}</p>
        ) : null}
        {error ? (
          <p className="text-ui text-destructive" role="alert">
            {error}
          </p>
        ) : null}
        {oidc.isLoading ? (
          <p role="status" className="text-ui text-muted-foreground">
            {t("load.loading")}
          </p>
        ) : null}
      </div>
    </details>
  );
}
