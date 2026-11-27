/** The categories of the reader's sidebar, as the backend has them (`Backend.categories`). */

/** A category of the reader's sidebar, with the boards filed under it. */
export interface Category {
  id: string;
  name: string;
  userId: string;
  workspaceId: string;
  /** `system` is the one every reader has and cannot delete. */
  type: 'custom' | 'system';
  collapsed: boolean;
  /** The reader's order of the categories; ties are the client's to break. */
  sortOrder: number;
  /** The boards in the reader's order. */
  containerIds: string[];
  /** Boards of this category the reader hid from the sidebar. */
  hiddenContainerIds: string[];
  createdAt: number;
  updatedAt: number;
}

export interface CategoryDraft {
  name: string;
}

export interface CategoryPatch {
  name?: string;
  collapsed?: boolean;
}
