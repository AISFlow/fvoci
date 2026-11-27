/**
 * References on the server (`resolveReferences`, `getReferencesTo`,
 * `searchReferences`, `getCardByNumber`): what the reader may see of what
 * bodies name, of what names a block, and of what there is to name.
 */
import { isBackendError } from '../state/problems.ts';
import { REFERENCES_AT_ONCE, type LinkedBlock, type References } from '../state/references.ts';
import type { Paging } from '../state/trash-reads.ts';

import type { Schemas } from './convert.ts';
import type { RequestOptions } from './http.ts';
import type { WorkspacePath } from './requests.ts';

/** A card or a page as the server tells it, without the number it leaves out or empty. */
function linkedOf({ number, ...rest }: Schemas['LinkedBlock']): LinkedBlock {
  return number === undefined || number === null ? rest : { ...rest, number };
}

/** A request under the workspace `workspaceId`. */
type Ask = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options: RequestOptions,
  workspaceId: string,
) => Promise<T>;

export class ServerReferences implements References {
  readonly #ask: Ask;

  constructor(ask: Ask) {
    this.#ask = ask;
  }

  /** In parts of as many as the server takes at once; the parts are asked together. */
  async resolve(workspaceId: string, ids: readonly string[]): Promise<LinkedBlock[]> {
    const asked = [...new Set(ids)];
    const parts: string[][] = [];
    for (let start = 0; start < asked.length; start += REFERENCES_AT_ONCE) {
      parts.push(asked.slice(start, start + REFERENCES_AT_ONCE));
    }
    const answers = await Promise.all(
      parts.map((part) =>
        this.#ask<Schemas['LinkedBlock'][]>(
          '/references/resolve',
          {},
          { method: 'POST', json: part },
          workspaceId,
        ),
      ),
    );
    return answers.flat().map(linkedOf);
  }

  async to(workspaceId: string, blockId: string, paging?: Paging): Promise<LinkedBlock[]> {
    const query = paging ? { page: paging.page, perPage: paging.perPage } : {};
    const blocks = await this.#ask<Schemas['LinkedBlock'][]>(
      '/references/to/{blockId}',
      { blockId },
      { query },
      workspaceId,
    );
    return blocks.map(linkedOf);
  }

  async search(workspaceId: string, term: string): Promise<LinkedBlock[]> {
    const blocks = await this.#ask<Schemas['LinkedBlock'][]>(
      '/references/search',
      {},
      { query: { q: term } },
      workspaceId,
    );
    return blocks.map(linkedOf);
  }

  /** One 404 for every reason there is none (the server's). */
  async numbered(
    workspaceId: string,
    boardId: string,
    number: number,
  ): Promise<LinkedBlock | undefined> {
    try {
      const card = await this.#ask<Schemas['Card']>(
        '/boards/{boardId}/cards/by-number/{number}',
        { boardId, number: String(number) },
        {},
        workspaceId,
      );
      const { id, title, icon, boardId: containerId } = card;
      const numbered = card.number === null ? {} : { number: card.number };
      return { id, kind: 'card', title, icon, containerId, containerKind: 'board', ...numbered };
    } catch (error) {
      if (isBackendError(error) && error.status === 404) return undefined;
      throw error;
    }
  }
}
