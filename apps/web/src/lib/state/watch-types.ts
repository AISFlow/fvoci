/**
 * What the reader watches (`/subscriptions`): the cards, pages and units they
 * are told of when someone comments on them or changes them. Whoever writes
 * one watches it already, and may stop and start again.
 */

/** What can be watched. A chapter of a course cannot (404). */
export type WatchedKind = 'card' | 'page' | 'unit';

export interface Watches {
  /** What `userId` (the reader; another is 404) watches in the workspace, by its identifier. */
  watching(workspaceId: string, userId: string): Promise<string[]>;
  /** Watches a block the reader may read; watching it again is no error. */
  watch(workspaceId: string, blockId: string, kind: WatchedKind, userId: string): Promise<void>;
  /** Stops watching it; one not watched is 404. */
  unwatch(workspaceId: string, blockId: string, userId: string): Promise<void>;
}

/** Where a backend keeps what the reader watches; none through a public link. */
export interface WatchesOfBackend {
  readonly watches: Watches | undefined;
}
