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
import type { WebhookCreatedOutput, WebhookCreateInput, WebhookOutput } from "@/lib/contracts";
import { formFieldMessage } from "@/lib/form-issues";
import { workspaceWebhooksQuery } from "@/lib/queries";
import { webhookCreateInput } from "@/lib/validators";
import { ConfirmDialog } from "./confirm-dialog";
import { WEBHOOK_EVENTS, webhookCreateProblemKey, webhookEventLabel } from "./webhook-events";
import "./settings-shell.css";

type CreateValues = z.infer<typeof webhookCreateInput>;

/** Keeps the problem `source` that `ensureOk` drops: `/url` tells a refused target apart. */
class WebhookCreateError extends Error {}

function failMessage(err: unknown): string {
  if (err instanceof WebhookCreateError) return err.message;
  return err instanceof ProblemError ? err.title : t("error.network");
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

/** The signing secret lives only in this component's state; it is never cached or persisted. */
function RevealedSecret({ secret }: { secret: string }) {
  const [copyStatus, setCopyStatus] = useState<"copied" | "failed" | null>(null);
  return (
    <div className="flex flex-col gap-2 rounded-md border border-border p-3">
      <p role="status" className="text-ui">
        {t("webhook.secret.once")}
      </p>
      <div className="flex flex-wrap items-end gap-2">
        <Input
          readOnly
          value={secret}
          className="font-mono"
          aria-label={t("webhook.secret.label")}
          autoComplete="off"
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => {
            void copyText(secret).then(
              () => setCopyStatus("copied"),
              () => setCopyStatus("failed"),
            );
          }}
        >
          {copyStatus === "copied" ? t("webhook.copied") : t("webhook.copy")}
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

export function WorkspaceWebhooksSection({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient();
  const formId = useId();
  const webhooks = useQuery(workspaceWebhooksQuery(workspaceId));
  const [revealed, setRevealed] = useState<{ id: string; secret: string } | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<WebhookOutput | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const form = useForm<CreateValues>({
    resolver: zodResolver(webhookCreateInput),
    defaultValues: { url: "", events: [] },
  });
  const events = form.watch("events");
  const urlError = formFieldMessage(form.formState.errors.url, "url");
  const eventsError = form.formState.errors.events ? t("webhook.events.required") : null;
  const queryKey = workspaceWebhooksQuery(workspaceId).queryKey;

  const create = useMutation({
    mutationFn: async (input: WebhookCreateInput): Promise<WebhookCreatedOutput> => {
      const result = await api.POST("/api/v1/workspaces/{workspace_id}/webhooks", {
        params: { path: { workspace_id: workspaceId } },
        body: input,
      });
      if (result.error) {
        const key = webhookCreateProblemKey(result.error.code, result.error.source);
        if (key) throw new WebhookCreateError(t(key));
      }
      return ensureOk(result);
    },
    onSuccess: async (created) => {
      setRevealed({ id: created.id, secret: created.secret });
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  const remove = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", {
          params: { path: { workspace_id: workspaceId, webhook_id: id } },
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  const items = webhooks.data?.items ?? [];
  const listError = webhooks.error ? failMessage(webhooks.error) : null;

  return (
    <details className="settings-disclosure">
      <summary className="settings-disclosure__summary">{t("settings.webhooks")}</summary>
      <div className="settings-disclosure__body">
        <form
          className="flex flex-col gap-2"
          noValidate
          onSubmit={form.handleSubmit(async (values) => {
            setFormError(null);
            try {
              await create.mutateAsync({ url: values.url, events: values.events });
              form.reset({ url: "", events: [] });
            } catch (err) {
              setFormError(failMessage(err));
            }
          })}
        >
          <p className="text-ui font-medium">{t("webhook.add")}</p>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${formId}-url`}>{t("webhook.url")}</Label>
            <Input
              id={`${formId}-url`}
              type="url"
              inputMode="url"
              autoComplete="off"
              maxLength={2048}
              disabled={create.isPending}
              aria-invalid={urlError ? true : undefined}
              aria-describedby={urlError ? `${formId}-url-error` : undefined}
              {...form.register("url")}
            />
            {urlError ? (
              <p id={`${formId}-url-error`} className="text-destructive" role="alert">
                {urlError}
              </p>
            ) : null}
          </div>
          <fieldset className="flex flex-col gap-1.5">
            <legend className="text-ui font-medium">{t("webhook.events")}</legend>
            <div className="grid gap-x-4 sm:grid-cols-2">
              {WEBHOOK_EVENTS.map((verb) => (
                <div key={verb} className="flex min-h-11 items-center gap-2">
                  <input
                    id={`${formId}-${verb.replace(".", "-")}`}
                    type="checkbox"
                    className="size-4"
                    checked={events.includes(verb)}
                    disabled={create.isPending}
                    onChange={(event) => {
                      form.setValue("events", toggleItem(events, verb, event.target.checked), {
                        shouldValidate: true,
                      });
                    }}
                  />
                  <Label htmlFor={`${formId}-${verb.replace(".", "-")}`}>{webhookEventLabel(verb)}</Label>
                </div>
              ))}
            </div>
            {eventsError ? (
              <p className="text-destructive" role="alert">
                {eventsError}
              </p>
            ) : null}
          </fieldset>
          <Button type="submit" size="sm" className="w-fit" disabled={create.isPending}>
            {t("webhook.create")}
          </Button>
          {formError ? (
            <p role="alert" className="settings-notice settings-notice--danger">
              {formError}
            </p>
          ) : null}
        </form>
        {revealed ? <RevealedSecret key={revealed.id} secret={revealed.secret} /> : null}
        {webhooks.isPending ? <p role="status">{t("load.loading")}</p> : null}
        {listError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {listError}
          </p>
        ) : null}
        {!webhooks.isPending && !listError && items.length === 0 ? (
          <p className="text-ui text-muted-foreground">{t("webhook.empty")}</p>
        ) : null}
        {items.length > 0 ? (
          <ul className="flex flex-col divide-y text-ui" aria-label={t("settings.webhooks")}>
            {items.map((row) => (
              <li
                key={row.id}
                className="flex min-w-0 flex-col gap-2 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between"
              >
                <div className="min-w-0">
                  <p className="font-mono break-all">{row.url}</p>
                  <p className="text-muted-foreground break-keep">
                    {row.events.map(webhookEventLabel).join(", ")}
                  </p>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={remove.isPending}
                  aria-label={`${t("webhook.delete")} ${row.url}`}
                  onClick={() => {
                    setDeleteError(null);
                    setDeleteTarget(row);
                  }}
                >
                  {t("webhook.delete")}
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      {deleteTarget ? (
        <ConfirmDialog
          title={t("webhook.delete.confirm.title")}
          body={t("webhook.delete.confirm.body", { url: deleteTarget.url })}
          actionLabel={t("webhook.delete")}
          pending={remove.isPending}
          error={deleteError}
          onCancel={() => {
            setDeleteTarget(null);
            setDeleteError(null);
          }}
          onConfirm={() => {
            const target = deleteTarget;
            void remove.mutateAsync(target.id).then(
              () => {
                if (revealed?.id === target.id) {
                  setRevealed(null);
                }
                setDeleteTarget(null);
              },
              (err: unknown) => {
                setDeleteError(failMessage(err));
              },
            );
          }}
        />
      ) : null}
    </details>
  );
}
