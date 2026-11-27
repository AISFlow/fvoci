/** The previews a gallery shows: read when it starts, followed by change messages, the same in both backends. */
import { describe, expect, it, vi } from 'vitest';

import { cardOfBlock } from '$lib/api/convert.ts';
import { summaryOfBody } from '$lib/state/body-summary.ts';
import { isCardBlock, type CardSummary } from '$lib/state/backend.ts';
import { CardPreviews } from '$lib/state/index.ts';

import { openFirstBoard, serverBackend, settle, text } from './support.ts';

describe('the previews of a gallery', () => {
  it('are read when it starts, follow a body written elsewhere, and stop with it', async () => {
    const { board, backend, other } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    const asked = vi.spyOn(backend, 'cardPreviews');

    const previews = new CardPreviews(backend, board.boardId);
    expect(asked).not.toHaveBeenCalled();
    const stop = previews.start();
    await settle();
    expect(asked).toHaveBeenCalledOnce();
    expect(previews.of(alpha.id)).toEqual({ imageFileId: '', text: 'first body' });

    const now = await other.card(board.boardId, alpha.id);
    await other.writeCardBody(board.boardId, alpha.id, {
      body: text('written elsewhere'),
      readAt: now.updatedAt,
    });
    await settle();
    expect(previews.of(alpha.id)?.text).toBe('written elsewhere');

    stop();
    const later = await other.card(board.boardId, alpha.id);
    await other.writeCardBody(board.boardId, alpha.id, {
      body: text('not followed'),
      readAt: later.updatedAt,
    });
    await settle();
    expect(previews.of(alpha.id)?.text).toBe('written elsewhere');
  });

  it('keep what a change message told over a reading begun before it', async () => {
    const { board, backend, other } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    // The reading takes its answer before the write, and gives it after the write's message.
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const real = backend.cardPreviews.bind(backend);
    vi.spyOn(backend, 'cardPreviews').mockImplementationOnce(async (boardId) => {
      const answer = await real(boardId);
      await held;
      return answer;
    });
    const previews = new CardPreviews(backend, board.boardId);
    const stop = previews.start();
    await settle();
    const now = await other.card(board.boardId, alpha.id);
    await other.writeCardBody(board.boardId, alpha.id, {
      body: text('newer'),
      readAt: now.updatedAt,
    });
    await settle();
    release();
    await settle();
    expect(previews.of(alpha.id)?.text).toBe('newer');
    stop();
  });
});

describe('the server backend', () => {
  it('asks the card list with previews, and takes a change message’s from its summary', async () => {
    const sent: string[] = [];
    vi.stubGlobal('fetch', (url: string) => {
      sent.push(url);
      return Promise.resolve(
        new Response(
          JSON.stringify([
            { id: 'c1', preview: { imageFileId: 'f1', text: 'Hello' } },
            { id: 'c2', preview: { imageFileId: '', text: '' } },
            { id: 'c3' },
          ]),
          { status: 200, headers: { 'Content-Type': 'application/json' } },
        ),
      );
    });
    const previews = await serverBackend('').cardPreviews('b');
    vi.unstubAllGlobals();
    expect(sent).toEqual(['/api/v1/w/w1/boards/b/cards?page=0&perPage=1000&preview=true']);
    expect([...previews]).toEqual([
      ['c1', { imageFileId: 'f1', text: 'Hello' }],
      ['c2', { imageFileId: '', text: '' }],
    ]);

    const body = {
      type: 'doc',
      content: [
        { type: 'image', attrs: { fileId: 'f9' } },
        { type: 'paragraph', content: [{ type: 'text', text: 'Below it' }] },
      ],
    };
    // The server's message: the block without its body, the summary beside it.
    const summary = {
      hasBody: true,
      tasksTotal: 0,
      tasksDone: 0,
      preview: { imageFileId: 'f9', text: 'Below it' },
    };
    expect(summaryOfBody(body)).toEqual(summary);
    const block = cardOfBlock(
      {
        id: 'c4',
        containerId: 'b',
        parentId: '',
        createdBy: 'u',
        modifiedBy: 'u',
        type: 'card',
        title: 'With a picture',
        fields: { properties: {} },
        createdAt: 1,
        updatedAt: 1,
        body: null,
      },
      summary,
    );
    expect(block).toMatchObject({ id: 'c4', hasBody: true, preview: summary.preview });
  });
});

describe('a card’s change message', () => {
  it('has the shape of the server’s from the memory backend: a summary in place of the body', async () => {
    const { board, backend, other } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    const told: CardSummary[] = [];
    const stop = backend.watchContainer(board.boardId, (change) => {
      if (change.kind === 'blocks') told.push(...change.blocks.filter(isCardBlock));
    });
    const task = (checked: boolean, words: string) => ({
      type: 'taskItem',
      attrs: { checked },
      content: [{ type: 'paragraph', content: [{ type: 'text', text: words }] }],
    });
    const body = {
      type: 'doc',
      content: [
        { type: 'taskList', content: [task(true, 'one'), task(false, 'two')] },
        { type: 'paragraph', content: [{ type: 'text', text: 'notes' }] },
      ],
    };
    const now = await other.card(board.boardId, alpha.id);
    await other.writeCardBody(board.boardId, alpha.id, { body, readAt: now.updatedAt });
    await settle();
    stop();

    const card = told.at(-1);
    expect(card).toMatchObject({ id: alpha.id, hasBody: true, tasksTotal: 2, tasksDone: 1 });
    expect(card?.preview).toEqual(summaryOfBody(body).preview);
    expect(card).not.toHaveProperty('body');
    // The server's message of the same card, as the web reads it.
    const fromServer = cardOfBlock(
      {
        id: alpha.id,
        containerId: board.boardId,
        parentId: '',
        createdBy: 'u',
        modifiedBy: 'u',
        type: 'card',
        title: 'Alpha',
        fields: { properties: {} },
        createdAt: 1,
        updatedAt: 2,
        body: null,
        number: alpha.number,
      },
      summaryOfBody(body),
    );
    expect(Object.keys(card ?? {}).sort()).toEqual(Object.keys(fromServer).sort());
  });
});
