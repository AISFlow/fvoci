// Adapted from source apps/web/src/features/settings/settings-document-tags.tsx
// and routes/w.$slug.settings.document-tags.tsx (native table/select controls).
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { ConfirmActionButton } from "@/components/confirm-action";
import { EmptyState } from "@/components/empty-state";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { TagChip } from "@/features/documents/tag-chip";
import { api, ensureOk, ProblemError } from "@/lib/api";
import {
  documentTagPoolQuery,
  TAG_COLORS,
  type DocumentTagPoolItem,
  type TagColor,
} from "@/lib/queries/collections";
import "./settings-shell.css";
import "@/features/collections/collections.css";

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

function ColorSelect({
  id,
  value,
  disabled,
  onChange,
}: {
  id?: string;
  value: string;
  disabled?: boolean;
  onChange: (color: TagColor) => void;
}) {
  return (
    <select
      id={id}
      className="collection-select"
      aria-label={t("doc.tags.color")}
      value={value}
      disabled={disabled}
      onChange={(event) => {
        const next = TAG_COLORS.find((color) => color === event.target.value);
        if (next) onChange(next);
      }}
    >
      {TAG_COLORS.map((color) => (
        <option key={color} value={color}>
          {color}
        </option>
      ))}
    </select>
  );
}

export function DocumentTagsSettingsSection({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient();
  const nameId = useId();
  const colorId = useId();
  const [name, setName] = useState("");
  const [color, setColor] = useState<TagColor>("gray");
  const [actionError, setActionError] = useState<string | null>(null);
  const tags = useQuery(documentTagPoolQuery(workspaceId));

  async function invalidate() {
    await queryClient.invalidateQueries({ queryKey: ["document-tags", workspaceId] });
  }

  const create = useMutation({
    mutationFn: async (input: { name: string; color: TagColor }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/document-tags", {
          params: { path: { workspace_id: workspaceId } },
          body: input,
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      setName("");
      setColor("gray");
      await invalidate();
    },
    onError: (err) => setActionError(failMessage(err)),
  });
  const patch = useMutation({
    mutationFn: async (vars: { id: string; body: { name?: string; color?: TagColor } }) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/document-tags/{tag_id}", {
          params: { path: { workspace_id: workspaceId, tag_id: vars.id } },
          body: vars.body,
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await invalidate();
    },
    onError: async (err) => {
      setActionError(failMessage(err));
      await invalidate();
    },
  });
  const remove = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/document-tags/{tag_id}", {
          params: { path: { workspace_id: workspaceId, tag_id: id } },
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await invalidate();
    },
    onError: (err) => setActionError(failMessage(err)),
  });

  if (tags.isPending) return <QueryLoading />;
  if (tags.isError) {
    return (
      <QueryError message={loadErrorMessage(tags.error)} onRetry={() => void tags.refetch()} />
    );
  }

  const { canCreate, canManage, items } = tags.data;
  const pending = create.isPending || patch.isPending || remove.isPending;

  return (
    <section className="settings-section" data-testid="document-tags-settings">
      <h1 className="settings-section__title">{t("settings.documentTags.title")}</h1>
      {actionError ? (
        <p role="alert" className="task-form__alert">
          {actionError}
        </p>
      ) : null}
      {items.length === 0 ? (
        <EmptyState title={t("settings.documentTags.empty")} />
      ) : (
        <div className="data-table-wrap">
          <table className="data-table">
            <thead>
              <tr>
                <th scope="col">{t("doc.tags.name")}</th>
                <th scope="col">{t("doc.tags.color")}</th>
                <th scope="col">{t("doc.tags.assignments")}</th>
                {canManage ? <th scope="col"><span className="sr-only">{t("doc.tags.delete")}</span></th> : null}
              </tr>
            </thead>
            <tbody>
              {items.map((row: DocumentTagPoolItem) => (
                <tr key={row.id} data-testid={`document-tag-row-${row.name}`}>
                  <td>
                    {canManage ? (
                      <Input
                        key={`${row.id}:${row.name}`}
                        className="h-9"
                        defaultValue={row.name}
                        aria-label={`${t("doc.tags.rename")}: ${row.name}`}
                        maxLength={100}
                        disabled={pending}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                            event.currentTarget.blur();
                          }
                        }}
                        onBlur={(event) => {
                          const next = event.currentTarget.value.trim();
                          if (next === "" || next === row.name) {
                            event.currentTarget.value = row.name;
                            return;
                          }
                          patch.mutate({ id: row.id, body: { name: next } });
                        }}
                      />
                    ) : (
                      <TagChip name={row.name} color={row.color} />
                    )}
                  </td>
                  <td>
                    {canManage ? (
                      <ColorSelect
                        value={row.color}
                        disabled={pending}
                        onChange={(next) => patch.mutate({ id: row.id, body: { color: next } })}
                      />
                    ) : (
                      row.color
                    )}
                  </td>
                  <td className="settings-tabular">{row.assignmentCount}</td>
                  {canManage ? (
                    <td>
                      <ConfirmActionButton
                        title={t("doc.tags.delete.confirm.title")}
                        description={t("doc.tags.delete.confirm.body", {
                          count: row.assignmentCount,
                        })}
                        actionLabel={t("doc.tags.delete")}
                        disabled={pending}
                        onConfirm={async () => {
                          try {
                            await remove.mutateAsync(row.id);
                          } catch {
                            /* onError shows the problem title. */
                          }
                        }}
                      >
                        {t("doc.tags.delete")}
                      </ConfirmActionButton>
                    </td>
                  ) : null}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {canCreate ? (
        <form
          className="settings-form__row"
          onSubmit={(event) => {
            event.preventDefault();
            const trimmed = name.trim();
            if (trimmed === "" || pending) return;
            create.mutate({ name: trimmed, color });
          }}
        >
          <div className="collection-field">
            <Label htmlFor={nameId}>{t("doc.tags.name")}</Label>
            <Input
              id={nameId}
              value={name}
              maxLength={100}
              disabled={pending}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <div className="collection-field">
            <Label htmlFor={colorId}>{t("doc.tags.color")}</Label>
            <ColorSelect id={colorId} value={color} disabled={pending} onChange={setColor} />
          </div>
          <Button type="submit" size="sm" disabled={pending || name.trim() === ""}>
            {t("doc.tags.create.action")}
          </Button>
        </form>
      ) : null}
    </section>
  );
}
