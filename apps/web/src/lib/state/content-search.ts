/**
 * Finding cards and pages by what they say, part of the data boundary
 * (`backend.ts`): the server's `searchWorkspaceContent`, which its search
 * server answers, or its database where it has none (`searchEnabled` of the
 * client configuration says so). A search that is not answered in time, or
 * whose search server cannot hold its index, is 503 `search_unavailable`.
 */

import type { ContainerKind } from '../logic/wire.ts';

/** What a search finds: a card of a board, a page of a wiki, an entry of a diary, or a unit of a course. */
export type SearchKind = 'card' | 'page' | 'entry' | 'unit';

/** A stretch of a snippet: plain text, which matched what was asked or not. */
export interface SnippetPart {
  text: string;
  matched: boolean;
}

/** One thing a search found, which the reader reads now. */
export interface SearchHit {
  id: string;
  kind: SearchKind;
  /** The board the card is on, the wiki the page is in, or the diary the entry is in. */
  containerId: string;
  containerKind: ContainerKind;
  /** The title of the board, wiki, diary or course; empty if untitled. */
  containerTitle?: string;
  /** Its title as it is now. */
  title: string;
  /** A card's number on its board; none for a page. */
  number?: number;
  /**
   * A piece of its body around what matched, its stretches one after the
   * other; empty for a body that changed a moment ago, or has nothing to show.
   */
  snippet: SnippetPart[];
}

/** What a search asks for. */
export interface SearchAsk {
  /** The words, at most `MAX_SEARCH_CHARS` characters; empty finds nothing. */
  query: string;
  /** One kind only; both when left out. */
  kind?: SearchKind;
  /** How many at most, from 1 to 50; 20 when left out. */
  limit?: number;
}

/** The most characters the words of a search may have. */
export const MAX_SEARCH_CHARS = 200;

/** The reason of the 503 of a search that is not answered in time, or not at all. */
export const SEARCH_UNAVAILABLE = 'search_unavailable';

export interface ContentSearch {
  /** Whether the bodies of cards and pages can be searched here at all. */
  enabled(): Promise<boolean>;
  /**
   * The cards and pages of the workspace the reader has a role on (an open
   * board not joined is not searched) whose title and body hold every word;
   * the best first. Templates are not found.
   */
  find(workspaceId: string, ask: SearchAsk): Promise<SearchHit[]>;
}

/** Where a backend searches the bodies of its cards and pages. */
export interface ContentSearchOfBackend {
  readonly search: ContentSearch;
}
