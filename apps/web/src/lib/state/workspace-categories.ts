/**
 * Sidebar category mutations for `Workspace`: optimistic updates with rollback
 * and background re-read when the backend refuses a write, and undoable hiding.
 */
import type { Backend, Category, WorkspaceInfo } from './backend.ts';
import type { SidebarCategory } from './order.ts';
import type { SidebarWrites } from './sidebar-writes.ts';
import type { UndoManager } from '../logic/undo.ts';

export interface CategoryHost {
  readonly backend: Backend;
  readonly info: WorkspaceInfo | undefined;
  readonly sidebar: readonly SidebarCategory[];
  readonly undo: UndoManager;
  categories: Category[];
  report(action: 'save', error: unknown): void;
}

export class WorkspaceCategories {
  readonly #host: CategoryHost;
  readonly #writes: SidebarWrites;
  readonly #refresh: () => void;

  constructor(host: CategoryHost, writes: SidebarWrites, refresh: () => void) {
    this.#host = host;
    this.#writes = writes;
    this.#refresh = refresh;
  }

  /**
   * Shows a sidebar write at once. When it is refused the sidebar goes back
   * to what it was and is read again: what was taken back may be older than
   * what arrived from elsewhere meanwhile.
   */
  async #write(optimistic: Category[], send: () => Promise<unknown>): Promise<void> {
    const [before, workspace] = [this.#host.categories, this.#host.info];
    this.#host.categories = optimistic;
    this.#writes.begin();
    try {
      await send();
    } catch (error) {
      this.#refused(before, workspace);
      this.#host.report('save', error);
    } finally {
      this.#writes.end();
    }
  }

  #refused(before: Category[], workspace: WorkspaceInfo | undefined): void {
    if (this.#host.info !== workspace) return;
    this.#host.categories = before;
    this.#refresh();
  }

  /** A new category goes first, as the backend puts it. */
  async create(name: string): Promise<void> {
    const workspace = this.#host.info;
    if (!workspace) return;
    this.#writes.begin();
    try {
      const category = await this.#host.backend.createCategory(workspace.id, { name });
      // Shown already, or of a workspace left meanwhile: nothing to add.
      if (this.#host.categories.some((existing) => existing.id === category.id)) return;
      if (this.#host.info !== workspace) return;
      // The others were moved down to make room; until they are read again,
      // the new one is put before them.
      const first = Math.min(...this.#host.categories.map((existing) => existing.sortOrder));
      this.#host.categories = [
        ...this.#host.categories,
        { ...category, sortOrder: Math.min(category.sortOrder, first - 1) },
      ];
    } catch (error) {
      this.#host.report('save', error);
    } finally {
      this.#writes.end();
    }
  }

  async rename(categoryId: string, name: string): Promise<void> {
    const workspace = this.#host.info;
    if (!workspace) return;
    await this.#write(
      this.#host.categories.map((category) =>
        category.id === categoryId ? { ...category, name } : category,
      ),
      () => this.#host.backend.patchCategory(workspace.id, categoryId, { name }),
    );
  }

  async setCollapsed(categoryId: string, collapsed: boolean): Promise<void> {
    const workspace = this.#host.info;
    if (!workspace) return;
    await this.#write(
      this.#host.categories.map((category) =>
        category.id === categoryId ? { ...category, collapsed } : category,
      ),
      () => this.#host.backend.patchCategory(workspace.id, categoryId, { collapsed }),
    );
  }

  async remove(categoryId: string): Promise<void> {
    const workspace = this.#host.info;
    const gone = this.#host.categories.find((category) => category.id === categoryId);
    if (!workspace || !gone) return;
    await this.#write(
      this.#host.categories
        .filter((category) => category.id !== categoryId)
        .map((category) =>
          // Its boards go to the default category, those it hid still hidden.
          category.type === 'system'
            ? {
                ...category,
                containerIds: [...category.containerIds, ...gone.containerIds],
                hiddenContainerIds: [...category.hiddenContainerIds, ...gone.hiddenContainerIds],
              }
            : category,
        ),
      () => this.#host.backend.deleteCategory(workspace.id, categoryId),
    );
  }

  /** Sets the order of the categories; `categoryIds` names every one of them. */
  async reorder(categoryIds: readonly string[]): Promise<void> {
    const workspace = this.#host.info;
    if (!workspace) return;
    await this.#write(
      this.#host.categories.map((category) => ({
        ...category,
        sortOrder: categoryIds.indexOf(category.id) * 10,
      })),
      () => this.#host.backend.reorderCategories(workspace.id, [...categoryIds]),
    );
  }

  /** Files a board under a category, at `index` among the boards shown there. */
  async moveContainer(containerId: string, categoryId: string, index: number): Promise<void> {
    const workspace = this.#host.info;
    if (!workspace) return;
    await this.#write(
      this.#host.categories.map((category) => {
        const containerIds = category.containerIds.filter((id) => id !== containerId);
        if (category.id === categoryId) {
          containerIds.splice(Math.max(0, Math.min(index, containerIds.length)), 0, containerId);
        }
        return { ...category, containerIds };
      }),
      () =>
        this.#host.backend.moveContainerToCategory(workspace.id, containerId, categoryId, index),
    );
  }

  /** Hides a board or a wiki in its category, out of the sidebar. Hiding can be undone. */
  async hideContainer(containerId: string): Promise<void> {
    const entry = this.#host.sidebar.find((shown) =>
      shown.containers.some((board) => board.id === containerId),
    );
    if (!entry) return;
    const categoryId = entry.category.id;
    try {
      await this.#host.undo.perform(
        () => this.#setContainerHidden(categoryId, containerId, true),
        () => this.#setContainerHidden(categoryId, containerId, false),
        'hideContainer',
      );
    } catch (error) {
      this.#host.report('save', error);
    }
  }

  /** Shows a hidden board in the sidebar again, in the category that hides it. */
  async unhideContainer(containerId: string): Promise<void> {
    const category = this.#host.categories.find((one) =>
      one.hiddenContainerIds.includes(containerId),
    );
    if (!category) return;
    try {
      await this.#setContainerHidden(category.id, containerId, false);
    } catch (error) {
      this.#host.report('save', error);
    }
  }

  async #setContainerHidden(
    categoryId: string,
    containerId: string,
    hidden: boolean,
  ): Promise<void> {
    const workspace = this.#host.info;
    if (!workspace) return;
    const before = this.#host.categories;
    const category = before.find((one) => one.id === categoryId);
    // A board no category files (one brought back from the trash) is shown in
    // the default category but cannot be hidden there until it is filed there.
    const loose = hidden && category !== undefined && !category.containerIds.includes(containerId);
    this.#host.categories = before.map((one) => {
      if (one.id !== categoryId) return one;
      const others = one.hiddenContainerIds.filter((id) => id !== containerId);
      return {
        ...one,
        containerIds: loose ? [...one.containerIds, containerId] : one.containerIds,
        hiddenContainerIds: hidden ? [...others, containerId] : others,
      };
    });
    this.#writes.begin();
    try {
      if (loose) {
        const filed = category.containerIds.length;
        await this.#host.backend.moveContainerToCategory(
          workspace.id,
          containerId,
          categoryId,
          filed,
        );
      }
      await this.#host.backend.setContainerHidden(workspace.id, categoryId, containerId, hidden);
    } catch (error) {
      this.#refused(before, workspace);
      throw error;
    } finally {
      this.#writes.end();
    }
  }
}
