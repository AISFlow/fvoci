/**
 * The activity of a card (`getCardActivity`): its making, each change of its
 * title, of a property's value, of its parent card and of whether it is a
 * template, and its coming back from the trash, the newest first. One
 * person's changes of one thing within five minutes are one line, and a
 * change back to what it was takes the line away. A card keeps its 500
 * newest lines. Its body's changes are its versions' (`body-versions.ts`).
 *
 * And how a card is changed to repeat or stop (`logic/recurrence.ts`).
 */
import type { Recurrence } from '../logic/recurrence.ts';

import type { Paging } from './trash-reads.ts';

export type CardActivityKind =
  'created' | 'title' | 'property' | 'parent' | 'template' | 'restored';

export interface CardActivityLine {
  id: string;
  kind: CardActivityKind;
  /** The property whose value changed, for `property`; it may be one the board has no more. */
  propertyId: string | null;
  /**
   * What it was, as the card held it: the title, the property's stored value
   * (an option's or a person's identifier, a date value; `null` for none),
   * the parent's identifier (`null` for the top), whether it was a template.
   * `null` for `created` and `restored`.
   */
  before: unknown;
  /** What it became, as `before` says it. */
  after: unknown;
  /** Who did it; `null` for what the server did itself, and once their account is erased. */
  actorId: string | null;
  /** The token an agent did it through, as it was called then; `null` for a person's own doing. */
  actorTokenName: string | null;
  createdAt: number;
  /** When it was last done again (the same person within five minutes). */
  updatedAt: number;
}

/** How many lines a page of the activity has. */
export const ACTIVITY_PAGE = 20;

export interface CardActivity {
  /** A page of the card's lines, the newest first, for the board's members; 404 for a card in the trash. */
  list(boardId: string, cardId: string, paging?: Paging): Promise<CardActivityLine[]>;
}

/** Where a backend keeps the activity of cards; none through a public link. */
export interface CardActivityOfBackend {
  readonly cardActivity: CardActivity | undefined;
}

/** How a new card repeats, of a board that lets it. */
export interface RecurrenceDraft {
  recurrence?: Recurrence;
}

/** A change of how a card repeats: `null` makes it stop; left out, it stays as it is. */
export interface RecurrenceChange {
  recurrence?: Recurrence | null;
}
