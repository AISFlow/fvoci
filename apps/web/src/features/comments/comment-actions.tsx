import { t } from "@fvoci/i18n";
import { useMutation } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { loadErrorMessage } from "@/components/query-status";
import { ensureOk, api } from "@/lib/api";
import type { CommentsTargetKind } from "@/lib/queries/comments";
import { nextReplyTarget } from "./comment-drafts";
import type { CommentOutput } from "./comment-tree";

const REACTIONS = ["👍", "❤️", "🎉"] as const;

/** Comment mutations and draft state shared by the comment panel and the task activity feed. */
export function useCommentActions({
  workspaceId,
  kind,
  targetId,
  invalidate,
}: {
  workspaceId: string;
  kind: CommentsTargetKind;
  targetId: string;
  invalidate: () => Promise<unknown>;
}) {
  const [draft, setDraft] = useState("");
  const [replyDraft, setReplyDraft] = useState("");
  const [replyToId, setReplyToId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const onError = (error: unknown) => setActionError(loadErrorMessage(error));

  const create = useMutation({
    mutationFn: async (body: { text: string; parentId?: string | null }) =>
      ensureOk(
        kind === "document"
          ? await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments", {
              params: { path: { workspace_id: workspaceId, document_id: targetId } },
              body: {
                body: body.text,
                parentId: body.parentId ?? undefined,
              },
            })
          : await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments", {
              params: { path: { workspace_id: workspaceId, task_id: targetId } },
              body: {
                body: body.text,
                parentId: body.parentId ?? undefined,
              },
            }),
      ),
    onSuccess: async () => {
      setDraft("");
      setReplyDraft("");
      setReplyToId(null);
      setActionError(null);
      await invalidate();
    },
    onError,
  });

  const patch = useMutation({
    mutationFn: async ({ id, body }: { id: string; body: string }) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/comments/{comment_id}", {
          params: { path: { workspace_id: workspaceId, comment_id: id } },
          body: { body },
        }),
      ),
    onSuccess: async () => {
      setEditingId(null);
      setEditDraft("");
      setActionError(null);
      await invalidate();
    },
    onError,
  });

  const remove = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/comments/{comment_id}", {
          params: { path: { workspace_id: workspaceId, comment_id: id } },
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await invalidate();
    },
    onError,
  });

  const resolve = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve", {
          params: { path: { workspace_id: workspaceId, comment_id: id } },
        }),
      ),
    onSuccess: invalidate,
    onError,
  });

  const unresolve = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve", {
          params: { path: { workspace_id: workspaceId, comment_id: id } },
        }),
      ),
    onSuccess: invalidate,
    onError,
  });

  const react = useMutation({
    mutationFn: async ({ id, emoji, on }: { id: string; emoji: string; on: boolean }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions", {
          params: { path: { workspace_id: workspaceId, comment_id: id } },
          body: { emoji, on },
        }),
      ),
    onSuccess: invalidate,
    onError,
  });

  const pending =
    create.isPending ||
    patch.isPending ||
    remove.isPending ||
    resolve.isPending ||
    unresolve.isPending ||
    react.isPending;

  return {
    draft,
    setDraft,
    replyDraft,
    setReplyDraft,
    replyToId,
    setReplyToId,
    editingId,
    setEditingId,
    editDraft,
    setEditDraft,
    actionError,
    create,
    patch,
    remove,
    resolve,
    unresolve,
    react,
    pending,
  };
}

export type CommentActions = ReturnType<typeof useCommentActions>;

/** One comment with reactions, resolve, reply, edit and delete. */
export function CommentItem({
  comment,
  actions,
  currentUserId,
  readOnly,
  depth = 0,
  before,
  meta,
  children,
}: {
  comment: CommentOutput;
  actions: CommentActions;
  currentUserId: string;
  readOnly: boolean;
  depth?: number;
  before?: ReactNode;
  meta?: ReactNode;
  children?: ReactNode;
}) {
  const { pending } = actions;
  const isAuthor = comment.createdBy === currentUserId;
  const isRoot = comment.parentId == null;
  const resolved = comment.resolvedAt != null;
  const editing = actions.editingId === comment.id;

  return (
    <li className="comment-thread__item" style={depth ? { marginLeft: depth * 16 } : undefined}>
      {before}
      <p className="comment-thread__body">{comment.body}</p>
      {meta}
      <div className="comment-thread__actions">
        {REACTIONS.map((emoji) => {
          const summary = comment.reactions?.[emoji];
          const mine = summary?.reactedByMe ?? false;
          return (
            <Button
              key={emoji}
              type="button"
              size="sm"
              variant={mine ? "default" : "outline"}
              disabled={pending || readOnly}
              aria-pressed={mine}
              aria-label={`${t("comment.reaction")} ${emoji}`}
              onClick={() => void actions.react.mutateAsync({ id: comment.id, emoji, on: !mine })}
            >
              {emoji}
              {summary && summary.count > 0 ? ` ${summary.count}` : ""}
            </Button>
          );
        })}
        {!readOnly && isRoot ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={pending}
            onClick={() =>
              void (resolved
                ? actions.unresolve.mutate(comment.id)
                : actions.resolve.mutate(comment.id))
            }
          >
            {resolved ? t("comment.unresolve") : t("comment.resolve")}
          </Button>
        ) : null}
        {!readOnly ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={pending}
            onClick={() => {
              actions.setReplyToId((current) => nextReplyTarget(current, comment.id));
              actions.setReplyDraft("");
            }}
          >
            {t("comment.reply")}
          </Button>
        ) : null}
        {!readOnly && isAuthor ? (
          <>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => {
                actions.setReplyToId(null);
                actions.setEditingId(comment.id);
                actions.setEditDraft(comment.body);
              }}
            >
              {t("comment.edit")}
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => void actions.remove.mutateAsync(comment.id)}
            >
              {t("comment.delete")}
            </Button>
          </>
        ) : null}
      </div>
      {editing ? (
        <form
          className="comment-thread__compose"
          onSubmit={(event) => {
            event.preventDefault();
            void actions.patch.mutateAsync({ id: comment.id, body: actions.editDraft.trim() });
          }}
        >
          <textarea
            className="comment-thread__input"
            value={actions.editDraft}
            aria-label={t("comment.placeholder")}
            disabled={pending}
            onChange={(event) => actions.setEditDraft(event.target.value)}
          />
          <Button type="submit" size="sm" disabled={pending || actions.editDraft.trim() === ""}>
            {t("comment.save")}
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => {
              actions.setEditingId(null);
              actions.setEditDraft("");
            }}
          >
            {t("comment.edit.cancel")}
          </Button>
        </form>
      ) : null}
      {actions.replyToId === comment.id && !readOnly ? (
        <form
          data-comment-reply=""
          className="comment-thread__compose"
          onSubmit={(event) => {
            event.preventDefault();
            const text = actions.replyDraft.trim();
            if (!text) return;
            void actions.create.mutateAsync({ text, parentId: comment.id });
          }}
        >
          <textarea
            className="comment-thread__input"
            value={actions.replyDraft}
            aria-label={t("comment.placeholder")}
            disabled={pending}
            onChange={(event) => actions.setReplyDraft(event.target.value)}
          />
          <Button type="submit" size="sm" disabled={pending || actions.replyDraft.trim() === ""}>
            {t("comment.submit")}
          </Button>
        </form>
      ) : null}
      {children}
    </li>
  );
}

/** New root comment form. */
export function CommentCompose({ actions }: { actions: CommentActions }) {
  const { pending } = actions;
  return (
    <form
      data-comment-compose=""
      className="comment-thread__compose"
      onSubmit={(event) => {
        event.preventDefault();
        const text = actions.draft.trim();
        if (!text) return;
        void actions.create.mutateAsync({ text, parentId: null });
      }}
    >
      <textarea
        className="comment-thread__input"
        value={actions.draft}
        aria-label={t("comment.placeholder")}
        placeholder={t("comment.placeholder")}
        disabled={pending}
        onChange={(event) => actions.setDraft(event.target.value)}
      />
      <Button type="submit" disabled={pending || actions.draft.trim() === ""}>
        {t("comment.submit")}
      </Button>
    </form>
  );
}
