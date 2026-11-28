/**
 * The reader's stars in the in-memory backend (`Backend.favorites`), as the
 * server keeps them (`crates/server/src/app/favorites.rs`): on a board, a wiki
 * or a course of the workspace the reader reads, no template, or on a card, a
 * page or a unit as a reference resolves it; never on a diary or an entry.
 * Listed as the reader reads them now: the rest is left out without a word.
 */
import type { Favorite, Favorites } from '../favorite-types.ts';
import { MAX_FAVORITES } from '../favorite-types.ts';
import type { References } from '../references.ts';

import { reads } from './container-access.ts';
import { fail } from './refuse.ts';
import { checkWorkspace } from './spaces.ts';
import type { World } from './world.ts';

interface Asker {
  readonly world: World;
  readonly userId: string;
  readonly references: References;
}

export class MemoryFavorites implements Favorites {
  readonly #asker: Asker;
  readonly #enter: () => Promise<void>;

  constructor(asker: Asker, enter: () => Promise<void>) {
    this.#asker = asker;
    this.#enter = enter;
  }

  #key(workspaceId: string): string {
    return `${this.#asker.userId}/${workspaceId}`;
  }

  #ids(workspaceId: string): string[] {
    return this.#asker.world.favorites.get(this.#key(workspaceId)) ?? [];
  }

  #keep(workspaceId: string, ids: string[]): void {
    const { world, userId } = this.#asker;
    world.favorites.set(this.#key(workspaceId), ids);
    world.tellUsers([userId], { kind: 'favorites', workspaceId });
  }

  /** The board, wiki or course as a star shows it, if the reader may star it. */
  #container(workspaceId: string, id: string): Favorite | undefined {
    const { world, userId } = this.#asker;
    const container = world.containers.get(id);
    if (
      container?.workspaceId !== workspaceId ||
      container.kind === 'diary' ||
      container.isTemplate ||
      !reads(world, userId, id)
    ) {
      return undefined;
    }
    const [icon, containerKind] = [container.icon ?? '', container.kind ?? 'board'];
    const { title } = container;
    return { targetId: id, kind: 'container', title, icon, containerId: id, containerKind };
  }

  async #shown(workspaceId: string, ids: readonly string[]): Promise<Favorite[]> {
    const blocks = new Map(
      (await this.#asker.references.resolve(workspaceId, ids)).map((linked) => [linked.id, linked]),
    );
    return ids.flatMap((id) => {
      const linked = blocks.get(id);
      if (!linked) return this.#container(workspaceId, id) ?? [];
      const { kind, title, icon, number, containerId, containerKind } = linked;
      // A view of a board is starred with its board, not apart from it.
      if (kind === 'view') return [];
      return [
        {
          targetId: id,
          kind,
          title,
          icon,
          ...(number === undefined ? {} : { number }),
          containerId,
          containerKind,
        },
      ];
    });
  }

  async list(workspaceId: string): Promise<Favorite[]> {
    await this.#enter();
    checkWorkspace(this.#asker.world, this.#asker.userId, workspaceId);
    return this.#shown(workspaceId, this.#ids(workspaceId));
  }

  async star(workspaceId: string, targetId: string): Promise<void> {
    await this.#enter();
    checkWorkspace(this.#asker.world, this.#asker.userId, workspaceId);
    if ((await this.#shown(workspaceId, [targetId])).length === 0) {
      fail(404, 'no such favorite target');
    }
    const ids = this.#ids(workspaceId);
    if (ids.includes(targetId)) return;
    if (ids.length >= MAX_FAVORITES) {
      fail(400, `a user keeps at most ${String(MAX_FAVORITES)} favorites in a workspace`, {
        reason: 'too_many_favorites',
        limit: MAX_FAVORITES,
      });
    }
    this.#keep(workspaceId, [...ids, targetId]);
  }

  async unstar(workspaceId: string, targetId: string): Promise<void> {
    await this.#enter();
    checkWorkspace(this.#asker.world, this.#asker.userId, workspaceId);
    const ids = this.#ids(workspaceId);
    if (!ids.includes(targetId)) fail(404, 'favorite not found');
    this.#keep(
      workspaceId,
      ids.filter((id) => id !== targetId),
    );
  }

  async reorder(workspaceId: string, targetIds: readonly string[]): Promise<void> {
    await this.#enter();
    checkWorkspace(this.#asker.world, this.#asker.userId, workspaceId);
    if (targetIds.length > MAX_FAVORITES) fail(400, 'too many favorites to put in order');
    const ids = this.#ids(workspaceId);
    const first = targetIds.filter((id, at) => ids.includes(id) && targetIds.indexOf(id) === at);
    this.#keep(workspaceId, [...first, ...ids.filter((id) => !first.includes(id))]);
  }
}
