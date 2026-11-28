/** Where the reader's stars lead (`SidebarFavorites`), given by who knows the addresses. */
import type { Favorite } from '$lib/state/index.ts';

export interface FavoritePlaces {
  /** Its address. */
  readonly href: (favorite: Favorite) => string;
  /** Goes there. */
  readonly open: (favorite: Favorite) => void;
}
