/**
 * What the account's hub of today changes (`state/today-hub.svelte.ts`) over
 * the backend in memory: a card made done by its board's rule, a rule of done
 * chosen where its board had none, a unit put off by days, and a line for the
 * diary of the reader's own space; and the value a rule of done gives
 * (`logic/done-rule.ts`).
 */
import { describe, expect, it } from 'vitest';

import { addDays, type CalendarDate } from '$lib/logic/dates.ts';
import { doneChoices, doneValue } from '$lib/logic/done-rule.ts';
import type { DoneRule, PropertyTemplate } from '$lib/logic/wire.ts';
import { TodayHubStore } from '$lib/state/index.ts';

import { openFirstBoard } from './support.ts';

/** The first board, done at its status's last option, with a card of the reader's due today. */
async function dueToday() {
  const made = await openFirstBoard();
  const { board, backend, workspace } = made;
  const status = board.board?.cardProperties.find((prop) => prop.type === 'select');
  const doneOption = status?.options.at(-1);
  if (!status || !doneOption) throw new Error('the seed board has no status');
  const rule: DoneRule = { kind: 'option', propertyId: status.id, optionIds: [doneOption.id] };
  await board.changeBoard((cur) => ({
    ...cur,
    doneWhen: rule,
    dueDatePropertyId: 'due',
    cardProperties: [
      ...cur.cardProperties.filter((prop) => prop.id !== 'owner'),
      { id: 'owner', name: 'Owner', type: 'person', options: [] },
      { id: 'due', name: 'Due', type: 'date', options: [] },
    ],
  }));
  const today = (await backend.accountToday()).today;
  const card = board.realCards.find((one) => one.fields.properties[status.id] !== doneOption.id);
  if (!card) throw new Error('the seed has no card left to do');
  await backend.patchCard(board.boardId, card.id, {
    updatedProperties: { owner: workspace.me?.id ?? '', due: { from: today } },
  });
  return { ...made, card, today, rule, doneId: doneOption.id };
}

describe('the hub of today', () => {
  it('makes a card done by its board’s rule, at once and on the server', async () => {
    const { board, backend, workspace, card, rule, doneId } = await dueToday();
    const hub = new TodayHubStore(workspace);
    await hub.load();
    await expect.poll(() => hub.board(board.boardId)?.canEdit).toBe(true);
    const item = hub.data?.cards.find((one) => one.cardId === card.id);
    if (!item) throw new Error('the card is not due on the hub');

    const done = hub.markDone(item);
    expect(hub.data?.cards.some((one) => one.cardId === card.id)).toBe(false);
    await done;
    const read = await backend.card(board.boardId, card.id);
    expect(read.fields.properties[rule.propertyId]).toBe(doneId);
    expect(workspace.problems).toHaveLength(0);
    hub.close();
  });

  it('keeps a card made done off the list where a read on its way had it still', async () => {
    const { board, backend, workspace, card } = await dueToday();
    const hub = new TodayHubStore(workspace);
    await hub.load();
    await expect.poll(() => hub.canMarkDone(board.boardId)).toBe(true);
    const item = hub.data?.cards.find((one) => one.cardId === card.id);
    if (!item) throw new Error('the card is not due on the hub');

    // A read goes out (a notification came), and the card is made done before it answers.
    backend.latency = 20;
    const reading = hub.load({ diaries: false });
    const done = hub.markDone(item);
    await Promise.all([reading, done]);
    backend.latency = 0;
    await expect.poll(() => hub.status).toBe('ready');
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(hub.data?.cards.some((one) => one.cardId === card.id)).toBe(false);
    hub.close();
  });

  it('offers no done for a board whose rule names no option, and does not drop its card', async () => {
    const { board, workspace, card, rule } = await dueToday();
    await board.changeBoard((cur) => ({ ...cur, doneWhen: { ...rule, optionIds: [] } }));
    const hub = new TodayHubStore(workspace);
    await hub.load();
    await expect.poll(() => hub.board(board.boardId)?.canEdit).toBe(true);
    expect(hub.canMarkDone(board.boardId)).toBe(false);
    const item = hub.data?.cards.find((one) => one.cardId === card.id);
    if (!item) throw new Error('the card is not due on the hub');
    await hub.markDone(item);
    expect(hub.data?.cards.some((one) => one.cardId === card.id)).toBe(true);
    hub.close();
  });

  it('offers the choices of a board with no rule, and the chosen one is the board’s', async () => {
    const { board, backend, workspace } = await dueToday();
    await board.changeBoard((cur) => ({ ...cur, doneWhen: null }));
    const hub = new TodayHubStore(workspace);
    await hub.load();
    await expect.poll(() => hub.board(board.boardId)?.board.doneWhen ?? undefined).toBe(undefined);
    const read = hub.board(board.boardId);
    if (!read) throw new Error('the board is not read');
    expect(read.canChooseDone).toBe(true);
    const [choice] = doneChoices(read.board.cardProperties);
    if (!choice) throw new Error('the board offers no rule');

    await hub.chooseDone(board.boardId, choice.rule);
    expect(hub.board(board.boardId)?.board.doneWhen).toEqual(choice.rule);
    expect((await backend.container(board.boardId)).doneWhen).toEqual(choice.rule);
    hub.close();
  });

  it('puts a unit due today off by days, on the reader’s own plan', async () => {
    const { backend, workspace, world } = await openFirstBoard();
    const course = await backend.course.createCourse(world.workspace.id, {
      title: 'Reading',
      visibility: 'private',
    });
    const unit = await backend.course.createUnit(course.id, { kind: 'unit', title: 'Chapter 1' });
    const today = (await backend.accountToday()).today as CalendarDate;
    await backend.course.putUnitSchedule(course.id, unit.id, { plannedDate: today });
    const hub = new TodayHubStore(workspace);
    await hub.load();
    const item = hub.data?.units.find((one) => one.unitId === unit.id);
    if (!item) throw new Error('the unit is not due on the hub');

    await hub.putOff(item, 3);
    expect(hub.data?.units.some((one) => one.unitId === unit.id)).toBe(false);
    const planned = (await backend.course.schedule(course.id)).units.find(
      (one) => one.unitId === unit.id,
    );
    expect(planned?.myPlannedDate).toBe(addDays(today, 3));
    hub.close();
  });
});

describe('the value a rule of done gives', () => {
  const status: PropertyTemplate = {
    id: 'status',
    name: 'Status',
    type: 'select',
    options: [
      { id: 'todo', label: 'To do', color: 'default' },
      { id: 'done', label: 'Done', color: 'default' },
    ],
  };
  const tags: PropertyTemplate = { ...status, id: 'tags', name: 'Tags', type: 'multiSelect' };
  const ticked: PropertyTemplate = { id: 'ok', name: 'Finished', type: 'checkbox', options: [] };

  it('is offered by checkboxes and by each option of a select', () => {
    expect(doneChoices([ticked, status]).map((choice) => [choice.value, choice.short])).toEqual([
      ['checkbox:ok', 'Finished'],
      ['option:status:todo', 'To do'],
      ['option:status:done', 'Done'],
    ]);
  });

  it('ticks a checkbox, chooses an option, and adds one to the options of a multi-select', () => {
    const option: DoneRule = { kind: 'option', propertyId: 'tags', optionIds: ['done'] };
    expect(doneValue({ kind: 'checkbox', propertyId: 'ok' }, ticked, undefined)).toBe('true');
    expect(doneValue({ ...option, propertyId: 'status' }, status, 'todo')).toBe('done');
    expect(doneValue(option, tags, ['todo'])).toEqual(['todo', 'done']);
    expect(doneValue(option, tags, ['done'])).toEqual(['done']);
  });
});
