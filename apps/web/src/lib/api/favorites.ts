/**
 * The reader's stars, on the server (`getFavorites`, `putFavorite`,
 * `deleteFavorite`, `reorderFavorites`).
 */
import type { Favorite, Favorites } from '../state/favorite-types.ts';

import type { Schemas } from './convert.ts';
import type { RequestOptions } from './http.ts';
import type { WorkspacePath } from './requests.ts';

/** A request under the workspace `workspaceId`. */
type Ask = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options: RequestOptions,
  workspaceId: string,
) => Promise<T>;

function favoriteOf(favorite: Schemas['Favorite']): Favorite {
  return {
    targetId: favorite.targetId,
    kind: favorite.kind,
    title: favorite.title,
    icon: favorite.icon,
    ...(favorite.number === undefined || favorite.number === null
      ? {}
      : { number: favorite.number }),
    containerId: favorite.containerId,
    containerKind: favorite.containerKind,
  };
}

export class ServerFavorites implements Favorites {
  readonly #ask: Ask;

  constructor(ask: Ask) {
    this.#ask = ask;
  }

  async list(workspaceId: string): Promise<Favorite[]> {
    const listed = await this.#ask<Schemas['Favorite'][]>('/favorites', {}, {}, workspaceId);
    return listed.map(favoriteOf);
  }

  async star(workspaceId: string, targetId: string): Promise<void> {
    await this.#ask('/favorites/{targetId}', { targetId }, { method: 'PUT' }, workspaceId);
  }

  async unstar(workspaceId: string, targetId: string): Promise<void> {
    await this.#ask('/favorites/{targetId}', { targetId }, { method: 'DELETE' }, workspaceId);
  }

  async reorder(workspaceId: string, targetIds: readonly string[]): Promise<void> {
    await this.#ask('/favorite-order', {}, { method: 'PUT', json: [...targetIds] }, workspaceId);
  }
}
