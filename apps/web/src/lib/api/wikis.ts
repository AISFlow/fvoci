/**
 * The wikis and their pages on the server (`Backend.wiki`): the operations
 * under `/w/{workspaceId}/wikis` (`getWikis`, `createWiki`, `getPageTree`,
 * `createPage`, `getPage`, `patchPage`, `deletePage`, `movePage`, `duplicatePage`, `getDeletedPages`,
 * `restorePage`), the comments on a page (`getPageComments` and the rest)
 * and the versions of its body (`ServerPageVersions`). What a wiki shares
 * with a board (reading it, changing, deleting and bringing it back, its
 * members, joining it, the sidebar) goes by the board's operations.
 */
import type { DocNode } from '../editor/schema.ts';
import type { Container } from '../logic/wire.ts';
import {
  BackendError,
  type BodyWrite,
  type Comment,
  type CommentVersion,
} from '../state/backend.ts';
import type { WithReactions } from '../state/reaction-types.ts';
import type { DeletedItem, Paging } from '../state/trash-reads.ts';
import { BlockTurns } from '../state/write.ts';
import type {
  PageDraft,
  PageMove,
  PagePatch,
  WikiDraft,
  WikiPage,
  WikiPageDetail,
  Wikis,
} from '../state/wiki-types.ts';

import { ServerPageVersions } from './body-versions.ts';
import {
  bodyOf,
  containerOf,
  commentOf,
  commentVersionOf,
  wireBody,
  type Schemas,
} from './convert.ts';
import type { RequestOptions } from './http.ts';
import { withReactions } from './reactions.ts';
import type { WorkspacePath } from './requests.ts';

/**
 * A request under the workspace of the wiki `params.wikiId` names (the one it
 * was read in), or under `workspaceId`, or the one the reader is in.
 */
export type WikiRequest = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options?: RequestOptions,
  workspaceId?: string,
) => Promise<T>;

/** A page of the tree as the screens have it; `wikiId` is the wiki it was read from. */
export function pageOf(node: Schemas['PageNode'], wikiId: string): WikiPage {
  return {
    id: node.id,
    wikiId,
    parentId: node.parentId,
    title: node.title,
    icon: node.icon,
    order: node.order,
    createdAt: node.createdAt,
    updatedAt: node.updatedAt,
  };
}

export function pageDetailOf(page: Schemas['Page']): WikiPageDetail {
  return {
    ...pageOf(page, page.wikiId),
    body: bodyOf(page.body),
    createdBy: page.createdBy,
    modifiedBy: page.modifiedBy,
  };
}

/**
 * A page as a change message brings it: a block of the type `page`, whose
 * icon and place are among its `fields`. A page at the top of its wiki may
 * name the wiki as its parent.
 */
export function pageOfBlock(block: Schemas['Block']): WikiPage {
  const fields = block.fields as { icon?: unknown; order?: unknown };
  return pageOf(
    {
      id: block.id,
      parentId: block.parentId,
      title: block.title,
      icon: typeof fields.icon === 'string' ? fields.icon : '',
      order: typeof fields.order === 'string' ? fields.order : '',
      createdAt: block.createdAt,
      updatedAt: block.updatedAt,
    },
    block.containerId,
  );
}

/** A page in the trash as the screens list it: its title, who deleted it and when, and how many went with it. */
function deletedPageOf(page: Schemas['DeletedPage']): DeletedItem {
  return {
    id: page.id,
    title: page.title,
    deletedAt: page.deletedAt,
    deletedBy: page.deletedBy,
    below: page.below,
  };
}

function newWikiOf(draft: WikiDraft): Schemas['NewWiki'] {
  return {
    title: draft.title,
    visibility: draft.visibility,
    ...(draft.description ? { description: draft.description } : {}),
    ...(draft.icon ? { icon: draft.icon } : {}),
    ...(draft.minimumRole ? { minimumRole: draft.minimumRole } : {}),
  };
}

function pagePatchOf(patch: PagePatch): Schemas['PagePatch'] {
  const body = patch.body === undefined ? {} : { body: patch.body && wireBody(patch.body) };
  return {
    ...(patch.title === undefined ? {} : { title: patch.title }),
    ...(patch.icon === undefined ? {} : { icon: patch.icon }),
    ...body,
    ...(patch.readAt === undefined ? {} : { readAt: patch.readAt }),
  };
}

export class ServerWikis implements Wikis {
  readonly #request: WikiRequest;
  /** Keeps the workspace of each wiki read, for the paths under it. */
  readonly #learn: (wikis: readonly Schemas['Container'][]) => void;
  readonly #turns = new BlockTurns();

  readonly bodyVersions: ServerPageVersions<WikiPageDetail>;

  constructor(request: WikiRequest, learn: (wikis: readonly Schemas['Container'][]) => void) {
    this.#request = request;
    this.#learn = learn;
    this.bodyVersions = new ServerPageVersions(request, pageDetailOf);
  }

  async wikis(workspaceId: string): Promise<Container[]> {
    const wikis = await this.#request<Schemas['Container'][]>('/wikis', {}, {}, workspaceId);
    this.#learn(wikis);
    return wikis.map(containerOf);
  }

  async createWiki(workspaceId: string, draft: WikiDraft): Promise<Container> {
    const wiki = await this.#request<Schemas['Container']>(
      '/wikis',
      {},
      { method: 'POST', json: newWikiOf(draft) },
      workspaceId,
    );
    this.#learn([wiki]);
    return containerOf(wiki);
  }

  async pages(wikiId: string): Promise<WikiPage[]> {
    const nodes = await this.#request<Schemas['PageNode'][]>('/wikis/{wikiId}/pages', { wikiId });
    return nodes.map((node) => pageOf(node, wikiId));
  }

  async page(wikiId: string, pageId: string): Promise<WithReactions<WikiPageDetail>> {
    const page = await this.#request<Schemas['PageWithReactions']>(
      '/wikis/{wikiId}/pages/{pageId}',
      { wikiId, pageId },
    );
    return withReactions(pageDetailOf(page), page);
  }

  async createPage(wikiId: string, draft: PageDraft): Promise<WikiPageDetail> {
    const request: Schemas['NewPage'] = {
      ...(draft.title ? { title: draft.title } : {}),
      ...(draft.icon ? { icon: draft.icon } : {}),
      ...(draft.parentId ? { parentId: draft.parentId } : {}),
      ...(draft.after === undefined ? {} : { after: draft.after }),
    };
    return pageDetailOf(
      await this.#request<Schemas['Page']>(
        '/wikis/{wikiId}/pages',
        { wikiId },
        { method: 'POST', json: request },
      ),
    );
  }

  patchPage(wikiId: string, pageId: string, patch: PagePatch): Promise<WikiPageDetail> {
    return this.#turns.run(pageId, async () =>
      pageDetailOf(
        await this.#request<Schemas['Page']>(
          '/wikis/{wikiId}/pages/{pageId}',
          { wikiId, pageId },
          { method: 'PATCH', json: pagePatchOf(patch) },
        ),
      ),
    );
  }

  async deletePage(wikiId: string, pageId: string): Promise<void> {
    await this.#request('/wikis/{wikiId}/pages/{pageId}', { wikiId, pageId }, { method: 'DELETE' });
  }

  async movePage(wikiId: string, pageId: string, to: PageMove): Promise<WikiPageDetail> {
    const request: Schemas['PageMove'] = {
      ...(to.parentId ? { parentId: to.parentId } : {}),
      ...(to.after === undefined ? {} : { after: to.after }),
    };
    return pageDetailOf(
      await this.#request<Schemas['Page']>(
        '/wikis/{wikiId}/pages/{pageId}/move',
        { wikiId, pageId },
        { method: 'POST', json: request },
      ),
    );
  }

  async duplicatePage(wikiId: string, pageId: string, title?: string): Promise<WikiPageDetail> {
    const request: Schemas['PageDuplicate'] = title === undefined ? {} : { title };
    return pageDetailOf(
      await this.#request<Schemas['Page']>(
        '/wikis/{wikiId}/pages/{pageId}/duplicate',
        { wikiId, pageId },
        { method: 'POST', json: request },
      ),
    );
  }

  async deletedPages(wikiId: string, paging?: Paging): Promise<DeletedItem[]> {
    const pages = await this.#request<Schemas['DeletedPage'][]>(
      '/wikis/{wikiId}/trash/pages',
      { wikiId },
      { query: { ...paging } },
    );
    return pages.map(deletedPageOf);
  }

  async restorePage(wikiId: string, pageId: string): Promise<WikiPageDetail> {
    return pageDetailOf(
      await this.#request<Schemas['Page']>(
        '/wikis/{wikiId}/pages/{pageId}/restore',
        { wikiId, pageId },
        { method: 'POST' },
      ),
    );
  }

  async comments(wikiId: string, pageId: string): Promise<WithReactions<Comment>[]> {
    const blocks = await this.#request<Schemas['CommentWithReactions'][]>(
      '/wikis/{wikiId}/pages/{pageId}/comments',
      { wikiId, pageId },
    );
    return blocks.map((block) => withReactions(commentOf(block), block));
  }

  async createComment(wikiId: string, pageId: string, body: DocNode): Promise<Comment> {
    const request: Schemas['NewPageComment'] = { body: wireBody(body) };
    return commentOf(
      await this.#request<Schemas['Block']>(
        '/wikis/{wikiId}/pages/{pageId}/comments',
        { wikiId, pageId },
        { method: 'POST', json: request },
      ),
    );
  }

  patchComment(
    wikiId: string,
    pageId: string,
    commentId: string,
    write: BodyWrite,
  ): Promise<Comment> {
    if (!write.body) {
      return Promise.reject(new BackendError(400, { detail: 'a comment has a text' }));
    }
    const request: Schemas['PageCommentPatch'] = {
      body: wireBody(write.body),
      readAt: write.readAt,
    };
    return this.#turns.run(commentId, async () =>
      commentOf(
        await this.#request<Schemas['Block']>(
          '/wikis/{wikiId}/pages/{pageId}/comments/{commentId}',
          { wikiId, pageId, commentId },
          { method: 'PATCH', json: request },
        ),
      ),
    );
  }

  async deleteComment(wikiId: string, pageId: string, commentId: string): Promise<void> {
    await this.#request(
      '/wikis/{wikiId}/pages/{pageId}/comments/{commentId}',
      { wikiId, pageId, commentId },
      { method: 'DELETE' },
    );
  }

  async restoreComment(wikiId: string, pageId: string, commentId: string): Promise<Comment> {
    return commentOf(
      await this.#request<Schemas['Block']>(
        '/wikis/{wikiId}/pages/{pageId}/comments/{commentId}/restore',
        { wikiId, pageId, commentId },
        { method: 'POST' },
      ),
    );
  }

  async commentVersions(
    wikiId: string,
    pageId: string,
    commentId: string,
  ): Promise<CommentVersion[]> {
    const versions = await this.#request<Schemas['CommentVersion'][]>(
      '/wikis/{wikiId}/pages/{pageId}/comments/{commentId}/versions',
      { wikiId, pageId, commentId },
    );
    return versions.map(commentVersionOf);
  }
}
