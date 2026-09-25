import { formatPersonName, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { loadErrorMessage } from "@/components/query-status";
import { ensureOk, ProblemError, api } from "@/lib/api";
import { groupsQuery, membersQuery } from "@/lib/queries";
import type { CommentsTargetKind } from "@/lib/queries/comments";
import { nextReplyTarget } from "./comment-drafts";
import { mentionTargetsFromBody } from "./group-mentions";
import type { CommentOutput } from "./comment-tree";

const REACTIONS = ["👍", "❤️", "🎉"] as const;
const NONE = "";

type MentionGroup = { id: string; name: string };

function commentPostBody(
  text: string,
  parentId: string | null | undefined,
  members: ReadonlyArray<{ userId: string; name: string }>,
  groups: ReadonlyArray<MentionGroup>,
) {
  const mentions = mentionTargetsFromBody(text, members, groups);
  return {
    body: text,
    parentId: parentId ?? undefined,
    mentionedUserIds: mentions.mentionedUserIds,
    mentionedGroupIds: mentions.mentionedGroupIds,
  };
}

function appendGroupMention(body: string, name: string): string {
  const prefix = body.length === 0 || body.endsWith(" ") || body.endsWith("\n") ? "" : " ";
  return `${body}${prefix}@${name} `;
}

/** Comment mutations and draft state shared by the comment panel and the task activity feed. */
export function useCommentActions({
  workspaceId,
  kind,
  targetId,
  projectId = null,
  invalidate,
}: {
  workspaceId: string;
  kind: CommentsTargetKind;
  targetId: string;
  /** Set for comments on a project document (project-scoped comment route). */
  projectId?: string | null;
  invalidate: () => Promise<unknown>;
}) {
  const queryClient = useQueryClient();
  const project = projectId && projectId.length > 0 ? projectId : null;
  const groupsList = useQuery(groupsQuery(workspaceId));
  const groups = groupsList.error instanceof ProblemError ? [] : (groupsList.data?.items ?? []);
  const [draft, setDraft] = useState("");
  const [replyDraft, setReplyDraft] = useState("");
  const [replyToId, setReplyToId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const onError = (error: unknown) => setActionError(loadErrorMessage(error));

  const create = useMutation({
    mutationFn: async (body: { text: string; parentId?: string | null }) => {
      let members: ReadonlyArray<{ userId: string; name: string }> = [];
      let mentionGroups: ReadonlyArray<MentionGroup> = [];
      if (body.text.includes("@")) {
        const [membersResult, groupsResult] = await Promise.allSettled([
          queryClient.fetchQuery(membersQuery(workspaceId)),
          queryClient.fetchQuery(groupsQuery(workspaceId)),
        ]);
        if (membersResult.status === "fulfilled") {
          members = membersResult.value.items.map((member) => ({
            userId: member.userId,
            name: formatPersonName(member),
          }));
        } else if (!(membersResult.reason instanceof ProblemError)) {
          throw membersResult.reason;
        }
        if (groupsResult.status === "fulfilled") {
          mentionGroups = groupsResult.value.items;
        } else if (!(groupsResult.reason instanceof ProblemError)) {
          throw groupsResult.reason;
        }
      }
      const payload = commentPostBody(body.text, body.parentId, members, mentionGroups);
      return ensureOk(
        kind === "document"
          ? project
            ? await api.POST(
                "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments",
                {
                  params: {
                    path: {
                      workspace_id: workspaceId,
                      project_id: project,
                      document_id: targetId,
                    },
                  },
                  body: payload,
                },
              )
            : await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments", {
                params: { path: { workspace_id: workspaceId, document_id: targetId } },
                body: payload,
              })
          : await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments", {
              params: { path: { workspace_id: workspaceId, task_id: targetId } },
              body: payload,
            }),
      );
    },
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
    groups,
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
        <CommentCompose
          actions={actions}
          draft={actions.replyDraft}
          onDraftChange={actions.setReplyDraft}
          onSubmit={(text) => {
            void actions.create.mutateAsync({ text, parentId: comment.id });
          }}
          testAttr="data-comment-reply"
        />
      ) : null}
      {children}
    </li>
  );
}

/** Root or reply comment form with the group-mention picker. */
export function CommentCompose({
  actions,
  draft,
  onDraftChange,
  onSubmit,
  testAttr,
}: {
  actions: CommentActions;
  draft: string;
  onDraftChange: (body: string) => void;
  onSubmit: (body: string) => void;
  testAttr: "data-comment-compose" | "data-comment-reply";
}) {
  const { pending, groups } = actions;
  const attrs = { [testAttr]: "" };
  return (
    <form
      {...attrs}
      className="comment-thread__compose"
      onSubmit={(event) => {
        event.preventDefault();
        const text = draft.trim();
        if (!text) return;
        onSubmit(text);
      }}
    >
      <textarea
        className="comment-thread__input"
        value={draft}
        aria-label={t("comment.placeholder")}
        placeholder={t("comment.placeholder")}
        disabled={pending}
        onChange={(event) => onDraftChange(event.target.value)}
      />
      <div className="comment-thread__compose-row">
        {groups.length > 0 ? (
          <select
            className="comment-thread__mention"
            aria-label={t("group.mention")}
            value={NONE}
            disabled={pending}
            onChange={(event) => {
              const groupId = event.target.value;
              event.target.value = NONE;
              const group = groups.find((item) => item.id === groupId);
              if (!group) return;
              onDraftChange(appendGroupMention(draft, group.name));
            }}
          >
            <option value={NONE}>{t("group.mention")}</option>
            {groups.map((group) => (
              <option key={group.id} value={group.id}>
                {group.name}
              </option>
            ))}
          </select>
        ) : null}
        <Button type="submit" size="sm" disabled={pending || draft.trim() === ""}>
          {t("comment.submit")}
        </Button>
      </div>
    </form>
  );
}

/** New root comment form. */
export function RootCommentCompose({ actions }: { actions: CommentActions }) {
  return (
    <CommentCompose
      actions={actions}
      draft={actions.draft}
      onDraftChange={actions.setDraft}
      onSubmit={(text) => {
        void actions.create.mutateAsync({ text, parentId: null });
      }}
      testAttr="data-comment-compose"
    />
  );
}
