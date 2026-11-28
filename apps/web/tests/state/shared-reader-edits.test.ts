/**
 * A reader by a board's public link may only read it: an open board whose
 * least role writes is not joined by one who is nobody (`join-on-write.ts`),
 * so nothing of the board is editable to them, the bar of a selection
 * included. One who works in the workspace still writes in it as joined
 * (join-on-write.test.ts, "a write off the body of an open board").
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SHARED_PLACE, SharedBackend } from '$lib/api/shared-backend.ts';
import type { MemberRole } from '$lib/logic/container-members.ts';
import { Workspace } from '$lib/state/index.ts';

/** The board the link names, as the server gives it to a reader by the link. */
const boardOf = (visibility: 'open' | 'private', minimumRole: MemberRole) => ({
  id: 'b1',
  workspaceId: '',
  title: 'Shared',
  visibility,
  minimumRole,
  cardProperties: [],
  properties: {},
  createdAt: 1,
  updatedAt: 1,
});

/** Answers the board's own address with `board`, and every other reading with nothing. */
function serve(board: unknown): void {
  vi.stubGlobal('fetch', (url: string) => {
    const path = url.split('?')[0] ?? '';
    const answer = path.endsWith('/shared/b1') ? board : [];
    return Promise.resolve(
      new Response(JSON.stringify(answer), { headers: { 'Content-Type': 'application/json' } }),
    );
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('a reader by the public link', () => {
  it.each([
    { visibility: 'private', minimumRole: 'viewer' },
    { visibility: 'open', minimumRole: 'viewer' },
    { visibility: 'open', minimumRole: 'editor' },
  ] as const)(
    'edits nothing of a $visibility board whose least role is $minimumRole',
    async ({ visibility, minimumRole }) => {
      serve(boardOf(visibility, minimumRole));
      const workspace = new Workspace({
        backend: new SharedBackend('', 'b1', 't'),
        locale: () => 'en',
      });
      await workspace.load(SHARED_PLACE);
      const board = await workspace.openBoard('b1');
      expect(board.board?.visibility).toBe(visibility);
      expect([board.joinsOnWrite, board.canEdit, board.canComment]).toEqual([false, false, false]);
    },
  );
});
