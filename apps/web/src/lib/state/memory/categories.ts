/**
 * The categories of the sidebar, for the backend in memory, as the server
 * keeps them: each person's own, in each workspace they work in (the world
 * holds them by person, each naming its workspace). What a person is told of
 * them is the categories of one workspace. A board or a wiki made, or one a
 * person is added to, is filed under their default category of its
 * workspace (`fileAsNew`).
 */
import type { Container } from '../../logic/wire.ts';
import type { Category, CategoryDraft, CategoryPatch } from '../backend.ts';

import { fail } from './refuse.ts';
import { copy, scramble } from './shapes.ts';
import type { World } from './world.ts';

/** The user's categories in the workspace: those the world holds, not copies. */
export function categoriesIn(world: World, userId: string, workspaceId: string): Category[] {
  return (world.categories.get(userId) ?? []).filter(
    (category) => category.workspaceId === workspaceId,
  );
}

/** Tells the user their categories of the workspace, as they are now. */
export function tellCategories(world: World, userId: string, workspaceId: string): void {
  world.tellUsers([userId], {
    kind: 'categories',
    categories: copy(categoriesIn(world, userId, workspaceId)),
  });
}

/**
 * Files a board or a wiki under a user's default category of its workspace,
 * as the server does for a new one (not a template) and for a new member of one.
 */
export function fileAsNew(world: World, userId: string, container: Container): void {
  if (container.isTemplate) return;
  const theirs = categoriesIn(world, userId, container.workspaceId);
  const fallback = theirs.find((category) => category.type === 'system');
  if (!fallback || theirs.some(({ containerIds }) => containerIds.includes(container.id))) return;
  fallback.containerIds.push(container.id);
  tellCategories(world, userId, container.workspaceId);
}

/** The calls of categories, each of which enters the workspace it names first. */
export type CategoryMethod =
  | 'categories'
  | 'createCategory'
  | 'patchCategory'
  | 'deleteCategory'
  | 'reorderCategories'
  | 'moveContainerToCategory'
  | 'setContainerHidden';

/** Who asks, and of which world. */
interface Asker {
  readonly world: World;
  readonly userId: string;
}

export class MemoryCategories {
  readonly #asker: Asker;
  readonly #enter: (method: CategoryMethod, workspaceId: string) => Promise<void>;

  constructor(asker: Asker, enter: (method: CategoryMethod, workspaceId: string) => Promise<void>) {
    this.#asker = asker;
    this.#enter = enter;
  }

  #mine(workspaceId: string): Category[] {
    return categoriesIn(this.#asker.world, this.#asker.userId, workspaceId);
  }

  #category(workspaceId: string, categoryId: string): Category {
    return (
      this.#mine(workspaceId).find((category) => category.id === categoryId) ??
      fail(404, 'no such category')
    );
  }

  #tell(workspaceId: string): void {
    tellCategories(this.#asker.world, this.#asker.userId, workspaceId);
  }

  async categories(workspaceId: string): Promise<Category[]> {
    await this.#enter('categories', workspaceId);
    return scramble(this.#mine(workspaceId)).map(copy);
  }

  async createCategory(workspaceId: string, draft: CategoryDraft): Promise<Category> {
    await this.#enter('createCategory', workspaceId);
    if (draft.name.trim() === '') fail(400, 'a category needs a name', { field: 'name' });
    const { world, userId } = this.#asker;
    const now = world.now();
    // A new category is placed first in the sidebar.
    for (const category of this.#mine(workspaceId)) category.sortOrder += 10;
    const category: Category = {
      id: world.newId(),
      name: draft.name,
      userId,
      workspaceId,
      type: 'custom',
      collapsed: false,
      sortOrder: 0,
      containerIds: [],
      hiddenContainerIds: [],
      createdAt: now,
      updatedAt: now,
    };
    world.categories.set(userId, [...(world.categories.get(userId) ?? []), category]);
    this.#tell(workspaceId);
    return copy(category);
  }

  async patchCategory(
    workspaceId: string,
    categoryId: string,
    patch: CategoryPatch,
  ): Promise<Category> {
    await this.#enter('patchCategory', workspaceId);
    const category = this.#category(workspaceId, categoryId);
    if (patch.name !== undefined) {
      if (patch.name.trim() === '') fail(400, 'a category needs a name', { field: 'name' });
      // The default category keeps its name, as the server's does.
      if (category.type !== 'system') category.name = patch.name;
    }
    if (patch.collapsed !== undefined) category.collapsed = patch.collapsed;
    category.updatedAt = this.#asker.world.now();
    this.#tell(workspaceId);
    return copy(category);
  }

  async deleteCategory(workspaceId: string, categoryId: string): Promise<void> {
    await this.#enter('deleteCategory', workspaceId);
    const category = this.#category(workspaceId, categoryId);
    if (category.type === 'system') fail(403, 'the default category cannot be deleted');
    // Its boards fall back to the default category, those it hid still hidden.
    const fallback = this.#mine(workspaceId).find((candidate) => candidate.type === 'system');
    fallback?.containerIds.push(...category.containerIds);
    fallback?.hiddenContainerIds.push(...category.hiddenContainerIds);
    const { world, userId } = this.#asker;
    world.categories.set(
      userId,
      (world.categories.get(userId) ?? []).filter((other) => other !== category),
    );
    this.#tell(workspaceId);
  }

  async reorderCategories(workspaceId: string, categoryIds: string[]): Promise<void> {
    await this.#enter('reorderCategories', workspaceId);
    const mine = this.#mine(workspaceId);
    const known = new Set(mine.map((category) => category.id));
    if (categoryIds.length !== mine.length || !categoryIds.every((id) => known.has(id))) {
      fail(400, 'the order must name every category once');
    }
    for (const category of mine) category.sortOrder = categoryIds.indexOf(category.id) * 10;
    this.#tell(workspaceId);
  }

  async moveContainerToCategory(
    workspaceId: string,
    id: string,
    categoryId: string,
    index: number,
  ): Promise<void> {
    await this.#enter('moveContainerToCategory', workspaceId);
    if (!this.#asker.world.containers.has(id)) fail(404, 'no board');
    const target = this.#category(workspaceId, categoryId);
    for (const category of this.#mine(workspaceId)) {
      category.containerIds = category.containerIds.filter((other) => other !== id);
    }
    const at = Math.max(0, Math.min(index, target.containerIds.length));
    target.containerIds.splice(at, 0, id);
    this.#tell(workspaceId);
  }

  async setContainerHidden(
    workspaceId: string,
    categoryId: string,
    id: string,
    hidden: boolean,
  ): Promise<void> {
    await this.#enter('setContainerHidden', workspaceId);
    const category = this.#category(workspaceId, categoryId);
    const others = category.hiddenContainerIds.filter((other) => other !== id);
    // As the server: a board the category does not hold is no error.
    category.hiddenContainerIds =
      hidden && category.containerIds.includes(id) ? [...others, id] : others;
    this.#tell(workspaceId);
  }
}
