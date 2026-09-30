import { formatPersonName } from "@fvoci/i18n";
import type { QueryClient } from "@tanstack/query-core";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { groupsQuery, membersQuery } from "@/lib/queries";
import type { CommentsTargetKind } from "@/lib/queries/comments";
import { mentionTargetsFromBody } from "./group-mentions";

// Comment requests shared by the React and Vue comment panels.

export const REACTIONS = ["👍", "❤️", "🎉"] as const;

export type MentionGroup = { id: string; name: string };

export interface CommentTarget {
  workspaceId: string;
  kind: CommentsTargetKind;
  targetId: string;
  /** Set for comments on a project document (project-scoped comment route). */
  projectId: string | null;
}

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

export function appendGroupMention(body: string, name: string): string {
  const prefix = body.length === 0 || body.endsWith(" ") || body.endsWith("\n") ? "" : " ";
  return `${body}${prefix}@${name} `;
}

/** Posts a root comment or reply; `@name` mentions resolve against the workspace members and groups. */
export async function createComment(
  queryClient: QueryClient,
  target: CommentTarget,
  body: { text: string; parentId?: string | null },
) {
  const { workspaceId, kind, targetId } = target;
  const project = target.projectId && target.projectId.length > 0 ? target.projectId : null;
  let members: ReadonlyArray<{ userId: string; name: string }> = [];
  let mentionGroups: ReadonlyArray<MentionGroup> = [];
  if (body.text.includes("@")) {
    const [membersResult, groupsResult] = await Promise.allSettled([
      queryClient.query(membersQuery(workspaceId)),
      queryClient.query(groupsQuery(workspaceId)),
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
}

export async function patchComment(workspaceId: string, id: string, body: string) {
  return ensureOk(
    await api.PATCH("/api/v1/workspaces/{workspace_id}/comments/{comment_id}", {
      params: { path: { workspace_id: workspaceId, comment_id: id } },
      body: { body },
    }),
  );
}

export async function deleteComment(workspaceId: string, id: string) {
  return ensureOk(
    await api.DELETE("/api/v1/workspaces/{workspace_id}/comments/{comment_id}", {
      params: { path: { workspace_id: workspaceId, comment_id: id } },
    }),
  );
}

export async function resolveComment(workspaceId: string, id: string) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve", {
      params: { path: { workspace_id: workspaceId, comment_id: id } },
    }),
  );
}

export async function unresolveComment(workspaceId: string, id: string) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve", {
      params: { path: { workspace_id: workspaceId, comment_id: id } },
    }),
  );
}

export async function reactToComment(workspaceId: string, id: string, emoji: string, on: boolean) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions", {
      params: { path: { workspace_id: workspaceId, comment_id: id } },
      body: { emoji, on },
    }),
  );
}
