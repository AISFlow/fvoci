/**
 * What a drag on the calendar or a Gantt chart writes to the board
 * (features/views/calendar/write.ts): a date moved whole or by one end, in
 * one change that one undo takes back; a change the backend refuses, put
 * back; and a moment the wire cannot carry, not written at all.
 */
import { describe, expect, it } from 'vitest';

import { moveCard, resizeCard, writeDays } from '$lib/features/views/calendar/write.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';
import { cardProperty, type PropertyTemplate } from '$lib/logic/wire.ts';
import { BackendError } from '$lib/state/index.ts';

import { openFirstBoard, setup, SMALL } from './support.ts';

const DUE: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
const day = (text: string): CalendarDate => text as CalendarDate;
const ZONE = 'Asia/Seoul';

/** The small board with a date property, Alpha on 12 to 14 November and Beta at 08:00 on 2 January 1970 in Seoul. */
async function dated(): Promise<Awaited<ReturnType<typeof openFirstBoard>>> {
  const template = structuredClone(SMALL);
  template.board.cardProperties.push(DUE);
  const [alpha, beta] = template.cards;
  if (!alpha || !beta) throw new Error('the seed has no cards');
  alpha.fields.properties.due = { from: '2026-11-12', to: '2026-11-14' };
  beta.fields.properties.due = { from: Date.UTC(1970, 0, 1, 23, 0) };
  return openFirstBoard(setup([template]));
}

function cardNamed(board: Awaited<ReturnType<typeof dated>>['board'], title: string) {
  const card = board.realCards.find((each) => each.title === title);
  if (!card) throw new Error(`no card ${title}`);
  return card;
}

describe('a dragged date, written', () => {
  it('moves whole, keeping its length, and one undo takes it back', async () => {
    const { board, workspace } = await dated();
    const alpha = cardNamed(board, 'Alpha');
    expect(await moveCard(board, alpha, DUE, day('2026-11-30'), ZONE)).toBe(true);
    expect(cardProperty(cardNamed(board, 'Alpha'), 'due')).toEqual({
      from: '2026-11-30',
      to: '2026-12-02',
    });
    await workspace.undoLast();
    expect(cardProperty(cardNamed(board, 'Alpha'), 'due')).toEqual({
      from: '2026-11-12',
      to: '2026-11-14',
    });
  });

  it('moves one end, and the days of a drag as they come', async () => {
    const { board } = await dated();
    expect(
      await resizeCard(board, cardNamed(board, 'Alpha'), DUE, 'end', day('2026-11-20'), ZONE),
    ).toBe(true);
    expect(cardProperty(cardNamed(board, 'Alpha'), 'due')).toEqual({
      from: '2026-11-12',
      to: '2026-11-20',
    });
    const days = { first: day('2026-11-10'), last: day('2026-11-20') };
    expect(await writeDays(board, cardNamed(board, 'Alpha'), DUE, 'start', days, ZONE)).toBe(true);
    expect(cardProperty(cardNamed(board, 'Alpha'), 'due')).toEqual({
      from: '2026-11-10',
      to: '2026-11-20',
    });
  });

  it('is put back when the backend refuses it', async () => {
    const { board, backend, workspace } = await dated();
    backend.failNext('patchCard', new BackendError(403));
    expect(await moveCard(board, cardNamed(board, 'Alpha'), DUE, day('2026-11-30'), ZONE)).toBe(
      false,
    );
    expect(cardProperty(cardNamed(board, 'Alpha'), 'due')).toEqual({
      from: '2026-11-12',
      to: '2026-11-14',
    });
    expect(workspace.canUndo).toBe(false);
  });

  it('is not written where a moment would be before 1970, which the wire cannot carry', async () => {
    const { board, workspace } = await dated();
    const beta = cardNamed(board, 'Beta');
    expect(await moveCard(board, beta, DUE, day('1969-12-30'), ZONE)).toBe(false);
    expect(cardProperty(cardNamed(board, 'Beta'), 'due')).toEqual({
      from: Date.UTC(1970, 0, 1, 23, 0),
    });
    expect(workspace.canUndo).toBe(false);
    // A day without a time can be.
    expect(await moveCard(board, cardNamed(board, 'Alpha'), DUE, day('1969-12-30'), ZONE)).toBe(
      true,
    );
  });
});
