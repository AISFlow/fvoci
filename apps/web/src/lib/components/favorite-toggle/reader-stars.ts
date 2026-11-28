/** The reader's stars, as `WorkspaceFavorites` keeps them, for the ★ (`FavoriteToggle`). */
export interface ReaderStars {
  /** Whether a star is on it; undefined until the stars are read, and where none are kept. */
  readonly has: (targetId: string) => boolean | undefined;
  /** For whom and where they are now; undefined where none are kept. They are read again as it changes. */
  readonly scope: string | undefined;
  readonly read: () => Promise<void>;
  readonly toggle: (targetId: string) => Promise<void>;
}
