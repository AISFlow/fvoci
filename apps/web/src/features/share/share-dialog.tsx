import { isI18nKey, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeModal } from "@/features/projects/native-modal";
import { api, ensureOk, problemMessage } from "@/lib/api";
import { documentShareLinksQuery, type ShareDocumentTarget } from "@/lib/queries/share";
import { SHARE_DEFAULT_EXPIRES_DAYS, SHARE_EXPIRES_OPTIONS } from "@/lib/share";
import "@/features/projects/projects.css";
import "./share.css";

const dateFormat = new Intl.DateTimeFormat("ko", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});

function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : dateFormat.format(date);
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

function RevealedShareUrl({ url }: { url: string }) {
  const [copied, setCopied] = useState(false);
  /* WHY: 공유 링크 원문은 생성 직후 한 번만 보여 준다. 목록은 id·만료만 두고 토큰을 다시 그리지 않는다. */
  return (
    <div className="share-dialog__secret">
      <Input aria-label={t("share.url")} readOnly value={url} className="font-mono" />
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => {
          void copyText(url).then(
            () => setCopied(true),
            () => setCopied(false),
          );
        }}
      >
        {copied ? t("share.copied") : t("share.copy")}
      </Button>
    </div>
  );
}

/** Source `ShareDialog` for one document; expiry choices mirror the source catalog defaults. */
export function ShareDialog({
  workspaceId,
  target,
}: {
  workspaceId: string;
  target: ShareDocumentTarget;
}) {
  const queryClient = useQueryClient();
  const titleId = useId();
  const expiresId = useId();
  const [open, setOpen] = useState(false);
  const [createdUrl, setCreatedUrl] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [expiresInDays, setExpiresInDays] = useState<number>(SHARE_DEFAULT_EXPIRES_DAYS);
  const linksQuery = documentShareLinksQuery(workspaceId, target);
  const list = useQuery({ ...linksQuery, enabled: open });

  async function invalidateLinks(): Promise<void> {
    await queryClient.invalidateQueries({ queryKey: linksQuery.queryKey });
  }

  const create = useMutation({
    mutationFn: async (days: number) =>
      ensureOk(
        target.projectId === null
          ? await api.POST("/api/v1/workspaces/{workspace_id}/documents/{id}/share-links", {
              params: { path: { workspace_id: workspaceId, id: target.documentId } },
              body: { expiresInDays: days },
            })
          : await api.POST(
              "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{id}/share-links",
              {
                params: {
                  path: {
                    workspace_id: workspaceId,
                    project_id: target.projectId,
                    id: target.documentId,
                  },
                },
                body: { expiresInDays: days },
              },
            ),
      ),
    onSuccess: async (created) => {
      setActionError(null);
      setCreatedUrl(created.url);
      await invalidateLinks();
    },
    onError: (err: unknown) => {
      setActionError(problemMessage(err, "error.share.failed"));
    },
  });

  const revoke = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/share-links/{id}", {
          params: { path: { workspace_id: workspaceId, id } },
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await invalidateLinks();
    },
    onError: (err: unknown) => {
      setActionError(problemMessage(err, "error.share.failed"));
    },
  });

  const pending = create.isPending || revoke.isPending;
  const links = list.data?.items ?? [];
  const error =
    actionError ?? (list.error ? problemMessage(list.error, "error.share.failed") : null);

  function close() {
    setOpen(false);
    setCreatedUrl(null);
    setActionError(null);
  }

  return (
    <>
      <Button type="button" size="sm" variant="outline" onClick={() => setOpen(true)}>
        {t("share.create")}
      </Button>
      <NativeModal open={open} labelledBy={titleId} onClose={close}>
        <div className="share-dialog">
          <div>
            <h2 id={titleId} className="project-dialog__title">
              {t("share.create")}
            </h2>
            <p className="share-dialog__empty">{t("share.document")}</p>
          </div>
          <section className="share-dialog__stack">
            <div className="share-dialog__field">
              <Label htmlFor={expiresId}>{t("share.expiresIn")}</Label>
              <select
                id={expiresId}
                className="document-page__field-select"
                value={String(expiresInDays)}
                disabled={pending}
                onChange={(event) => setExpiresInDays(Number(event.target.value))}
              >
                {SHARE_EXPIRES_OPTIONS.map((days) => {
                  const key = `share.expires.${days}`;
                  return (
                    <option key={days} value={String(days)}>
                      {isI18nKey(key) ? t(key) : String(days)}
                    </option>
                  );
                })}
              </select>
            </div>
            <Button
              type="button"
              size="sm"
              disabled={pending}
              onClick={() => create.mutate(expiresInDays)}
            >
              {t("share.create")}
            </Button>
            {createdUrl ? <RevealedShareUrl key={createdUrl} url={createdUrl} /> : null}
            {error ? (
              <p className="share-dialog__alert" role="alert">
                {error}
              </p>
            ) : null}
          </section>
          <section className="share-dialog__stack">
            {list.isLoading ? (
              <p role="status" className="share-dialog__empty">
                {t("load.loading")}
              </p>
            ) : null}
            {!list.isLoading && !list.isError && links.length === 0 ? (
              <p className="share-dialog__empty">{t("share.empty")}</p>
            ) : null}
            {links.length > 0 ? (
              <table className="share-dialog__table">
                <thead>
                  <tr>
                    <th scope="col">{t("share.document")}</th>
                    <th scope="col">{t("share.expires")}</th>
                    <th scope="col">
                      <span className="sr-only">{t("share.revoke")}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {links.map((row) => (
                    <tr key={row.id}>
                      <td>{row.documentId ? t("share.document") : t("share.project")}</td>
                      <td>{formatDate(row.expiresAt)}</td>
                      <td className="text-right">
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          className="text-destructive"
                          disabled={pending}
                          onClick={() => {
                            if (
                              !window.confirm(
                                `${t("share.revoke.confirm.title")}\n${t("share.revoke.confirm.body")}`,
                              )
                            ) {
                              return;
                            }
                            revoke.mutate(row.id);
                          }}
                        >
                          {t("share.revoke")}
                        </Button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : null}
          </section>
          <div className="flex justify-end">
            <Button type="button" size="sm" variant="outline" onClick={close}>
              {t("common.dismiss")}
            </Button>
          </div>
        </div>
      </NativeModal>
    </>
  );
}
