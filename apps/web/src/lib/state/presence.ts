/** Ephemeral visits: a connection views one board or wiki and at most one card or page. */
export interface PresenceVisit {
  workspaceId: string;
  containerId: string;
  cardId?: string;
}

export interface PresentPerson {
  userId: string;
  /** The union of cards opened in this person's tabs. */
  cardIds: readonly string[];
}

export interface PresenceSnapshot {
  people: readonly PresentPerson[];
  anonymousCount: number;
  hiddenCount: number;
}

export interface Presence {
  setPresence(visit: PresenceVisit | null): void;
  watchPresence(containerId: string, hear: (snapshot: PresenceSnapshot) => void): () => void;
}

/** Where a backend keeps who is on its boards and wikis. */
export interface PresenceOfContainers {
  readonly presence: Presence;
}

export const EMPTY_PRESENCE: PresenceSnapshot = {
  people: [],
  anonymousCount: 0,
  hiddenCount: 0,
};
