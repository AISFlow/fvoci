/**
 * The Gantt view on the real server: made on the page from the list of
 * kinds, dated cards as bars on their days, and what the view is placed by,
 * the card relation it reads as its dependencies and its zoom chosen on the
 * page; after a reload the view is as it was left, as the server keeps it.
 * A choice taken back (an undo) reaches the server too; the last test has
 * that for a view that names no date, which only the API can make on a
 * board that has one.
 *
 * The server does not check the kind of a view or the members of its
 * `fields`, so a Gantt chart is a view like any other to it.
 */
import {
  expect,
  test,
  type APIRequestContext,
  type Locator,
  type Page,
  type Response,
} from '@playwright/test';

import { primaryKey } from '../e2e/keyboard.support.ts';
import {
  boardPath,
  type MadeBoard,
  makeBoard,
  type Person,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US', timezoneId: 'Asia/Seoul' });

const bar = (page: Page, title: string): Locator =>
  page
    .getByTestId('gantt')
    .locator('[data-row]')
    .filter({ has: page.locator('[data-row-title]', { hasText: title }) })
    .locator('[data-bar]');

/** A board of someone new, and a client of the API signed in as them. */
async function start(
  baseURL: string | undefined,
  name: string,
  title: string,
): Promise<{ owner: Person; api: APIRequestContext; board: MadeBoard }> {
  const base = baseURL ?? '';
  const owner = person(name);
  await register(base, owner);
  const api = await signedIn(base, owner);
  return { owner, api, board: await makeBoard(api, { title }) };
}

/** Gives the board a property through the API. */
async function define(
  api: APIRequestContext,
  board: { workspaceId: string; boardId: string },
  property: { id: string; name: string; type: string },
): Promise<void> {
  const answer = await api.patch(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`, {
    data: { updatedCardProperties: [{ ...property, options: [] }] },
  });
  expect(answer.ok(), await answer.text()).toBe(true);
}

/** Makes a card through the API and answers its identifier. */
async function card(
  api: APIRequestContext,
  board: { workspaceId: string; boardId: string },
  title: string,
  properties: Record<string, unknown>,
): Promise<string> {
  const answer = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
    data: { title, properties },
  });
  expect(answer.ok(), await answer.text()).toBe(true);
  return ((await answer.json()) as Schemas['Card']).id;
}

/** The fields of a view as the server keeps them; the view has to be there. */
async function storedFields(
  api: APIRequestContext,
  board: { workspaceId: string; boardId: string },
  viewId: string,
): Promise<Record<string, unknown>> {
  const view = (await ganttViews(api, board)).find((each) => each.id === viewId);
  if (!view) throw new Error(`the server has no Gantt view ${viewId}`);
  return view.fields;
}

/** The Gantt views of a board as the server keeps them. */
async function ganttViews(
  api: APIRequestContext,
  board: { workspaceId: string; boardId: string },
): Promise<Schemas['Block'][]> {
  const views = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/views`)
  ).json()) as Schemas['Block'][];
  return views.filter((view) => view.fields.viewType === 'gantt');
}

/** Opens the board signed in, at a fixed day, and makes a Gantt view on the page; answers its identifier. */
async function addGantt(
  page: Page,
  owner: Person,
  api: APIRequestContext,
  board: MadeBoard,
): Promise<string> {
  await page.clock.setFixedTime(new Date('2026-11-10T00:00:00Z'));
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await page.getByTestId('view-header').locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add Gantt view' }).click();
  await expect(page.getByTestId('gantt')).toBeVisible();
  await expect.poll(async () => (await ganttViews(api, board)).length).toBe(1);
  const viewId = (await ganttViews(api, board))[0]?.id ?? '';
  await expect(page).toHaveURL(new RegExp(`${boardPath(board, viewId)}$`));
  return viewId;
}

test('a Gantt view made on the page draws dated cards as bars, and the server keeps it and its "display by"', async ({
  page,
  baseURL,
}) => {
  const { owner, api, board } = await start(baseURL, 'gantt', 'Harvest plan');
  // A second date, after "Due": the one the page will choose.
  await define(api, board, { id: 'start', name: 'Start', type: 'date' });
  const days = { due: { from: '2026-11-12' }, start: { from: '2026-11-03' } };
  await card(api, board, 'Pick apples', days);
  await card(api, board, 'Press cider', {
    due: { from: '2026-11-12', to: '2026-11-17' },
    start: { from: '2026-11-04' },
  });
  const errors = watchErrors(page);
  const viewId = await addGantt(page, owner, api, board);
  const fields = (): Promise<Record<string, unknown>> => storedFields(api, board, viewId);

  // Made as the page makes one: placed by the first date, zoomed to weeks, no relation to read.
  expect(await fields()).toMatchObject({ dateProperty: 'due', ganttZoom: 'week' });
  expect('dependencyPropertyId' in (await fields())).toBe(false);
  const gantt = page.getByTestId('gantt');
  await expect(gantt).toHaveAttribute('data-zoom', 'week');
  await expect(gantt.locator('[data-bar]')).toHaveCount(2);
  await expect(bar(page, 'Pick apples')).toHaveAccessibleName('number 1, Pick apples, November 12');
  await expect(bar(page, 'Press cider')).toHaveAccessibleName(
    'number 2, Press cider, November 12 – November 17',
  );
  /** How many days a bar is wide, by the marks of the axis (a week each). */
  const daysWide = async (title: string): Promise<number> => {
    const week = await gantt.locator('[data-tick]').nth(1).boundingBox();
    const box = await bar(page, title).boundingBox();
    if (!week || !box) throw new Error('nothing to measure');
    return Math.round(box.width / (week.width / 7));
  };
  expect(await daysWide('Press cider')).toBe(6);

  const tool = page.getByTestId('view-header').locator('[data-tool="display-by"]');
  await expect(tool).toHaveAccessibleName('Display by: Due');
  await tool.click();
  await page.locator('[role="menuitem"][data-action="start"]').click();
  await expect.poll(async () => (await fields()).dateProperty).toBe('start');
  await expect(bar(page, 'Press cider')).toHaveAccessibleName('number 2, Press cider, November 4');

  // After a reload the view is still a Gantt chart, placed by "Start" as the server keeps it.
  await page.reload();
  await expect(gantt.locator('[data-bar]')).toHaveCount(2);
  await expect(tool).toHaveAccessibleName('Display by: Start');
  await expect(bar(page, 'Pick apples')).toHaveAccessibleName('number 1, Pick apples, November 3');
  expect(await daysWide('Press cider')).toBe(1);
  expect((await fields()).viewType).toBe('gantt');
  expect(errors).toEqual([]);
  await api.dispose();
});

test('the dependencies and the zoom chosen on the page are kept by the server, and a choice taken back is too', async ({
  page,
  baseURL,
}) => {
  const { owner, api, board } = await start(baseURL, 'gantt-deps', 'Building plan');
  const foundation = await card(api, board, 'Pour the foundation', {
    due: { from: '2026-11-03', to: '2026-11-06' },
  });
  const errors = watchErrors(page);
  // Made before the board has a relation: the view names none.
  const viewId = await addGantt(page, owner, api, board);
  const fields = (): Promise<Record<string, unknown>> => storedFields(api, board, viewId);
  expect('dependencyPropertyId' in (await fields())).toBe(false);

  await define(api, board, { id: 'after', name: 'After', type: 'relation' });
  // Starts on the day the foundation ends: in order.
  const walls = await card(api, board, 'Build the walls', {
    due: { from: '2026-11-06', to: '2026-11-13' },
    after: [{ card: foundation, kind: 'fs', lagDays: 0 }],
  });
  // Starts before the walls end: out of order. And names a card that is not there: one
  // deleted since (the server takes only cards of the board, in the trash too).
  const scrapped = await card(api, board, 'Scrapped', {});
  await card(api, board, 'Raise the roof', {
    due: { from: '2026-11-11', to: '2026-11-14' },
    after: [
      { card: walls, kind: 'fs', lagDays: 0 },
      { card: scrapped, kind: 'fs', lagDays: 0 },
    ],
  });
  const deleted = await api.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/batch-delete`,
    { data: { cardIds: [scrapped] } },
  );
  expect(deleted.ok(), await deleted.text()).toBe(true);
  await page.reload();
  const gantt = page.getByTestId('gantt');
  const arrows = gantt.locator('[data-arrow]');
  await expect(gantt.locator('[data-bar]')).toHaveCount(3);
  await expect(arrows).toHaveCount(0);

  // "Dependencies" is behind "…" of the header.
  const more = page.getByTestId('view-header').locator('[data-tool="more"]');
  const line = page.locator('[role="menuitem"][data-action="dependencies"]');
  await more.click();
  await expect(line).toContainText('None');
  await page.keyboard.press('Escape');
  await expect(line).toHaveCount(0);
  const choose = async (): Promise<void> => {
    await more.click();
    await line.click();
    await page.locator('[role="menuitem"][data-action="after"]').click();
    await expect.poll(async () => (await fields()).dependencyPropertyId).toBe('after');
    await expect(arrows).toHaveCount(2);
  };
  await choose();
  // The first choice taken back reaches the server: the view names no relation, as before.
  await page.getByTestId('undo').click();
  await expect(arrows).toHaveCount(0);
  await expect.poll(async () => 'dependencyPropertyId' in (await fields())).toBe(false);
  await choose();
  await expect(gantt).toHaveAttribute('data-edges', '2');
  await expect(page.getByTestId('gantt-late')).toHaveAccessibleName(
    'Out of order: 1. Show the next.',
  );
  await expect(bar(page, 'Raise the roof')).toHaveAccessibleName(
    'number 4, Raise the roof, November 11 – November 14. After Build the walls. Out of order: starts before the end of Build the walls.',
  );

  await page.getByTestId('gantt-zoom-in').click();
  await expect(gantt).toHaveAttribute('data-zoom', 'day');
  await expect.poll(async () => (await fields()).ganttZoom).toBe('day');

  // After a reload the chart is as it was left: its relation, its arrows, its zoom.
  await page.reload();
  await expect(gantt).toHaveAttribute('data-zoom', 'day');
  await expect(arrows).toHaveCount(2);
  await expect(gantt.locator('[data-arrow][data-late]')).toHaveCount(1);
  await more.click();
  await expect(line).toContainText('After');
  await page.keyboard.press('Escape');
  expect(await fields()).toMatchObject({
    viewType: 'gantt',
    dateProperty: 'due',
    dependencyPropertyId: 'after',
    ganttZoom: 'day',
  });
  expect(errors).toEqual([]);
  await api.dispose();
});

test('"display by" chosen for a view that named no date can be taken back on the server', async ({
  page,
  baseURL,
}) => {
  const { owner, api, board } = await start(baseURL, 'gantt-undo', 'Sowing plan');
  await define(api, board, { id: 'start', name: 'Start', type: 'date' });
  await card(api, board, 'Sow the beans', {
    due: { from: '2026-11-12' },
    start: { from: '2026-11-03' },
  });
  // A view as another client may have stored it: a Gantt chart that names no date.
  const made = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/views`, {
    data: {
      title: 'Bare Gantt',
      viewType: 'gantt',
      fields: {
        viewType: 'gantt',
        sort: [],
        shownProperties: ['__title'],
        visibleGroups: [],
        hiddenGroups: [],
        collapsedGroups: [],
        cardOrder: [],
        filter: { operation: 'and', filters: [] },
      },
    },
  });
  expect(made.ok(), await made.text()).toBe(true);
  const viewId = ((await made.json()) as Schemas['BoardViewWrite']).view.id;
  const fields = (): Promise<Record<string, unknown>> => storedFields(api, board, viewId);

  await page.clock.setFixedTime(new Date('2026-11-10T00:00:00Z'));
  const errors = watchErrors(page);
  await page.goto(boardPath(board, viewId));
  await signInOnPage(page, owner);
  // Placed by the first date, "Due", while it names none.
  await expect(bar(page, 'Sow the beans')).toHaveAccessibleName(
    'number 1, Sow the beans, November 12',
  );
  const tool = page.getByTestId('view-header').locator('[data-tool="display-by"]');
  await tool.click();
  await page.locator('[role="menuitem"][data-action="start"]').click();
  await expect.poll(async () => (await fields()).dateProperty).toBe('start');
  await expect(bar(page, 'Sow the beans')).toHaveAccessibleName(
    'number 1, Sow the beans, November 3',
  );

  // Taken back: the server drops the name, and a reload shows the view as it was.
  await page.getByTestId('undo').click();
  await expect.poll(async () => 'dateProperty' in (await fields())).toBe(false);
  await page.reload();
  await expect(bar(page, 'Sow the beans')).toHaveAccessibleName(
    'number 1, Sow the beans, November 12',
  );
  await expect(tool).toHaveAccessibleName('Display by: Due');
  expect(errors).toEqual([]);
  await api.dispose();
});

test('a bar dragged on the page, and an end of it, change the card on the server; one undo takes the last back', async ({
  page,
  baseURL,
}) => {
  const { owner, api, board } = await start(baseURL, 'gantt-drag', 'Moving plan');
  const cider = await card(api, board, 'Press cider', {
    due: { from: '2026-11-12', to: '2026-11-17' },
  });
  // 08:00 on the 10th in Seoul.
  const call = await card(api, board, 'Morning call', {
    due: { from: Date.parse('2026-11-09T23:00:00Z') },
  });
  const dueOf = async (cardId: string): Promise<unknown> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
      ).json()) as Schemas['Card']
    ).properties.due;
  const errors = watchErrors(page);
  await addGantt(page, owner, api, board);
  const gantt = page.getByTestId('gantt');
  await expect(gantt.locator('[data-bar]')).toHaveCount(2);
  const week = await gantt.locator('[data-tick]').nth(1).boundingBox();
  if (!week) throw new Error('no marks on the axis');
  const day = week.width / 7;
  /**
   * The page's change of a card, once the server has answered it. The test
   * goes by that answer, not by a while: on a busy machine a write may be
   * answered after any while the test would pick.
   */
  const written = (cardId: string): Promise<Response> =>
    page.waitForResponse(
      (response) =>
        response.request().method() === 'PATCH' &&
        new URL(response.url()).pathname.endsWith(`/cards/${cardId}`),
    );
  /** Answers once the server has answered the page's change of `cardId`, which went well. */
  const answered = async (cardId: string, change: () => Promise<void>): Promise<void> => {
    const answer = written(cardId);
    await change();
    expect((await answer).ok()).toBe(true);
  };
  /**
   * Presses on `target`, moves `days` days across, and lets go: the bar has
   * to be taken first (a press that did not take it would be a click), and
   * the change it makes to `cardId` answered by the server.
   */
  const dragBy = async (target: Locator, days: number, cardId: string): Promise<void> => {
    // Wait for the bar to be stable and hittable before taking its coordinates.
    await target.hover();
    const box = await target.boundingBox();
    if (!box) throw new Error('nothing to drag');
    const from = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + days * day, from.y, { steps: 10 });
    await expect(gantt.locator('[data-bar][data-dragged]')).toHaveCount(1);
    await answered(cardId, () => page.mouse.up());
  };

  // Moved whole: six days still, from the 15th.
  await dragBy(bar(page, 'Press cider'), 3, cider);
  expect(await dueOf(cider)).toEqual({ from: '2026-11-15', to: '2026-11-20' });
  // A moment keeps 08:00 on the reader's clock.
  await dragBy(bar(page, 'Morning call'), 2, call);
  expect(await dueOf(call)).toEqual({ from: Date.parse('2026-11-11T23:00:00Z') });
  // The last day by its grip.
  const row = gantt.locator('[data-row]').filter({ hasText: 'Press cider' });
  await row.hover();
  await dragBy(row.locator('[data-grip="end"]'), 2, cider);
  expect(await dueOf(cider)).toEqual({ from: '2026-11-15', to: '2026-11-22' });

  // One undo takes the last drag back, on the server too: once the page has
  // taken the answer to the last drag in as a step to undo (`data-settling`).
  await expect(page.locator('[data-settling]')).toHaveCount(0);
  await answered(cider, async () => page.keyboard.press(await primaryKey(page, 'z')));
  expect(await dueOf(cider)).toEqual({ from: '2026-11-15', to: '2026-11-20' });
  await page.reload();
  await expect(bar(page, 'Press cider')).toHaveAccessibleName(
    'number 1, Press cider, November 15 – November 20',
  );
  expect(errors).toEqual([]);
  await api.dispose();
});

test('editing and removing a Gantt arrow relation persists kind and lagDays on the server across reload', async ({
  page,
  baseURL,
}) => {
  const { owner, api, board } = await start(baseURL, 'gantt-rel-edit', 'Release schedule');
  await define(api, board, { id: 'after', name: 'After', type: 'relation' });
  const spec = await card(api, board, 'Write spec', {
    due: { from: '2026-11-10', to: '2026-11-12' },
  });
  const impl = await card(api, board, 'Implement feature', {
    due: { from: '2026-11-11', to: '2026-11-15' },
    after: [{ card: spec, kind: 'fs', lagDays: 0 }],
  });
  const errors = watchErrors(page);
  await addGantt(page, owner, api, board);
  const gantt = page.getByTestId('gantt');
  const trigger = gantt.locator(`[data-arrow-trigger][data-from="${spec}"][data-to="${impl}"]`);
  const hitPath = gantt.locator(`[data-arrow-hit][data-from="${spec}"][data-to="${impl}"]`);
  await expect(trigger).toHaveCount(1);
  // Initially FS 0 -> out of order because impl starts Nov 11 before spec ends Nov 12.
  await expect(gantt.locator('[data-arrow][data-late]')).toHaveCount(1);

  const patchAnswered = (): Promise<Response> =>
    page.waitForResponse(
      (response) =>
        response.request().method() === 'PATCH' &&
        new URL(response.url()).pathname.endsWith(`/cards/${impl}`),
    );

  // Edit FS 0 -> SS +1d by clicking the arrow hit path: impl starts Nov 11 >= spec start Nov 10 + 1 -> now in order!
  await hitPath.click({ force: true });
  const editor = page.getByTestId('gantt-relation-editor');
  await expect(editor).toBeVisible();
  await editor.locator('[data-relation-kind="ss"]').click();
  await page.getByTestId('relation-lag-inc').click();
  const saveResponse = patchAnswered();
  await page.getByTestId('relation-save').click();
  expect((await saveResponse).ok()).toBe(true);
  await expect(editor).toHaveCount(0);
  await expect(trigger).toHaveText('SS +1d');
  await expect(gantt.locator('[data-arrow][data-late]')).toHaveCount(0);

  // Reload and verify the server persisted SS +1d.
  await page.reload();
  const reloadedTrigger = gantt.locator(
    `[data-arrow-trigger][data-from="${spec}"][data-to="${impl}"]`,
  );
  await expect(reloadedTrigger).toHaveText('SS +1d');
  await expect(gantt.locator('[data-arrow][data-kind="ss"]')).toHaveCount(1);
  await expect(gantt.locator('[data-arrow][data-late]')).toHaveCount(0);

  // Delete the relation via the editor and confirm across reload.
  await reloadedTrigger.click();
  const deleteResponse = patchAnswered();
  await page.getByTestId('relation-remove').click();
  expect((await deleteResponse).ok()).toBe(true);
  await expect(gantt.locator('[data-arrow]')).toHaveCount(0);
  await page.reload();
  await expect(gantt.locator('[data-arrow]')).toHaveCount(0);
  expect(errors).toEqual([]);
  await api.dispose();
});
