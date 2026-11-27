/**
 * The search of the bodies of cards and pages, for the backend in memory: as
 * the server answers `searchWorkspaceContent`, and as its search server.
 *
 * A word matches where its letters stand in the title or the body as they
 * are, whatever the case, as the tests' fake search server matches (not by
 * the words of a language, as the real one does). Found are the cards and
 * pages of the boards and wikis the reader reads (a role on it, or an open
 * wiki of the workspace they work in, as on the server), of no template,
 * and no card that is a template of its board. The snippet is one piece of
 * the body around the first word it holds, about as long as the server's,
 * every word in it marked; of a body that holds none, its beginning.
 *
 * A body is read as its room has it, written into the card first
 * (`bodyFlushed`): the words just typed are found at once. The server finds
 * them once it has written the body and sent it to its search server.
 */
import { plainText } from '../../editor/body-text.ts';
import { hashCardNumberOf } from '../../logic/card-numbers.ts';
import { BackendError, problemOf } from '../backend.ts';
import {
  MAX_SEARCH_CHARS,
  type ContentSearch,
  type SearchAsk,
  type SearchHit,
  type SearchKind,
  type SnippetPart,
} from '../content-search.ts';

import { reads } from './container-access.ts';
import { checkWorkspace } from './spaces.ts';
import type { StoredBlock, World } from './world.ts';

/** About how many characters of a body a snippet shows, as the server's. */
export const SNIPPET_CHARS = 160;
/** How many characters of a snippet come before the first word found. */
const BEFORE_MATCH = 40;
const DEFAULT_LIMIT = 20;
const MAX_LIMIT = 50;

/** The kinds a block of a type is found as. */
const KIND_OF_TYPE: Readonly<Record<string, SearchKind>> = {
  card: 'card',
  page: 'page',
  entry: 'entry',
  unit: 'unit',
};

/** The 400 of a parameter of the query out of bounds. */
function refusedParameter(parameter: string, detail: string): never {
  throw new BackendError(400, {
    ...problemOf(400, detail),
    errors: [{ parameter, detail, reason: 'invalid' }],
  });
}

function escaped(word: string): string {
  return word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** What finds any of the words, whatever the case; the longest first, so that a word holding another is found whole. */
function patternOf(words: readonly string[]): RegExp {
  const longestFirst = [...words].sort((first, second) => second.length - first.length);
  return new RegExp(longestFirst.map(escaped).join('|'), 'giu');
}

/** The pieces of `text`, the stretches that are one of the words marked. */
export function markedParts(text: string, words: readonly string[]): SnippetPart[] {
  if (text === '') return [];
  if (words.length === 0) return [{ text, matched: false }];
  const parts: SnippetPart[] = [];
  let at = 0;
  for (const match of text.matchAll(patternOf(words))) {
    if (match.index > at) parts.push({ text: text.slice(at, match.index), matched: false });
    parts.push({ text: match[0], matched: true });
    at = match.index + match[0].length;
  }
  if (at < text.length) parts.push({ text: text.slice(at), matched: false });
  return parts;
}

/** The snippet of a body for the words: around the first one it holds, else its beginning. */
export function snippetOf(text: string, words: readonly string[]): SnippetPart[] {
  const chars = Array.from(text);
  const found = words.length === 0 ? null : patternOf(words).exec(text);
  if (found === null) return markedParts(chars.slice(0, SNIPPET_CHARS).join(''), []);
  // In characters, not code units: a piece never cuts a character in two.
  const first = Array.from(text.slice(0, found.index)).length;
  const start = Math.max(0, Math.min(first - BEFORE_MATCH, chars.length - SNIPPET_CHARS));
  return markedParts(chars.slice(start, start + SNIPPET_CHARS).join(''), words);
}

export class MemoryContentSearch implements ContentSearch {
  readonly #world: World;
  readonly #userId: string;
  readonly #enter: () => Promise<void>;

  /** `enter` is the wait every call of the backend starts with, and the failure a test asked for. */
  constructor(world: World, userId: string, enter: () => Promise<void>) {
    this.#world = world;
    this.#userId = userId;
    this.#enter = enter;
  }

  async enabled(): Promise<boolean> {
    await this.#enter();
    return this.#world.searchEnabled;
  }

  async find(workspaceId: string, ask: SearchAsk): Promise<SearchHit[]> {
    await this.#enter();
    const world = this.#world;
    checkWorkspace(world, this.#userId, workspaceId);
    const query = ask.query.trim();
    if (Array.from(query).length > MAX_SEARCH_CHARS)
      refusedParameter('q', 'the words are too long');
    const limit = ask.limit ?? DEFAULT_LIMIT;
    if (!Number.isInteger(limit) || limit < 1 || limit > MAX_LIMIT) {
      refusedParameter('limit', 'the number of hits is from 1 to 50');
    }
    const cardNumber =
      ask.kind === undefined || ask.kind === 'card' ? hashCardNumberOf(query) : undefined;
    if (cardNumber !== undefined) {
      const hits: { hit: SearchHit; updatedAt: number }[] = [];
      for (const board of world.containers.values()) {
        if (board.workspaceId !== workspaceId || board.isTemplate) continue;
        if (!reads(world, this.#userId, board.id)) continue;
        for (const block of world.blocksOf(board.id)) {
          if (this.#kindOf(block) !== 'card' || block.number !== cardNumber) continue;
          hits.push({
            hit: {
              id: block.id,
              kind: 'card',
              containerId: board.id,
              containerKind: board.kind ?? 'board',
              containerTitle: board.title,
              title: block.title,
              number: cardNumber,
              snippet: [],
            },
            updatedAt: block.updatedAt,
          });
        }
      }
      return hits
        .sort(
          (first, second) =>
            second.updatedAt - first.updatedAt || first.hit.id.localeCompare(second.hit.id),
        )
        .slice(0, limit)
        .map(({ hit }) => hit);
    }

    const words = query.split(/\s+/u).filter((word) => word !== '');
    if (words.length === 0) return [];
    const holds = (text: string, word: string): boolean =>
      new RegExp(escaped(word), 'iu').test(text);

    const hits: { hit: SearchHit; inTitle: number; updatedAt: number }[] = [];
    for (const board of world.containers.values()) {
      if (board.workspaceId !== workspaceId || board.isTemplate) continue;
      if (!reads(world, this.#userId, board.id)) continue;
      for (const block of world.blocksOf(board.id)) {
        const kind = this.#kindOf(block);
        if (kind === undefined || (ask.kind !== undefined && ask.kind !== kind)) continue;
        world.bodyFlushed?.(block.id);
        const text = block.body ? plainText(block.body) : '';
        if (!words.every((word) => holds(`${block.title}\n${text}`, word))) continue;
        hits.push({
          hit: {
            id: block.id,
            kind,
            containerId: board.id,
            containerKind: board.kind ?? 'board',
            containerTitle: board.title,
            title: block.title,
            ...(kind === 'card' && block.number !== undefined ? { number: block.number } : {}),
            snippet: snippetOf(text, words),
          },
          inTitle: words.filter((word) => holds(block.title, word)).length,
          updatedAt: block.updatedAt,
        });
      }
    }
    return hits
      .sort((first, second) => second.inTitle - first.inTitle || second.updatedAt - first.updatedAt)
      .slice(0, limit)
      .map(({ hit }) => hit);
  }

  /** What a block is found as: none for what is no card or page, or a card that is a template. */
  #kindOf(block: StoredBlock): SearchKind | undefined {
    const kind = KIND_OF_TYPE[block.type];
    if (kind === 'card' && block.fields.isTemplate === true) return undefined;
    return kind;
  }
}
