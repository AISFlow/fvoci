/**
 * The reader's stars in a workspace (`/favorites`): the boards, wikis,
 * courses, cards, pages and units they keep at hand, at the head of the
 * sidebar and of the search, in their order, at most `MAX_FAVORITES`. No
 * diary and no entry of one takes a star. Listed, a star shows what it is on
 * as the reader may read it now: what they do not read, or what is in the
 * trash, is left out. A person's alone; none through a public link.
 */
import type { ContainerKind } from '../logic/wire.ts';

/** The most stars a reader keeps in one workspace (the server's limit). */
export const MAX_FAVORITES = 100;

/** What a star is on. */
export type FavoriteKind = 'container' | 'card' | 'page' | 'unit';

export interface Favorite {
  /** What the star is on. */
  readonly targetId: string;
  readonly kind: FavoriteKind;
  /** Its title as it is now; empty for none. */
  readonly title: string;
  /** Its icon; empty for none. */
  readonly icon: string;
  /** A card's number on its board; none for anything else. */
  readonly number?: number;
  /** The board, wiki or course it is, or is of. */
  readonly containerId: string;
  readonly containerKind: ContainerKind;
}

export interface Favorites {
  /** The reader's stars in the workspace, in their order. */
  list(workspaceId: string): Promise<Favorite[]>;
  /**
   * Puts a star on what the reader reads in the workspace, after the others;
   * one there stays where it is. 404 for anything else, a diary and an entry
   * among it; 400 `too_many_favorites` past `MAX_FAVORITES`.
   */
  star(workspaceId: string, targetId: string): Promise<void>;
  /** Takes the star off; 404 where none is on it (taken off in another tab, say). */
  unstar(workspaceId: string, targetId: string): Promise<void>;
  /** Puts the stars in the order of `targetIds`, the others after them. */
  reorder(workspaceId: string, targetIds: readonly string[]): Promise<void>;
}

/** Where a backend keeps the reader's stars; none through a public link. */
export interface FavoritesOfBackend {
  readonly favorites: Favorites | undefined;
}
