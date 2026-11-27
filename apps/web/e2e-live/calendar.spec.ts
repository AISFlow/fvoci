/**
 * The calendar view on the real server: made on the page from the list of
 * kinds, dated cards on their days, and the date it is placed by chosen on
 * the page; after a reload the view is still a calendar placed by that date,
 * as the server keeps it. The server does not check the kind of a view, so a
 * calendar is a view like any other to it; "display by" is the view's
 * `dateProperty`. A date changed on the calendar is one change of a
 * card property, kept like any other.
 */
import { expect, test } from '@playwright/test';

import { primaryKey } from '../e2e/keyboard.support.ts';
import {
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US', timezoneId: 'Asia/Seoul' });

test('a calendar view of the server shows dated cards, and keeps its "display by"', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('calendar');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Harvest calendar' });
  // A second date, after "Due": the one the page will choose. Not the first, which a new
  // calendar is placed by, so that the reload shows the choice and not the start.
  const added = await api.patch(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`, {
    data: { updatedCardProperties: [{ id: 'start', name: 'Start', type: 'date', options: [] }] },
  });
  expect(added.ok()).toBe(true);
  for (const [title, due, start] of [
    ['Pick apples', { from: '2026-11-12' }, { from: '2026-11-03' }],
    ['Press cider', { from: '2026-11-12', to: '2026-11-17' }, { from: '2026-11-04' }],
  ] as const) {
    const answer = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
      data: { title, properties: { due, start } },
    });
    expect(answer.ok()).toBe(true);
  }
  /** The board's calendar view as the server keeps it. */
  const stored = async (): Promise<Schemas['Block'] | undefined> => {
    const views = (await (
      await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`, {
        params: { type: 'view' },
      })
    ).json()) as Schemas['Block'][];
    return views.find((each) => each.fields.viewType === 'calendar');
  };

  await page.clock.setFixedTime(new Date('2026-11-10T00:00:00Z'));
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);

  // Made on the page: placed by the first date of the board, "Due".
  const header = page.getByTestId('view-header');
  await header.locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add calendar view' }).click();
  await expect.poll(async () => (await stored())?.title).toBe('Calendar view');
  const viewId = (await stored())?.id ?? '';
  await expect(page).toHaveURL(new RegExp(`${boardPath(board, viewId)}$`));
  expect((await stored())?.fields.dateProperty).toBe('due');

  const calendar = page.getByTestId('calendar');
  await expect(calendar).toHaveAttribute('data-layout', 'bars');
  await expect(page.getByTestId('calendar-period')).toHaveText('November 2026');
  const grid = page.getByTestId('month-grid');
  await expect(grid.locator('[data-bar]').filter({ hasText: 'Pick apples' })).toHaveCount(1);
  // Thursday the 12th to Tuesday the 17th: two weeks, two bars.
  await expect(grid.locator('[data-bar]').filter({ hasText: 'Press cider' })).toHaveCount(2);

  const tool = header.locator('[data-tool="display-by"]');
  await expect(tool).toHaveAccessibleName('Display by: Due');
  await tool.click();
  await page.locator('[role="menuitem"][data-action="start"]').click();
  await expect.poll(async () => (await stored())?.fields.dateProperty).toBe('start');
  const week = (date: string) => grid.locator(`[data-week="${date}"] [data-bar]`);
  await expect(week('2026-11-01').filter({ hasText: 'Pick apples' })).toHaveCount(1);

  // After a reload the view is still a calendar, placed by "Start" as the server keeps it.
  await page.reload();
  await expect(calendar).toHaveAttribute('data-layout', 'bars');
  await expect(tool).toHaveAccessibleName('Display by: Start');
  await expect(week('2026-11-01').filter({ hasText: 'Pick apples' })).toHaveCount(1);
  await expect(week('2026-11-01').filter({ hasText: 'Press cider' })).toHaveCount(1);
  await expect(week('2026-11-08')).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`${boardPath(board, viewId)}$`));
  expect((await stored())?.fields.viewType).toBe('calendar');
  expect(errors).toEqual([]);
  await api.dispose();
});

test('a date changed on the calendar is kept by the server, whole, after a reload', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('calendar-move');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Moving calendar' });
  const made = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
    data: { title: 'Press cider', properties: { due: { from: '2026-11-12', to: '2026-11-17' } } },
  });
  expect(made.ok()).toBe(true);
  const cardId = ((await made.json()) as Schemas['Card']).id;
  const due = async (): Promise<unknown> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
      ).json()) as Schemas['Card']
    ).properties.due;

  await page.clock.setFixedTime(new Date('2026-11-10T00:00:00Z'));
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await page.getByTestId('view-header').locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add calendar view' }).click();
  const grid = page.getByTestId('month-grid');
  await expect(grid.locator('[data-bar]').filter({ hasText: 'Press cider' })).toHaveCount(2);

  // From the cards of the 12th: the card's "…", "Change date", the 19th.
  await grid.locator('[data-day="2026-11-12"] [data-day-button]').click();
  const line = page.getByTestId('day-popover').locator('[data-card]');
  await line.hover();
  await line.getByRole('button', { name: 'Card menu: Press cider' }).click();
  await page.locator('[data-action="change-date"]').click();
  await page.getByTestId('day-picker').locator('[data-bits-day][data-value="2026-11-19"]').click();
  // The range keeps its six days.
  await expect.poll(due).toEqual({ from: '2026-11-19', to: '2026-11-24' });

  await page.reload();
  const week = (date: string) => grid.locator(`[data-week="${date}"] [data-bar]`);
  await expect(week('2026-11-15').filter({ hasText: 'Press cider' })).toHaveCount(1);
  await expect(week('2026-11-22').filter({ hasText: 'Press cider' })).toHaveCount(1);
  await expect(week('2026-11-08')).toHaveCount(0);
  expect(errors).toEqual([]);
  await api.dispose();
});

test('on the server: a card made on a day, undone; a timed card moved keeps its time; a date given from "No date"', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('calendar-write');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Writing calendar' });
  const make = async (title: string, due?: unknown): Promise<string> => {
    const answer = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
      data: { title, properties: due === undefined ? {} : { due } },
    });
    expect(answer.ok()).toBe(true);
    return ((await answer.json()) as Schemas['Card']).id;
  };
  // 08:00 on the 10th in Seoul.
  const timed = await make('Morning call', { from: Date.parse('2026-11-09T23:00:00Z') });
  const loose = await make('Someday');
  const cardsOf = async (): Promise<Schemas['Card'][]> =>
    (await (
      await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
        params: { perPage: 1000 },
      })
    ).json()) as Schemas['Card'][];
  const dueOf = async (cardId: string): Promise<unknown> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
      ).json()) as Schemas['Card']
    ).properties.due;

  await page.clock.setFixedTime(new Date('2026-11-10T00:00:00Z'));
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await page.getByTestId('view-header').locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add calendar view' }).click();
  const grid = page.getByTestId('month-grid');
  await expect(grid.locator('[data-bar]').filter({ hasText: 'Morning call' })).toHaveCount(1);

  // A card on the empty 25th, from the day's list: the server has it with that day.
  await grid.locator('[data-day="2026-11-25"] [data-day-button]').click();
  await page.getByTestId('day-popover').getByTestId('day-new-card').click();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect
    .poll(async () => (await cardsOf()).filter((card) => card.properties.due !== undefined).length)
    .toBe(2);
  const made = (await cardsOf()).find((card) => card.id !== timed && card.id !== loose);
  expect(made?.properties.due).toEqual({ from: '2026-11-25' });
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  // One undo takes it away on the server too.
  await page.keyboard.press(await primaryKey(page, 'z'));
  await expect.poll(async () => (await cardsOf()).length).toBe(2);

  // The timed card to the 19th: 08:00 there, on the reader's clock.
  await grid.locator('[data-day="2026-11-10"] [data-day-button]').click();
  const line = page.getByTestId('day-popover').locator('[data-card]');
  await line.hover();
  await line.getByRole('button', { name: 'Card menu: Morning call' }).click();
  await page.locator('[data-action="change-date"]').click();
  await page.getByTestId('day-picker').locator('[data-bits-day][data-value="2026-11-19"]').click();
  await expect.poll(() => dueOf(timed)).toEqual({ from: Date.parse('2026-11-18T23:00:00Z') });

  // "Someday" is given the 20th from "No date".
  await page.getByTestId('calendar-undated').click();
  const undated = page.getByTestId('undated-list').locator('[data-card]');
  await undated.hover();
  await undated.getByRole('button', { name: 'Card menu: Someday' }).click();
  await page.locator('[data-action="change-date"]').click();
  await page.getByTestId('day-picker').locator('[data-bits-day][data-value="2026-11-20"]').click();
  await expect.poll(() => dueOf(loose)).toEqual({ from: '2026-11-20' });

  await page.reload();
  await expect(grid.locator('[data-week="2026-11-15"] [data-bar]')).toHaveCount(2);
  expect(errors).toEqual([]);
  await api.dispose();
});
