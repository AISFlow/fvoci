/**
 * What the browser tests of the Gantt view share: a Gantt view of the
 * welcome board with dated cards (and a card relation between them), the
 * parts of the chart on the page, and reading days off the marks of its axis.
 *
 * The clock of the page is fixed at 2026-11-10 01:00 in Seoul (still the
 * 9th in UTC, so that a day taken from UTC would show), and so is its time
 * zone (`test.use` in each file); the language is English, whose weeks
 * start on Sunday.
 */
import { expect, type Locator, type Page, type TestInfo } from '@playwright/test';

import { viewIdOf } from './address.ts';
import { openView } from './kanban.support.ts';

export const WELCOME = ['Getting started with fvoci', 'Board'] as const;
export const NOW = new Date('2026-11-09T16:00:00Z'); // 01:00 on the 10th in Seoul
export const gantt = (page: Page): Locator => page.getByTestId('gantt');
export const scroller = (page: Page): Locator => page.getByTestId('gantt-scroller');
export const bars = (page: Page): Locator => gantt(page).locator('[data-bar]');
export const bar = (page: Page, title: string): Locator =>
  gantt(page)
    .locator('[data-row]')
    .filter({ has: page.locator('[data-row-title]', { hasText: title }) })
    .locator('[data-bar]');

/** A date value as the wire has it: a day as text, a moment as milliseconds. */
export type Stored = { from: string | number; to?: string | number } | undefined;

export interface Store {
  boardId: string;
  board: { cardProperties: { id: string; name: string; type: string }[] };
  cards: { id: string; title: string }[];
  views: { id: string; title: string; fields: Record<string, unknown> }[];
  setCardProperty(cardId: string, propertyId: string, value: unknown): Promise<void>;
  setViewFields(viewId: string, fields: Record<string, unknown>): Promise<void>;
  changeBoard(change: (board: Store['board']) => Store['board']): Promise<void>;
  deleteCards(cardIds: string[]): Promise<void>;
  createCard(draft: { title: string }): Promise<{ id: string } | undefined>;
}
export interface Dev {
  workspace: {
    board: Store;
    changeSettings(patch: Record<string, string>): Promise<void>;
    undoLast(): Promise<void>;
  };
  backend: {
    views(boardId: string): Promise<{ id: string; fields: Record<string, unknown> }[]>;
  };
}

/** Gives cards of the open board a value of "Day", through the board store. */
export async function setDates(page: Page, values: Record<string, Stored>): Promise<void> {
  await page.evaluate(async (wanted) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((property) => property.name === 'Day');
    for (const [title, value] of Object.entries(wanted)) {
      const card = store.cards.find((each) => each.title === title);
      if (!card || !due) throw new Error(`no card "${title}" or no Day`);
      await store.setCardProperty(card.id, due.id, value);
    }
  }, values);
}

/** Clears "Day" on every card and removes pre-existing relation properties on the open board. */
export async function clearDates(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((property) => property.name === 'Day');
    if (due) {
      const cards = store.cards as unknown as {
        id: string;
        fields: { properties: Record<string, unknown> };
      }[];
      for (const card of cards) {
        if (card.fields.properties[due.id] !== undefined) {
          await store.setCardProperty(card.id, due.id, undefined);
        }
      }
    }
    if (store.board.cardProperties.some((property) => property.type === 'relation')) {
      await store.changeBoard((board) => ({
        ...board,
        cardProperties: board.cardProperties.filter((property) => property.type !== 'relation'),
      }));
    }
  });
}

/** The fields of the view that is open, as the backend keeps them. */
export async function storedFields(page: Page): Promise<Record<string, unknown>> {
  const viewId = viewIdOf(page.url());
  return page.evaluate(async (id) => {
    const { workspace, backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const views = await backend.views(workspace.board.boardId);
    return views.find((view) => view.id === id)?.fields ?? {};
  }, viewId);
}

/** Adds a Gantt view to the board that is open, from the list of kinds. */
export async function addGantt(page: Page): Promise<void> {
  await page.getByTestId('view-header').locator('[data-tool="views"]').click();
  await page.getByRole('menuitem', { name: 'Add Gantt view' }).click();
  await expect(gantt(page)).toBeVisible();
  await expect(scroller(page)).toBeVisible();
}

/** A Gantt view of the welcome board with a few dated cards. */
export async function openGantt(page: Page, info: TestInfo): Promise<void> {
  await page.clock.setFixedTime(NOW);
  await openView(page, info, ...WELCOME);
  await clearDates(page);
  await setDates(page, {
    'Open a card': { from: '2026-11-12' },
    // Twelve days: 12 to 23 November.
    'Filter, sort and group': { from: '2026-11-12', to: '2026-11-23' },
    // 08:00 on the 10th in Seoul.
    'Write in the body': { from: Date.parse('2026-11-09T23:00:00Z') },
    'Keep your documents in a wiki': { from: '2026-12-03', to: '2026-12-04' },
  });
  await addGantt(page);
  await expect(bars(page)).toHaveCount(4);
}

/** Where a day begins across the page, and how wide a day is: read off the marks of the axis. */
export async function dayScale(
  page: Page,
): Promise<{ x: (date: string) => number; width: number }> {
  const marks = await gantt(page)
    .locator('[data-tick]')
    .evaluateAll((ticks) =>
      ticks.slice(0, 2).map((tick) => ({
        date: tick.getAttribute('data-tick') ?? '',
        x: tick.getBoundingClientRect().x,
      })),
    );
  const [first, second] = marks;
  if (!first || !second) throw new Error('the axis has no marks');
  const days = (from: string, to: string): number =>
    Math.round((Date.parse(to) - Date.parse(from)) / 86_400_000);
  const width = (second.x - first.x) / days(first.date, second.date);
  return { x: (date) => first.x + days(first.date, date) * width, width };
}

// ------------------------------------------------------- dependencies

export const arrows = (page: Page): Locator => gantt(page).locator('[data-arrow]');

/** Gives the board a card relation "After" (identifier `after`). */
export async function addRelation(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    await store.changeBoard((board) => ({
      ...board,
      cardProperties: [
        ...board.cardProperties,
        { id: 'after', name: 'After', type: 'relation', options: [] },
      ],
    }));
  });
}

/** Writes on each card named the cards it comes after, by their titles; other text is kept as an identifier. */
export async function setAfter(page: Page, values: Record<string, string[]>): Promise<void> {
  await setAfterLinks(
    page,
    Object.fromEntries(
      Object.entries(values).map(([title, before]) => [
        title,
        before.map((card) => ({ card, kind: 'fs' as const, lagDays: 0 })),
      ]),
    ),
  );
}

/** Writes typed relation links (`kind` + `lagDays`) on each named card. */
export async function setAfterLinks(
  page: Page,
  values: Record<string, { card: string; kind: 'fs' | 'ss' | 'ff' | 'sf'; lagDays: number }[]>,
): Promise<void> {
  await page.evaluate(async (wanted) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const idOf = (title: string): string =>
      store.cards.find((each) => each.title === title)?.id ?? title;
    for (const [title, links] of Object.entries(wanted)) {
      await store.setCardProperty(
        idOf(title),
        'after',
        links.map((link) => ({ card: idOf(link.card), kind: link.kind, lagDays: link.lagDays })),
      );
    }
  }, values);
}

/**
 * Chooses a line of "dependencies": the tool is behind "…" of the header, in
 * either layout. The line of "none" is `(none)`. When the list has closed,
 * the focus is back on "…", where the tool was opened from.
 */
export async function chooseDependencies(page: Page, action: string): Promise<void> {
  const more = page.getByTestId('view-header').locator('[data-tool="more"]');
  await more.click();
  await page.locator('[role="menuitem"][data-action="dependencies"]').click();
  await page.locator(`[role="menuitem"][data-action="${action}"]`).click();
  await expect(page.locator('[role="menuitem"]')).toHaveCount(0);
  await expect(more).toBeFocused();
}

/** What "dependencies" says it reads, on its line of "…". */
export async function expectDependencies(page: Page, name: string): Promise<void> {
  await page.getByTestId('view-header').locator('[data-tool="more"]').click();
  const line = page.locator('[role="menuitem"][data-action="dependencies"]');
  await expect(line).toContainText('Dependencies');
  await expect(line).toContainText(name);
  await page.keyboard.press('Escape');
  await expect(line).toHaveCount(0);
}

/** A Gantt view of the welcome board with dated cards and a relation between them. */
export async function openRelated(page: Page, info: TestInfo): Promise<void> {
  await page.clock.setFixedTime(NOW);
  await openView(page, info, ...WELCOME);
  await clearDates(page);
  await setDates(page, {
    'Open a card': { from: '2026-11-12' },
    'Filter, sort and group': { from: '2026-11-12', to: '2026-11-23' },
    'Write in the body': { from: Date.parse('2026-11-09T23:00:00Z') },
    'Keep your documents in a wiki': { from: '2026-12-03', to: '2026-12-04' },
  });
  await addRelation(page);
  await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    await store.createCard({ title: 'A card that is gone' });
  });
  await setAfter(page, {
    // After two cards, one of them deleted below: its identifier stays in the value.
    'Filter, sort and group': ['Write in the body', 'A card that is gone'],
    'Keep your documents in a wiki': ['Filter, sort and group', 'Open a card'],
    // A card without a date: it has no bar for an arrow to reach.
    'Attach files and open them right here': ['Open a card'],
  });
  await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const gone = store.cards.find((card) => card.title === 'A card that is gone');
    if (!gone) throw new Error('no card to delete');
    await store.deleteCards([gone.id]);
  });
  await addGantt(page);
  await expect(bars(page)).toHaveCount(4);
}

/** The first and the last point of the line of the arrow between two cards, on the page. */
export async function arrowEnds(
  page: Page,
  from: string,
  to: string,
): Promise<{ start: { x: number; y: number }; end: { x: number; y: number } }> {
  const ids = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    return Object.fromEntries(store.cards.map((card) => [card.title, card.id]));
  });
  return gantt(page)
    .locator(`[data-arrow][data-from="${ids[from] ?? ''}"][data-to="${ids[to] ?? ''}"] path`)
    .first()
    .evaluate((path) => {
      const drawn = path as SVGPathElement;
      const matrix = drawn.getScreenCTM();
      if (!matrix) throw new Error('the arrow is not drawn');
      const at = (length: number): { x: number; y: number } => {
        const point = drawn.getPointAtLength(length).matrixTransform(matrix);
        return { x: point.x, y: point.y };
      };
      return { start: at(0), end: at(drawn.getTotalLength()) };
    });
}

/** What the tests ask of the backend in memory, beside the page. */
export interface Backend {
  latency: number;
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  container(containerId: string): Promise<{ cardProperties: { id: string; name: string }[] }>;
  patchContainer(containerId: string, patch: Record<string, unknown>): Promise<unknown>;
  createCard(boardId: string, draft: Record<string, unknown>): Promise<{ id: string }>;
  createView(boardId: string, draft: Record<string, unknown>): Promise<{ id: string }>;
}

/** Goes to an address of the app the way a link does: the data in memory stays as it is. */
export async function goTo(page: Page, address: string): Promise<void> {
  await page.evaluate((path) => {
    history.pushState(null, '', path);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, address);
}
