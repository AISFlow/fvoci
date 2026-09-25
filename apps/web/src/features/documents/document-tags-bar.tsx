// Adapted from source apps/web/src/features/documents/document-tags-bar.tsx.
// The popover becomes an inline disclosure panel (this app has no popover primitive).
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import {
  documentAssignedTagsQuery,
  documentTagPoolQuery,
  type DocumentTag,
} from "@/lib/queries/collections";
import { TagChip } from "./tag-chip";
import "@/features/collections/collections.css";

export function DocumentTagsBar({
  workspaceId,
  documentId,
  projectId,
  readOnly,
}: {
  workspaceId: string;
  documentId: string;
  projectId: string | null;
  readOnly: boolean;
}) {
  const queryClient = useQueryClient();
  const panelId = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const [mutationError, setMutationError] = useState<string | null>(null);
  const assignedOptions = documentAssignedTagsQuery(workspaceId, documentId, projectId);
  const assignedQuery = useQuery(assignedOptions);
  const poolQuery = useQuery({ ...documentTagPoolQuery(workspaceId), enabled: open && !readOnly });

  const assigned = assignedQuery.data ?? [];
  const assignedIds = new Set(assigned.map((tag) => tag.id));
  const pool = poolQuery.data?.items ?? [];
  const needle = filter.trim().toLowerCase();
  const candidates = pool.filter(
    (tag) => !assignedIds.has(tag.id) && (needle === "" || tag.name.toLowerCase().includes(needle)),
  );
  const exact = pool.find((tag) => tag.name.toLowerCase() === needle);
  const canCreate = (poolQuery.data?.canCreate ?? false) && needle !== "" && exact === undefined;

  async function invalidate() {
    await queryClient.invalidateQueries({ queryKey: ["document-tags", workspaceId] });
  }

  function close() {
    setOpen(false);
    setFilter("");
    triggerRef.current?.focus();
  }

  const assign = useMutation({
    mutationFn: async (tagId: string) => {
      const result = projectId
        ? await api.POST(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags",
            {
              params: {
                path: { workspace_id: workspaceId, project_id: projectId, document_id: documentId },
              },
              body: { tagId },
            },
          )
        : await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags", {
            params: { path: { workspace_id: workspaceId, document_id: documentId } },
            body: { tagId },
          });
      return ensureOk(result);
    },
    onMutate: () => setMutationError(null),
    onError: (err) => setMutationError(problemMessage(err, "error.http.fallback")),
    onSuccess: async (tag: DocumentTag) => {
      queryClient.setQueryData(assignedOptions.queryKey, (items: DocumentTag[] = []) => [
        ...items.filter((item) => item.id !== tag.id),
        tag,
      ]);
      await invalidate();
      close();
    },
  });

  const create = useMutation({
    mutationFn: async (name: string) => {
      try {
        return await ensureOk(
          await api.POST("/api/v1/workspaces/{workspace_id}/document-tags", {
            params: { path: { workspace_id: workspaceId } },
            body: { name, color: "gray" },
          }),
        );
      } catch (err) {
        // Source: a concurrent create of the same name reuses the existing tag.
        if (err instanceof ProblemError && err.status === 409) {
          const page = await queryClient.fetchQuery({
            ...documentTagPoolQuery(workspaceId, name),
            staleTime: 0,
          });
          const existing = page.items.find((tag) => tag.name.toLowerCase() === name.toLowerCase());
          if (existing) return existing;
        }
        throw err;
      }
    },
    onMutate: () => setMutationError(null),
    onError: (err) => setMutationError(problemMessage(err, "error.http.fallback")),
    onSuccess: async (created) => {
      await assign.mutateAsync(created.id).catch(() => undefined);
    },
  });

  const remove = useMutation({
    mutationFn: async (tagId: string) => {
      const result = projectId
        ? await api.DELETE(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags/{tag_id}",
            {
              params: {
                path: {
                  workspace_id: workspaceId,
                  project_id: projectId,
                  document_id: documentId,
                  tag_id: tagId,
                },
              },
            },
          )
        : await api.DELETE(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags/{tag_id}",
            {
              params: {
                path: { workspace_id: workspaceId, document_id: documentId, tag_id: tagId },
              },
            },
          );
      return ensureOk(result);
    },
    onMutate: () => setMutationError(null),
    onError: (err) => setMutationError(problemMessage(err, "error.http.fallback")),
    onSuccess: async (_ok, tagId) => {
      queryClient.setQueryData(assignedOptions.queryKey, (items: DocumentTag[] = []) =>
        items.filter((tag) => tag.id !== tagId),
      );
      await invalidate();
    },
  });

  const pending = assign.isPending || create.isPending || remove.isPending;

  if (readOnly && assignedQuery.isSuccess && assigned.length === 0) return null;

  return (
    <div className="tags-bar" role="group" aria-label={t("doc.tags")} data-testid="document-tags-bar">
      {assignedQuery.isPending ? <QueryLoading /> : null}
      {assignedQuery.isError ? (
        <QueryError
          message={loadErrorMessage(assignedQuery.error)}
          onRetry={() => void assignedQuery.refetch()}
        />
      ) : null}
      {assigned.map((tag) => (
        <span key={tag.id} className="tags-bar__item">
          <TagChip name={tag.name} color={tag.color} />
          {readOnly ? null : (
            <button
              type="button"
              className="tags-bar__remove"
              aria-label={`${t("doc.tags.remove")}: ${tag.name}`}
              disabled={pending}
              onClick={() => remove.mutate(tag.id)}
            >
              <span aria-hidden="true">×</span>
            </button>
          )}
        </span>
      ))}
      {readOnly ? null : (
        <Button
          ref={triggerRef}
          type="button"
          size="sm"
          variant="outline"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={() => (open ? close() : setOpen(true))}
        >
          {t("doc.tags.add")}
        </Button>
      )}
      {open && !readOnly ? (
        <div
          id={panelId}
          className="tags-bar__picker"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              close();
            }
          }}
        >
          <Input
            className="h-9"
            value={filter}
            aria-label={t("doc.tags")}
            maxLength={100}
            disabled={pending}
            autoFocus
            onChange={(event) => setFilter(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
              event.preventDefault();
              const first = exact && !assignedIds.has(exact.id) ? exact : candidates[0];
              if (first) assign.mutate(first.id);
              else if (canCreate) create.mutate(filter.trim());
            }}
          />
          {poolQuery.isPending ? <QueryLoading /> : null}
          {poolQuery.isError ? (
            <QueryError
              message={loadErrorMessage(poolQuery.error)}
              onRetry={() => void poolQuery.refetch()}
            />
          ) : null}
          {poolQuery.isSuccess ? (
            <ul className="tags-bar__options">
              {candidates.map((tag) => (
                <li key={tag.id}>
                  <button
                    type="button"
                    className="tags-bar__option"
                    disabled={pending}
                    onClick={() => assign.mutate(tag.id)}
                  >
                    <TagChip name={tag.name} color={tag.color} />
                  </button>
                </li>
              ))}
              {canCreate ? (
                <li>
                  <button
                    type="button"
                    className="tags-bar__option"
                    disabled={pending}
                    onClick={() => create.mutate(filter.trim())}
                  >
                    {t("doc.tags.create", { name: filter.trim() })}
                  </button>
                </li>
              ) : null}
              {candidates.length === 0 && !canCreate ? (
                <li className="px-2 py-1 text-ui text-muted-foreground">{t("doc.tags.empty")}</li>
              ) : null}
            </ul>
          ) : null}
        </div>
      ) : null}
      {mutationError ? (
        <p role="alert" className="basis-full text-ui text-destructive">
          {mutationError}
        </p>
      ) : null}
    </div>
  );
}
