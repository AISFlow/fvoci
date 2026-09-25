import { formatPersonName, t } from "@fvoci/i18n";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { EmptyState } from "@/components/empty-state";
import { Button } from "@/components/ui/button";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { ensureOk, ProblemError, api } from "@/lib/api";
import { commentsQuery, type CommentsTargetKind } from "@/lib/queries/comments";
import { groupsQuery, membersQuery } from "@/lib/queries";
import { nextReplyTarget } from "./comment-drafts";
import { mentionTargetsFromBody } from "./group-mentions";
import { buildCommentTree, type CommentNode } from "./comment-tree";
import "./comments.css";

const REACTIONS = ["👍", "❤️", "🎉"] as const;
const NONE = "";

interface CommentPanelProps {
  workspaceId: string;
  kind: CommentsTargetKind;
  targetId: string;
  currentUserId: string;
  readOnly?: boolean;
  projectId?: string | null;
}

function commentPostBody(
  text: string,
  parentId: string | null | undefined,
  members: ReadonlyArray<{ userId: string; name: string }>,
  groups: ReadonlyArray<{ id: string; name: string }>,
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

function CommentCompose({
  pending,
  groups,
  draft,
  onDraftChange,
  onSubmit,
  testAttr,
}: {
  pending: boolean;
  groups: ReadonlyArray<{ id: string; name: string }>;
  draft: string;
  onDraftChange: (body: string) => void;
  onSubmit: (body: string) => void;
  testAttr: "data-comment-compose" | "data-comment-reply";
}) {
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

export function CommentPanel({
  workspaceId,
  kind,
  targetId,
  currentUserId,
  readOnly = false,
  projectId = null,
}: CommentPanelProps) {
  const queryClient = useQueryClient();
  const project = projectId && projectId.length > 0 ? projectId : null;
  const list = useInfiniteQuery(commentsQuery(workspaceId, kind, targetId, project));
  const groupsList = useQuery(groupsQuery(workspaceId));
  const [draft, setDraft] = useState("");
  const [replyDraft, setReplyDraft] = useState("");
  const [replyToId, setReplyToId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);

  const items = useMemo(
    () => list.data?.pages.flatMap((page) => page.items) ?? [],
    [list.data],
  );
  const roots = useMemo(() => buildCommentTree(items), [items]);
  const testId = kind === "document" ? "document-comments" : "task-comments";
  const groups = groupsList.error instanceof ProblemError ? [] : (groupsList.data?.items ?? []);

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ["comments", workspaceId, kind, targetId] });
  };

  const create = useMutation({
    mutationFn: async (body: { text: string; parentId?: string | null }) => {
      let members: ReadonlyArray<{ userId: string; name: string }> = [];
      let mentionGroups: ReadonlyArray<{ id: string; name: string }> = [];
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
    onError: (error: unknown) => setActionError(loadErrorMessage(error)),
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
    onError: (error: unknown) => setActionError(loadErrorMessage(error)),
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
    onError: (error: unknown) => setActionError(loadErrorMessage(error)),
  });

  const resolve = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve", {
          params: { path: { workspace_id: workspaceId, comment_id: id } },
        }),
      ),
    onSuccess: invalidate,
    onError: (error: unknown) => setActionError(loadErrorMessage(error)),
  });

  const unresolve = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve", {
          params: { path: { workspace_id: workspaceId, comment_id: id } },
        }),
      ),
    onSuccess: invalidate,
    onError: (error: unknown) => setActionError(loadErrorMessage(error)),
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
    onError: (error: unknown) => setActionError(loadErrorMessage(error)),
  });

  const pending =
    create.isPending ||
    patch.isPending ||
    remove.isPending ||
    resolve.isPending ||
    unresolve.isPending ||
    react.isPending;

  function renderNode(node: CommentNode, depth = 0) {
    const { comment } = node;
    const isAuthor = comment.createdBy === currentUserId;
    const isRoot = comment.parentId == null;
    const resolved = comment.resolvedAt != null;
    const editing = editingId === comment.id;

    return (
      <li key={comment.id} className="comment-thread__item" style={{ marginLeft: depth * 16 }}>
        <p className="comment-thread__body">{comment.body}</p>
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
                onClick={() => void react.mutateAsync({ id: comment.id, emoji, on: !mine })}
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
                void (resolved ? unresolve.mutate(comment.id) : resolve.mutate(comment.id))
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
                setReplyToId((current) => nextReplyTarget(current, comment.id));
                setReplyDraft("");
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
                  setReplyToId(null);
                  setEditingId(comment.id);
                  setEditDraft(comment.body);
                }}
              >
                {t("comment.edit")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={pending}
                onClick={() => void remove.mutateAsync(comment.id)}
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
              void patch.mutateAsync({ id: comment.id, body: editDraft.trim() });
            }}
          >
            <textarea
              className="comment-thread__input"
              value={editDraft}
              aria-label={t("comment.placeholder")}
              disabled={pending}
              onChange={(event) => setEditDraft(event.target.value)}
            />
            <Button type="submit" size="sm" disabled={pending || editDraft.trim() === ""}>
              {t("comment.save")}
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => {
                setEditingId(null);
                setEditDraft("");
              }}
            >
              {t("comment.edit.cancel")}
            </Button>
          </form>
        ) : null}
        {replyToId === comment.id && !readOnly ? (
          <CommentCompose
            pending={pending}
            groups={groups}
            draft={replyDraft}
            onDraftChange={setReplyDraft}
            onSubmit={(text) => {
              void create.mutateAsync({ text, parentId: comment.id });
            }}
            testAttr="data-comment-reply"
          />
        ) : null}
        {node.children.length > 0 ? (
          <ul className="comment-thread__list">
            {node.children.map((child) => renderNode(child, depth + 1))}
          </ul>
        ) : null}
      </li>
    );
  }

  if (list.isLoading) return <QueryLoading />;
  if (list.error) {
    const notFound = list.error instanceof ProblemError && list.error.status === 404;
    if (notFound) return null;
    return (
      <QueryError
        message={loadErrorMessage(list.error)}
        onRetry={() => {
          void list.refetch();
        }}
      />
    );
  }

  return (
    <section
      id={testId}
      className="comment-panel"
      aria-label={t("comment.title")}
      data-testid={testId}
    >
      <h2 className="comment-panel__title">{t("comment.title")}</h2>
      {actionError ? <p role="alert" className="comment-panel__error">{actionError}</p> : null}
      {roots.length === 0 ? <EmptyState title={t("comment.empty")} /> : null}
      {roots.length > 0 ? (
        <ul className="comment-thread__list">{roots.map((root) => renderNode(root))}</ul>
      ) : null}
      {list.hasNextPage ? (
        <Button
          type="button"
          variant="outline"
          disabled={list.isFetchingNextPage}
          onClick={() => {
            void list.fetchNextPage();
          }}
        >
          {t("comment.list.loadMore")}
        </Button>
      ) : null}
      {!readOnly ? (
        <CommentCompose
          pending={pending}
          groups={groups}
          draft={draft}
          onDraftChange={setDraft}
          onSubmit={(text) => {
            void create.mutateAsync({ text, parentId: null });
          }}
          testAttr="data-comment-compose"
        />
      ) : null}
    </section>
  );
}
