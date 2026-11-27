/**
 * Where an accepted invitation leads: into its workspace, or for a guest to
 * the board, wiki, or course they were invited to.
 */
import type { AcceptedInvitation } from '$lib/state/index.ts';

import type { Route } from './router.svelte.ts';

export function acceptedRoute(accepted: AcceptedInvitation): Route {
  const { workspaceId, containerId, containerKind } = accepted;
  if (!containerId) return { kind: 'workspace', workspaceId };
  if (containerKind === 'wiki') return { kind: 'wiki', workspaceId, wikiId: containerId };
  if (containerKind === 'course') return { kind: 'course', workspaceId, courseId: containerId };
  return { kind: 'board', workspaceId, boardId: containerId };
}
