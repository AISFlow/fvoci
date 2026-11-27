/** Acknowledged room updates must be present in the card copied by the UI. */
import { expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import { bodyOf } from '$lib/state/memory/rooms.ts';

import { openFirstBoard } from './support.ts';

it('duplicating immediately after settleBody copies the acknowledged room body', async () => {
  const { board, backend } = await openFirstBoard();
  try {
    const summary = board.cards.find((card) => card.title === 'Alpha');
    if (!summary) throw new Error('no card');
    const card = await board.showCard(summary.id);
    card.openBody();
    await vi.waitFor(() => {
      expect(card.room?.status).toBe('joined');
    });
    const shared = card.room?.doc;
    if (!shared) throw new Error('no shared document');
    const paragraph = new Y.XmlElement('paragraph');
    paragraph.insert(0, [new Y.XmlText('acknowledged just now')]);
    const fragment = shared.getXmlFragment(BODY_FIELD);
    fragment.insert(fragment.length, [paragraph]);

    // The same sequence as CardContent.duplicate, using the real memory backend.
    await card.settleBody();
    expect(card.bodyPending).toBe(false);
    const expected = bodyOf(shared);
    const copy = await board.duplicateCard(card.cardId);
    if (!copy) throw new Error('no copy');
    expect((await backend.card(board.boardId, copy.id)).body).toEqual(expected);
  } finally {
    board.close();
  }
});
