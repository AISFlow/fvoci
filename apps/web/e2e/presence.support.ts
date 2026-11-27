/**
 * Others on the open board, on the in-memory backend: other users of the same world
 * (`fvociDev.backend.as`) made members of it, who come onto it.
 */
import type { Page } from '@playwright/test';

export interface Dev {
  workspace: {
    me?: { id: string };
    board: {
      boardId: string;
      board: { workspaceId: string };
      members: { userId: string }[];
      addMember(userId: string, role: string): Promise<void>;
    };
  };
  backend: {
    world: { users: Map<string, Record<string, unknown>>; workspaceMembers: Set<string> };
    as(userId: string): {
      presence: { setPresence(visit: { workspaceId: string; containerId: string } | null): void };
    };
  };
}

/**
 * Makes `count` other users (made in the world, in its shared workspace, where it has
 * too few) members of the open board and has them come onto it; the number of its members then.
 */
export async function othersArrive(page: Page, count: number): Promise<number> {
  return page.evaluate(async (wanted) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const { users, workspaceMembers } = dev.backend.world;
    for (let made = users.size; made <= wanted; made += 1) {
      const id = crypto.randomUUID();
      workspaceMembers.add(id);
      users.set(id, {
        id,
        username: `visitor${String(made)}`,
        email: `visitor${String(made)}@example.com`,
        createdAt: 0,
        updatedAt: 0,
      });
    }
    const store = dev.workspace.board;
    const others = [...users.keys()].filter((id) => id !== dev.workspace.me?.id).slice(0, wanted);
    for (const id of others) {
      if (!store.members.some((member) => member.userId === id))
        await store.addMember(id, 'editor');
    }
    // Kept on the page: a visit lasts as long as its presence is not let go of.
    const visits = others.map((id) => dev.backend.as(id).presence);
    for (const presence of visits)
      presence.setPresence({ workspaceId: store.board.workspaceId, containerId: store.boardId });
    Object.assign(window, { visits });
    return store.members.length;
  }, count);
}
