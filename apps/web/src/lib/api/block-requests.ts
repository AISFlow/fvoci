/**
 * View, card, comment and attachment HTTP operations for `ServerBackend`
 * (`api/backend.ts`), using server-authority fractional-indexed view writes,
 * atomic `duplicateCard` / `card-moves` / `batch-delete` / `batch-restore`,
 * formal card comment routes, and verified card attachments.
 */
import type { CardPreview } from '../editor/card-preview.ts';
import type { DocNode } from '../editor/schema.ts';
import { createBoardView as emptyViewOf, type BlockPatch, type BoardView } from '../logic/wire.ts';
import {
  BackendError,
  isViewBlock,
  refusedMember,
  type Attachment,
  type BoardViewWriteResult,
  type BodyWrite,
  type CardDetail,
  type CardDraft,
  type CardMovesInput,
  type CardMovesOutput,
  type CardPatch,
  type CardSummary,
  type CardTelling,
  type Change,
  type Comment,
  type CommentVersion,
  type UploadedFile,
  type ViewDraft,
} from '../state/backend.ts';
import { pagesTogether } from '../state/page-reads.ts';
import type { WithReactions } from '../state/reaction-types.ts';
import type { BlockTurns } from '../state/write.ts';

import {
  attachmentOf,
  cardDetailOf,
  cardOf,
  commentOf,
  commentVersionOf,
  newCardRequest,
  patchCardRequest,
  previewsOf,
  viewOf,
  wireBody,
  type Schemas,
} from './convert.ts';
import type { RequestOptions } from './http.ts';
import { withReactions } from './reactions.ts';
import type { WorkspacePath } from './requests.ts';

type ChangedBlock = Extract<Change, { kind: 'blocks' }>['blocks'][number];

export type InWorkspaceFn = <T>(
  path: WorkspacePath,
  params?: Record<string, string>,
  options?: RequestOptions,
  workspaceId?: string,
) => Promise<T>;

export class ServerBlocks {
  readonly #inWorkspace: InWorkspaceFn;
  readonly #turns: BlockTurns;
  readonly #knownViews = new Map<string, BoardView>();

  constructor(inWorkspace: InWorkspaceFn, turns: BlockTurns) {
    this.#inWorkspace = inWorkspace;
    this.#turns = turns;
  }

  learnView(view: BoardView): void {
    this.#knownViews.set(view.id, view);
  }

  learnLiveBlock(block: ChangedBlock): void {
    if (isViewBlock(block)) this.#knownViews.set(block.id, block);
  }

  #takeViewWrite(written: Schemas['BoardViewWrite']): BoardViewWriteResult {
    const view = viewOf(written.view);
    const companionViews = written.companionViews.map(viewOf);
    this.#knownViews.set(view.id, view);
    for (const companion of companionViews) this.#knownViews.set(companion.id, companion);
    return { view, companionViews };
  }

  async views(boardId: string): Promise<BoardView[]> {
    const blocks = await this.#inWorkspace<Schemas['Block'][]>('/boards/{boardId}/views', {
      boardId,
    });
    const views = blocks.map(viewOf);
    for (const view of views) this.#knownViews.set(view.id, view);
    return views;
  }

  async writeView(boardId: string, draft: ViewDraft): Promise<BoardViewWriteResult> {
    const request: Schemas['NewBoardView'] = {
      title: draft.title,
      viewType: draft.fields.viewType,
      fields: draft.fields,
      ...(draft.after === undefined ? {} : { after: draft.after }),
    };
    const written = await this.#inWorkspace<Schemas['BoardViewWrite']>(
      '/boards/{boardId}/views',
      { boardId },
      { method: 'POST', json: request },
    );
    return this.#takeViewWrite(written);
  }

  async createView(boardId: string, draft: ViewDraft): Promise<BoardView> {
    return (await this.writeView(boardId, draft)).view;
  }

  async moveView(
    boardId: string,
    viewId: string,
    after: string | null,
  ): Promise<BoardViewWriteResult> {
    const request: Schemas['BoardViewMove'] = { after };
    const written = await this.#inWorkspace<Schemas['BoardViewWrite']>(
      '/boards/{boardId}/views/{viewId}/move',
      { boardId, viewId },
      { method: 'POST', json: request },
    );
    return this.#takeViewWrite(written);
  }

  async duplicateView(
    boardId: string,
    viewId: string,
    title?: string,
    after?: string | null,
  ): Promise<BoardViewWriteResult> {
    const request: Schemas['BoardViewDuplicate'] = {
      ...(title === undefined ? {} : { title }),
      ...(after === undefined ? {} : { after }),
    };
    const written = await this.#inWorkspace<Schemas['BoardViewWrite']>(
      '/boards/{boardId}/views/{viewId}/duplicate',
      { boardId, viewId },
      { method: 'POST', json: request },
    );
    return this.#takeViewWrite(written);
  }

  patchView(boardId: string, viewId: string, patch: BlockPatch): Promise<BoardView> {
    return this.#turns.patchView(viewId, patch, (sent, conditionalQueued) =>
      this.#patchView(boardId, viewId, sent, conditionalQueued),
    );
  }

  async #patchView(
    boardId: string,
    viewId: string,
    patch: BlockPatch,
    conditionalQueued: boolean,
  ): Promise<{ before: BoardView; after: BoardView }> {
    const known =
      conditionalQueued && patch.readAt === undefined
        ? await this.#view(boardId, viewId)
        : (this.#knownViews.get(viewId) ?? {
            ...emptyViewOf(),
            id: viewId,
            containerId: boardId,
            updatedAt: patch.readAt ?? 0,
          });
    const answered = await this.#inWorkspace<Schemas['Block']>(
      '/boards/{boardId}/views/{viewId}',
      { boardId, viewId },
      { method: 'PATCH', json: boardViewPatchOf(patch) },
    );
    const after = viewOf(answered);
    this.#knownViews.set(viewId, after);
    return { before: known, after };
  }

  async #view(boardId: string, viewId: string): Promise<BoardView> {
    return viewOf(
      await this.#inWorkspace<Schemas['Block']>('/boards/{boardId}/views/{viewId}', {
        boardId,
        viewId,
      }),
    );
  }

  async deleteView(boardId: string, viewId: string): Promise<void> {
    await this.#inWorkspace(
      '/boards/{boardId}/views/{viewId}',
      { boardId, viewId },
      { method: 'DELETE' },
    );
  }

  async restoreView(boardId: string, viewId: string): Promise<void> {
    const restored = await this.#inWorkspace<Schemas['Block']>(
      '/boards/{boardId}/views/{viewId}/restore',
      { boardId, viewId },
      { method: 'POST' },
    );
    this.#knownViews.set(viewId, viewOf(restored));
  }

  async cards(boardId: string): Promise<CardSummary[]> {
    return (await this.#everyCard(boardId, {})).map(cardOf);
  }

  async cardPreviews(boardId: string): Promise<Map<string, CardPreview>> {
    return previewsOf(await this.#everyCard(boardId, { preview: 'true' }));
  }

  #everyCard(boardId: string, query: Record<string, string>): Promise<Schemas['Card'][]> {
    return pagesTogether((paging) =>
      this.#inWorkspace<Schemas['Card'][]>(
        '/boards/{boardId}/cards',
        { boardId },
        { query: { ...paging, ...query } },
      ),
    );
  }

  async card(boardId: string, cardId: string): Promise<WithReactions<CardDetail>> {
    const card = await this.#inWorkspace<Schemas['CardWithReactions']>('/cards/{cardId}', {
      cardId,
      boardId,
    });
    if (card.boardId !== boardId) {
      throw new BackendError(404, { detail: 'no such card on this board' });
    }
    return withReactions(cardDetailOf(card), card);
  }

  async createCard(boardId: string, draft: CardDraft): Promise<CardSummary> {
    const card = await this.#inWorkspace<Schemas['Card']>(
      '/boards/{boardId}/cards',
      { boardId },
      newCardRequest(draft),
    );
    return cardOf(card);
  }

  async duplicateCard(boardId: string, cardId: string, asTemplate?: boolean): Promise<CardSummary> {
    const card = await this.#inWorkspace<Schemas['Card']>(
      '/boards/{boardId}/cards/{cardId}/duplicate',
      { boardId, cardId },
      {
        method: 'POST',
        query: { asTemplate: asTemplate === undefined ? undefined : String(asTemplate) },
      },
    );
    return cardOf(card);
  }

  async moveViewCards(
    boardId: string,
    viewId: string,
    move: CardMovesInput,
    telling?: CardTelling,
  ): Promise<CardMovesOutput> {
    const request: Schemas['CardMovesRequest'] = {
      cardIds: [...move.cardIds],
      ...(move.groupPropertyId === undefined ? {} : { groupPropertyId: move.groupPropertyId }),
      ...(move.groupValue === undefined ? {} : { groupValue: move.groupValue }),
      ...(move.afterCardId === undefined ? {} : { afterCardId: move.afterCardId }),
    };
    const result = await this.#inWorkspace<Schemas['CardMovesResult']>(
      '/boards/{boardId}/views/{viewId}/card-moves',
      { boardId, viewId },
      {
        method: 'POST',
        json: request,
        query: { disableNotify: telling?.quiet ? 'true' : undefined },
      },
    );
    const view = viewOf(result.view);
    this.#knownViews.set(view.id, view);
    return { view, cards: result.cards.map(cardOf) };
  }

  async batchDeleteCards(boardId: string, cardIds: readonly string[]): Promise<void> {
    const request: Schemas['CardBatchIds'] = { cardIds: [...cardIds] };
    await this.#inWorkspace(
      '/boards/{boardId}/cards/batch-delete',
      { boardId },
      { method: 'POST', json: request },
    );
  }

  async batchRestoreCards(boardId: string, cardIds: readonly string[]): Promise<CardSummary[]> {
    const request: Schemas['CardBatchIds'] = { cardIds: [...cardIds] };
    const restored = await this.#inWorkspace<Schemas['RestoredCards']>(
      '/boards/{boardId}/cards/batch-restore',
      { boardId },
      { method: 'POST', json: request },
    );
    return restored.cards.map(cardOf);
  }

  patchCard(
    boardId: string,
    cardId: string,
    patch: CardPatch,
    telling?: CardTelling,
  ): Promise<CardSummary> {
    return this.#turns.run(cardId, async () =>
      cardOf(
        await this.#inWorkspace<Schemas['Card']>(
          '/cards/{cardId}',
          { cardId, boardId },
          patchCardRequest(patch, telling),
        ),
      ),
    );
  }

  async comments(boardId: string, cardId: string): Promise<WithReactions<Comment>[]> {
    const blocks = await this.#inWorkspace<Schemas['CommentWithReactions'][]>(
      '/boards/{boardId}/cards/{cardId}/comments',
      { boardId, cardId },
    );
    return blocks.map((block) => withReactions(commentOf(block), block));
  }

  async createComment(boardId: string, cardId: string, body: DocNode): Promise<Comment> {
    const request: Schemas['NewCardComment'] = { body: wireBody(body) };
    const made = await this.#inWorkspace<Schemas['Block']>(
      '/boards/{boardId}/cards/{cardId}/comments',
      { boardId, cardId },
      { method: 'POST', json: request },
    );
    return commentOf(made);
  }

  patchComment(
    boardId: string,
    commentId: string,
    write: BodyWrite,
    cardId: string,
  ): Promise<Comment> {
    const body = write.body;
    if (!body) {
      return Promise.reject(
        new BackendError(400, {
          detail: 'a comment needs a text',
          errors: [refusedMember('body', 'a comment needs a text', 'required')],
        }),
      );
    }
    return this.#turns.run(commentId, async () => {
      const request: Schemas['CardCommentPatch'] = { body: wireBody(body), readAt: write.readAt };
      const updated = await this.#inWorkspace<Schemas['Block']>(
        '/boards/{boardId}/cards/{cardId}/comments/{commentId}',
        { boardId, cardId, commentId },
        { method: 'PATCH', json: request },
      );
      return commentOf(updated);
    });
  }

  async deleteComment(boardId: string, cardId: string, commentId: string): Promise<void> {
    await this.#inWorkspace(
      '/boards/{boardId}/cards/{cardId}/comments/{commentId}',
      { boardId, cardId, commentId },
      { method: 'DELETE' },
    );
  }

  async restoreComment(boardId: string, cardId: string, commentId: string): Promise<Comment> {
    const restored = await this.#inWorkspace<Schemas['Block']>(
      '/boards/{boardId}/cards/{cardId}/comments/{commentId}/restore',
      { boardId, cardId, commentId },
      { method: 'POST' },
    );
    return commentOf(restored);
  }

  async commentVersions(
    boardId: string,
    commentId: string,
    cardId: string,
  ): Promise<CommentVersion[]> {
    const versions = await this.#inWorkspace<Schemas['CommentVersion'][]>(
      '/boards/{boardId}/cards/{cardId}/comments/{commentId}/versions',
      { boardId, cardId, commentId },
    );
    return versions.map(commentVersionOf);
  }

  async attachments(boardId: string, cardId: string): Promise<Attachment[]> {
    const blocks = await this.#inWorkspace<Schemas['Block'][]>(
      '/boards/{boardId}/cards/{cardId}/attachments',
      { boardId, cardId },
    );
    return blocks.map(attachmentOf);
  }

  async createAttachment(boardId: string, cardId: string, file: UploadedFile): Promise<Attachment> {
    // The name it is shown under, which an edit kept apart has of its own.
    const request: Schemas['NewCardAttachment'] = { fileId: file.fileId, title: file.name };
    const made = await this.#inWorkspace<Schemas['Block']>(
      '/boards/{boardId}/cards/{cardId}/attachments',
      { boardId, cardId },
      { method: 'POST', json: request },
    );
    return attachmentOf(made);
  }

  async deleteAttachment(boardId: string, cardId: string, attachmentId: string): Promise<void> {
    await this.#inWorkspace(
      '/boards/{boardId}/cards/{cardId}/attachments/{attachmentId}',
      { boardId, cardId, attachmentId },
      { method: 'DELETE' },
    );
  }

  async restoreAttachment(boardId: string, cardId: string, attachmentId: string): Promise<void> {
    await this.#inWorkspace(
      '/boards/{boardId}/cards/{cardId}/attachments/{attachmentId}/restore',
      { boardId, cardId, attachmentId },
      { method: 'POST' },
    );
  }
}

/**
 * What of a view's patch `patchBoardView` takes: its title and fields, and
 * the time it was read; a view has no parent or type to change.
 */
function boardViewPatchOf(patch: BlockPatch): Schemas['BoardViewPatch'] {
  return {
    ...(patch.title === undefined ? {} : { title: patch.title }),
    ...(patch.updatedFields === undefined ? {} : { updatedFields: patch.updatedFields }),
    ...(patch.deletedFields === undefined ? {} : { deletedFields: [...patch.deletedFields] }),
    ...(patch.readAt === undefined ? {} : { readAt: patch.readAt }),
  };
}
