/**
 * The versions of a card's body on the server (`getCardBodyVersions`,
 * `getCardBodyVersion`, `restoreCardBodyVersion`, `keepCardBodyVersion`).
 * The card restored comes to the board as any change does, through the
 * socket: the answer's copy of it is not used.
 */
import type {
  BodyVersion,
  BodyVersionContent,
  BodyVersions,
  KeptBodyVersion,
} from '../state/body-versions.ts';

import { bodyOf, cardDetailOf, type Schemas } from './convert.ts';
import type { RequestOptions } from './http.ts';
import type { WorkspacePath } from './requests.ts';

/** A request under the workspace of the board that `params.boardId` names. */
type Ask = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options?: RequestOptions,
) => Promise<T>;

export class ServerBodyVersions implements BodyVersions {
  readonly #ask: Ask;

  constructor(ask: Ask) {
    this.#ask = ask;
  }

  list(boardId: string, cardId: string): Promise<BodyVersion[]> {
    return this.#ask<Schemas['BodyVersion'][]>('/cards/{cardId}/body-versions', {
      cardId,
      boardId,
    });
  }

  async read(boardId: string, cardId: string, version: number): Promise<BodyVersionContent> {
    const kept = await this.#ask<Schemas['BodyVersionContent']>(
      '/cards/{cardId}/body-versions/{version}',
      { cardId, boardId, version: String(version) },
    );
    return { ...kept, body: bodyOf(kept.body) };
  }

  async restore(boardId: string, cardId: string, version: number): Promise<unknown> {
    return cardDetailOf(
      await this.#ask<Schemas['Card']>(
        '/cards/{cardId}/body-versions/{version}/restore',
        { cardId, boardId, version: String(version) },
        { method: 'POST' },
      ),
    );
  }

  keep(boardId: string, cardId: string): Promise<KeptBodyVersion> {
    return this.#ask<Schemas['KeptBodyVersion']>(
      '/cards/{cardId}/body-versions',
      { cardId, boardId },
      { method: 'POST' },
    );
  }
}

/**
 * The versions of a wiki page's body on the server (`getPageBodyVersions`,
 * `getPageBodyVersion`, `restorePageBodyVersion`, `keepPageBodyVersion`),
 * as a card's: the wiki is
 * named where a card's board is, and the page where the card is. The page
 * restored comes to the wiki through the socket too.
 */
export class ServerPageVersions<T = unknown> implements BodyVersions {
  readonly #ask: Ask;
  readonly #map: ((page: Schemas['Page']) => T) | undefined;

  constructor(ask: Ask, map?: (page: Schemas['Page']) => T) {
    this.#ask = ask;
    this.#map = map;
  }

  list(wikiId: string, pageId: string): Promise<BodyVersion[]> {
    return this.#ask<Schemas['BodyVersion'][]>('/wikis/{wikiId}/pages/{pageId}/body-versions', {
      wikiId,
      pageId,
    });
  }

  async read(wikiId: string, pageId: string, version: number): Promise<BodyVersionContent> {
    const kept = await this.#ask<Schemas['BodyVersionContent']>(
      '/wikis/{wikiId}/pages/{pageId}/body-versions/{version}',
      { wikiId, pageId, version: String(version) },
    );
    return { ...kept, body: bodyOf(kept.body) };
  }

  async restore(wikiId: string, pageId: string, version: number): Promise<T | undefined> {
    const page = await this.#ask<Schemas['Page']>(
      '/wikis/{wikiId}/pages/{pageId}/body-versions/{version}/restore',
      { wikiId, pageId, version: String(version) },
      { method: 'POST' },
    );
    return this.#map?.(page);
  }

  keep(wikiId: string, pageId: string): Promise<KeptBodyVersion> {
    return this.#ask<Schemas['KeptBodyVersion']>(
      '/wikis/{wikiId}/pages/{pageId}/body-versions',
      { wikiId, pageId },
      { method: 'POST' },
    );
  }
}

/**
 * The versions of a course unit's body on the server (`getUnitBodyVersions`,
 * `getUnitBodyVersion`, `restoreUnitBodyVersion`, `keepUnitBodyVersion`).
 */
export class ServerUnitVersions<T = unknown> implements BodyVersions {
  readonly #ask: Ask;
  readonly #map: ((unit: Schemas['Unit']) => T) | undefined;

  constructor(ask: Ask, map?: (unit: Schemas['Unit']) => T) {
    this.#ask = ask;
    this.#map = map;
  }

  list(courseId: string, unitId: string): Promise<BodyVersion[]> {
    return this.#ask<Schemas['BodyVersion'][]>('/courses/{courseId}/units/{unitId}/body-versions', {
      courseId,
      unitId,
    });
  }

  async read(courseId: string, unitId: string, version: number): Promise<BodyVersionContent> {
    const kept = await this.#ask<Schemas['BodyVersionContent']>(
      '/courses/{courseId}/units/{unitId}/body-versions/{version}',
      { courseId, unitId, version: String(version) },
    );
    return { ...kept, body: bodyOf(kept.body) };
  }

  async restore(courseId: string, unitId: string, version: number): Promise<T | undefined> {
    const unit = await this.#ask<Schemas['Unit']>(
      '/courses/{courseId}/units/{unitId}/body-versions/{version}/restore',
      { courseId, unitId, version: String(version) },
      { method: 'POST' },
    );
    return this.#map?.(unit);
  }

  keep(courseId: string, unitId: string): Promise<KeptBodyVersion> {
    return this.#ask<Schemas['KeptBodyVersion']>(
      '/courses/{courseId}/units/{unitId}/body-versions',
      { courseId, unitId },
      { method: 'POST' },
    );
  }
}

/**
 * The versions of a diary entry's body on the server (`getDiaryEntryBodyVersions`,
 * `getDiaryEntryBodyVersion`, `restoreDiaryEntryBodyVersion`,
 * `keepDiaryEntryBodyVersion`).
 */
export class ServerEntryVersions<T = unknown> implements BodyVersions {
  readonly #ask: Ask;
  readonly #map: ((entry: Schemas['DiaryEntry']) => T) | undefined;

  constructor(ask: Ask, map?: (entry: Schemas['DiaryEntry']) => T) {
    this.#ask = ask;
    this.#map = map;
  }

  list(diaryId: string, entryId: string): Promise<BodyVersion[]> {
    return this.#ask<Schemas['BodyVersion'][]>(
      '/diaries/{diaryId}/entries/{entryId}/body-versions',
      { diaryId, entryId },
    );
  }

  async read(diaryId: string, entryId: string, version: number): Promise<BodyVersionContent> {
    const kept = await this.#ask<Schemas['BodyVersionContent']>(
      '/diaries/{diaryId}/entries/{entryId}/body-versions/{version}',
      { diaryId, entryId, version: String(version) },
    );
    return { ...kept, body: bodyOf(kept.body) };
  }

  async restore(diaryId: string, entryId: string, version: number): Promise<T | undefined> {
    const entry = await this.#ask<Schemas['DiaryEntry']>(
      '/diaries/{diaryId}/entries/{entryId}/body-versions/{version}/restore',
      { diaryId, entryId, version: String(version) },
      { method: 'POST' },
    );
    return this.#map?.(entry);
  }

  keep(diaryId: string, entryId: string): Promise<KeptBodyVersion> {
    return this.#ask<Schemas['KeptBodyVersion']>(
      '/diaries/{diaryId}/entries/{entryId}/body-versions',
      { diaryId, entryId },
      { method: 'POST' },
    );
  }
}
