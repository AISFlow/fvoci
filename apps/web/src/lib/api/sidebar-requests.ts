/**
 * Sidebar category HTTP operations for `ServerBackend` (`api/backend.ts`),
 * using single-transaction `PATCH /categories/{categoryId}` and
 * `PUT /categories/{categoryId}/containers/{containerId}` endpoints, serialized
 * per workspace via `BlockTurns`.
 */
import type { Category, CategoryDraft, CategoryPatch } from '../state/backend.ts';
import type { BlockTurns } from '../state/write.ts';

import { categoryOf, type Schemas } from './convert.ts';
import type { Http } from './http.ts';
import { underWorkspace } from './requests.ts';

export class ServerSidebar {
  readonly #http: Http;
  readonly #turns: BlockTurns;
  readonly #userId: () => Promise<string>;

  constructor(http: Http, turns: BlockTurns, userId: () => Promise<string>) {
    this.#http = http;
    this.#turns = turns;
    this.#userId = userId;
  }

  async categories(workspaceId: string): Promise<Category[]> {
    const categories = await this.#http.request<Schemas['CategoryContainers'][]>(
      underWorkspace('/categories'),
      { workspaceId },
    );
    return categories.map(categoryOf);
  }

  #turn<T>(workspaceId: string, write: () => Promise<T>): Promise<T> {
    return this.#turns.run(`sidebar ${workspaceId}`, write);
  }

  createCategory(workspaceId: string, draft: CategoryDraft): Promise<Category> {
    return this.#turn(workspaceId, async () => {
      const userId = await this.#userId();
      const request: Schemas['NewCategory'] = { name: draft.name, userId, workspaceId };
      const made = await this.#http.request<Schemas['Category']>(
        underWorkspace('/categories'),
        { workspaceId },
        { method: 'POST', json: request },
      );
      return categoryOf({ ...made, containerMetadata: [] });
    });
  }

  patchCategory(workspaceId: string, categoryId: string, patch: CategoryPatch): Promise<Category> {
    return this.#turn(workspaceId, async () => {
      const request: Schemas['CategoryPatch'] = {
        ...(patch.name === undefined ? {} : { name: patch.name }),
        ...(patch.collapsed === undefined ? {} : { collapsed: patch.collapsed }),
      };
      const updated = await this.#http.request<Schemas['Category']>(
        underWorkspace('/categories/{categoryId}'),
        { workspaceId, categoryId },
        { method: 'PATCH', json: request },
      );
      return categoryOf({ ...updated, containerMetadata: [] });
    });
  }

  deleteCategory(workspaceId: string, categoryId: string): Promise<void> {
    return this.#turn(workspaceId, async () => {
      await this.#http.request(
        underWorkspace('/categories/{categoryId}'),
        { workspaceId, categoryId },
        { method: 'DELETE' },
      );
    });
  }

  reorderCategories(workspaceId: string, categoryIds: string[]): Promise<void> {
    return this.#turn(workspaceId, async () => {
      await this.#http.request(
        underWorkspace('/category-order'),
        { workspaceId },
        { method: 'PUT', json: categoryIds },
      );
    });
  }

  moveContainerToCategory(
    workspaceId: string,
    containerId: string,
    categoryId: string,
    index: number,
  ): Promise<void> {
    return this.#turn(workspaceId, async () => {
      const json: Schemas['CategoryContainerUpdate'] = { index: Math.max(0, index) };
      await this.#http.request(
        underWorkspace('/categories/{categoryId}/containers/{containerId}'),
        { workspaceId, categoryId, containerId },
        { method: 'PUT', json },
      );
    });
  }

  setContainerHidden(
    workspaceId: string,
    categoryId: string,
    id: string,
    hidden: boolean,
  ): Promise<void> {
    return this.#turn(workspaceId, async () => {
      const json: Schemas['CategoryContainerUpdate'] = { hidden };
      await this.#http.request(
        underWorkspace('/categories/{categoryId}/containers/{containerId}'),
        { workspaceId, categoryId, containerId: id },
        { method: 'PUT', json },
      );
    });
  }
}
