// Ported from source apps/web/src/features/tasks/task-detail.tsx (the
// attachments section of the task body): list, paste to attach, delete with
// confirmation. A file picker is added beside paste so keyboard-only users
// can attach without the clipboard.
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useMemo, useRef, useState } from "react";
import { ConfirmActionButton } from "@/components/confirm-action";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { createTaskAttachmentBridge } from "@/features/workspace/attachment-upload";

export function taskAttachmentsQueryKey(workspaceId: string, taskId: string) {
  return ["task-attachments", workspaceId, taskId] as const;
}

function attachmentErrorMessage(error: unknown): string {
  return error instanceof ProblemError ? error.title : t("error.network");
}

export function TaskAttachmentsPanel({
  workspaceId,
  taskId,
  readOnly,
}: {
  workspaceId: string;
  taskId: string;
  readOnly: boolean;
}) {
  const queryClient = useQueryClient();
  const inputId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const bridge = useMemo(
    () => createTaskAttachmentBridge(workspaceId, taskId),
    [workspaceId, taskId],
  );
  const queryKey = taskAttachmentsQueryKey(workspaceId, taskId);
  const attachments = useQuery({
    queryKey,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/attachments", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
        }),
      ),
  });
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const remove = useMutation({
    mutationFn: async (attachmentId: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}", {
          params: { path: { workspace_id: workspaceId, attachment_id: attachmentId } },
        }),
      ),
    onSuccess: async () => {
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (err) => setError(attachmentErrorMessage(err)),
  });

  async function attach(files: File[]) {
    if (readOnly || files.length === 0) return;
    setError(null);
    setUploading(true);
    try {
      await Promise.all(files.map((file) => bridge.upload(file, () => {})));
    } catch (err) {
      setError(attachmentErrorMessage(err));
    } finally {
      setUploading(false);
      await queryClient.invalidateQueries({ queryKey });
    }
  }

  const items = attachments.data?.items ?? [];
  if (readOnly && items.length === 0) return null;

  return (
    <section
      className="group/attachments flex flex-col gap-1.5"
      aria-label={t("search.tab.attachment")}
      tabIndex={readOnly ? undefined : 0}
      onPaste={(event) => {
        if (readOnly) return;
        const files = [...event.clipboardData.files];
        if (files.length === 0) return;
        event.preventDefault();
        void attach(files);
      }}
    >
      {!readOnly ? (
        <div className="flex items-center gap-2">
          <label
            htmlFor={inputId}
            className="inline-flex h-9 cursor-pointer items-center rounded-md border border-border px-3 text-ui"
          >
            {t("task.attach.pick")}
          </label>
          <input
            id={inputId}
            ref={inputRef}
            type="file"
            multiple
            className="sr-only"
            disabled={uploading}
            onChange={(event) => {
              const files = [...(event.currentTarget.files ?? [])];
              event.currentTarget.value = "";
              void attach(files);
            }}
          />
          <p className="text-ui text-muted-foreground">
            {uploading ? t("task.attach.uploading") : t("task.attach.paste")}
          </p>
        </div>
      ) : null}
      <ul className="flex flex-col gap-1 text-doc">
        {items.map((a) => (
          <li key={a.id} className="flex items-center justify-between gap-2">
            {a.completedAt ? (
              <a
                className="min-w-0 truncate break-all underline"
                href={`/api/v1/workspaces/${workspaceId}/attachments/${a.id}/download`}
                download={a.name}
              >
                {a.name}
              </a>
            ) : (
              <span className="min-w-0 truncate break-all text-muted-foreground">{a.name}</span>
            )}
            {!readOnly ? (
              <ConfirmActionButton
                title={t("task.attach.delete.confirm.title")}
                description={t("task.attach.delete.confirm.body", { name: a.name })}
                actionLabel={t("task.attach.delete")}
                disabled={remove.isPending}
                onConfirm={() => remove.mutateAsync(a.id).then(() => undefined)}
              >
                {t("task.attach.delete")}
              </ConfirmActionButton>
            ) : null}
          </li>
        ))}
      </ul>
      {error ? (
        <p role="alert" className="text-ui text-destructive">
          {error}
        </p>
      ) : null}
    </section>
  );
}
