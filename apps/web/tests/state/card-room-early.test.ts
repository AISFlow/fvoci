/**
 * The room of a card's body is asked for as the card is opened, together
 * with the card's reading (`CardStore.load`), and is left again where
 * the card cannot be read: one that is not there, in the trash, or of a
 * board the reader may not see, in whichever order the room and the
 * reading answer. Cards opened and closed fast leave no room behind.
 */
import { describe, expect, it, onTestFinished, vi } from 'vitest';

import { BackendError } from '$lib/state/index.ts';
import type { BodyDoc, BodyDocHears } from '$lib/state/backend.ts';
import { openLater } from '$lib/state/doc-opener.ts';
import { roomsOf } from '$lib/state/memory/rooms.ts';
import { createBoardView } from '$lib/logic/wire.ts';

import { openFirstBoard, settle } from './support.ts';

type Made = Awaited<ReturnType<typeof openFirstBoard>>;

/** A promise and the function that settles it. */
function gate(): { open: () => void; passed: Promise<void> } {
  let open = (): void => undefined;
  const passed = new Promise<void>((resolve) => {
    open = resolve;
  });
  return { open, passed };
}

/** Every body opened by the reader's backend, and whether it was closed; `held` holds each back. */
function watchRooms(made: Made, held?: Promise<void>) {
  const opened: { cardId: string; closed: boolean }[] = [];
  const openDoc = made.backend.openDoc.bind(made.backend);
  const spy = vi
    .spyOn(made.backend, 'openDoc')
    .mockImplementation((workspaceId, boardId, cardId, hears: BodyDocHears): BodyDoc => {
      const one = { cardId, closed: false };
      opened.push(one);
      const doc = held
        ? openLater(async () => {
            await held;
            return (told) => openDoc(workspaceId, boardId, cardId, told);
          }, hears)
        : openDoc(workspaceId, boardId, cardId, hears);
      return {
        get state() {
          return doc.state;
        },
        retry: () => {
          doc.retry();
        },
        settle: () => doc.settle(),
        close: () => {
          one.closed = true;
          doc.close();
        },
      };
    });
  return { opened, spy };
}

/** A card of a private board of the other person, which the reader is not on. */
async function theirCard(made: Made): Promise<string> {
  const { board } = await made.other.createBoard(
    made.world.workspace.id,
    { title: 'Theirs', visibility: 'private' },
    { title: 'Board', fields: createBoardView().fields },
  );
  return (await made.other.createCard(board.id, { title: 'Hidden' })).id;
}

const realCard = (made: Made, title: string): string => {
  const card = made.board.realCards.find((one) => one.title === title);
  if (!card) throw new Error(`no card ${title}`);
  return card.id;
};

describe('the room asked for as a card is opened', () => {
  it('is asked for before the card has been read, and the reader is in it once it has', async () => {
    const made = await openFirstBoard();
    const { opened } = watchRooms(made);
    const cardId = realCard(made, 'Alpha');
    const reading = vi.spyOn(made.backend, 'card');
    const showing = made.board.showCard(cardId);
    // Asked for in the same turn as the reading: neither waits for the other.
    expect(opened.map((one) => one.cardId)).toEqual([cardId]);
    expect(reading).toHaveBeenCalledTimes(1);
    const card = await showing;
    await vi.waitFor(() => {
      expect(card.room?.status).toBe('joined');
    });
    expect(roomsOf(made.world).members(cardId)).toBe(1);
    // The body's own opening (CardBody) asks for no second room.
    card.openBody();
    expect(opened).toHaveLength(1);
    made.board.closeCard();
    expect(opened[0]?.closed).toBe(true);
    await vi.waitFor(() => {
      expect(roomsOf(made.world).members(cardId)).toBe(0);
    });
  });

  for (const [what, cardOf] of [
    ['not there', () => Promise.resolve('nope')],
    [
      'in the trash',
      async (made: Made) => {
        const cardId = realCard(made, 'Alpha');
        await made.other.deleteBlock(made.board.boardId, cardId);
        await settle();
        return cardId;
      },
    ],
    ['of a board the reader is not on', theirCard],
  ] as const) {
    it(`is left for a card ${what}, the reading answering first`, async () => {
      const made = await openFirstBoard();
      const cardId = await cardOf(made);
      const room = gate();
      const { opened } = watchRooms(made, room.passed);
      const reading = vi.spyOn(made.backend, 'card');
      const boardRead = vi.spyOn(made.backend, 'cards');
      const card = await made.board.showCard(cardId);
      expect(card.missing).toBe(true);
      expect(opened).toEqual([{ cardId, closed: true }]);
      expect(card.room).toBeUndefined();
      // The room's code arrives after: nothing is opened with it.
      room.open();
      await settle();
      await settle();
      expect(card.room).toBeUndefined();
      expect(roomsOf(made.world).members(cardId)).toBe(0);
      expect(reading).toHaveBeenCalledTimes(1);
      expect(boardRead).not.toHaveBeenCalled();
    });

    it(`is left for a card ${what}, the room answering first`, async () => {
      const made = await openFirstBoard();
      const cardId = await cardOf(made);
      const { opened } = watchRooms(made);
      // The card's reading (its block, comments and attachments) waits until let go.
      const read = gate();
      const { backend } = made;
      const [readCard, readComments, readAttachments] = [
        backend.card.bind(backend),
        backend.comments.bind(backend),
        backend.attachments.bind(backend),
      ];
      const reading = vi.spyOn(backend, 'card').mockImplementation(async (...asked) => {
        await read.passed;
        return readCard(...asked);
      });
      vi.spyOn(backend, 'comments').mockImplementation(async (...asked) => {
        await read.passed;
        return readComments(...asked);
      });
      vi.spyOn(backend, 'attachments').mockImplementation(async (...asked) => {
        await read.passed;
        return readAttachments(...asked);
      });
      const boardRead = vi.spyOn(made.backend, 'cards');
      const showing = made.board.showCard(cardId);
      const card = made.board.openCard;
      if (!card) throw new Error('no card open');
      // The room refuses the reader before the card's reading has answered.
      await vi.waitFor(() => {
        expect(card.room?.reason).toBe('not_found');
      });
      read.open();
      await showing;
      await settle();
      expect(card.missing).toBe(true);
      expect(card.room).toBeUndefined();
      expect(opened).toEqual([{ cardId, closed: true }]);
      expect(roomsOf(made.world).members(cardId)).toBe(0);
      // The reading under way said whether the card is there: it is not read again, nor the board.
      expect(reading).toHaveBeenCalledTimes(1);
      expect(boardRead).not.toHaveBeenCalled();
    });
  }

  it('finds a card deleted after it was read, the room telling before the reading arrives', async () => {
    const made = await openFirstBoard();
    const cardId = realCard(made, 'Alpha');
    const { backend } = made;
    const [readCard, readComments, readAttachments] = [
      backend.card.bind(backend),
      backend.comments.bind(backend),
      backend.attachments.bind(backend),
    ];
    // The card is read whole, then somebody deletes it; what was read arrives when let go.
    const deleted = gate();
    const arrives = gate();
    vi.spyOn(backend, 'card').mockImplementation(async (...asked) => {
      const detail = await readCard(...asked);
      await deleted.passed;
      await arrives.passed;
      return detail;
    });
    vi.spyOn(backend, 'comments').mockImplementation(async (...asked) => {
      const list = await readComments(...asked);
      await arrives.passed;
      return list;
    });
    vi.spyOn(backend, 'attachments').mockImplementation(async (...asked) => {
      const list = await readAttachments(...asked);
      deleted.open();
      await arrives.passed;
      return list;
    });
    const showing = made.board.showCard(cardId);
    const card = made.board.openCard;
    if (!card) throw new Error('no card open');
    await deleted.passed;
    await made.other.deleteBlock(made.board.boardId, cardId);
    await vi.waitFor(() => {
      expect(card.room?.reason).toBe('not_found');
    });
    arrives.open();
    await showing;
    await vi.waitFor(() => {
      expect(card.missing).toBe(true);
    });
  });

  it('leaves the room of a card that could not be read for another reason', async () => {
    const made = await openFirstBoard();
    const { opened } = watchRooms(made);
    const cardId = realCard(made, 'Alpha');
    made.backend.failNext('card', new BackendError(503, { detail: 'down' }));
    const card = await made.board.showCard(cardId);
    expect(card.status).toBe('failed');
    expect(card.room).toBeUndefined();
    expect(opened).toEqual([{ cardId, closed: true }]);
    await settle();
    expect(roomsOf(made.world).members(cardId)).toBe(0);
  });

  it('leaves no room behind when cards are opened and closed fast, their readings late', async () => {
    const made = await openFirstBoard();
    const { opened } = watchRooms(made);
    const [alpha, beta] = [realCard(made, 'Alpha'), realCard(made, 'Beta')];
    made.backend.latency = 30;
    const first = made.board.showCard(alpha);
    const second = made.board.showCard(beta);
    const third = made.board.showCard(alpha);
    // Only the card open now has its room; the others' were left as they went.
    expect(opened.map((one) => [one.cardId, one.closed])).toEqual([
      [alpha, true],
      [beta, true],
      [alpha, false],
    ]);
    // The first reading of Alpha answers after Beta was opened: it opens nothing again.
    await Promise.all([first, second]);
    expect(opened).toHaveLength(3);
    const open = await third;
    await vi.waitFor(() => {
      expect(open.room?.status).toBe('joined');
    });
    expect(roomsOf(made.world).members(alpha)).toBe(1);
    expect(roomsOf(made.world).members(beta)).toBe(0);
    made.board.closeCard();
    made.backend.latency = 0;
    await vi.waitFor(() => {
      expect(roomsOf(made.world).members(alpha)).toBe(0);
    });
    expect(opened.every((one) => one.closed)).toBe(true);
  });

  it("closes a card's room in test teardown when the test leaves it open", async () => {
    const made = await openFirstBoard();
    const cardId = realCard(made, 'Alpha');
    const card = await made.board.showCard(cardId);
    await vi.waitFor(() => {
      expect(card.room?.status).toBe('joined');
    });
    expect(roomsOf(made.world).members(cardId)).toBe(1);
    onTestFinished(async () => {
      expect(card.room).toBeUndefined();
      await vi.waitFor(() => {
        expect(roomsOf(made.world).members(cardId)).toBe(0);
      });
    });
  });
});
