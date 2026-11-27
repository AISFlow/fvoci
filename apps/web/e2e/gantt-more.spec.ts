/**
 * What a Gantt view does besides its bars, on the in-memory backend: the
 * header's "New" makes a card on a day of the chart; a card cannot be made
 * to come after a card that comes after it; a loop of cards without a date
 * is shown; a bar says its group; the counts above the chart say their
 * words on a narrow page too.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { viewIdOf } from './address.ts';
import {
  bar,
  dayScale,
  gantt,
  openGantt,
  openRelated,
  scroller,
  setAfter,
  type Dev,
} from './gantt.support.ts';
import { dialog } from './kanban.support.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

/** The header's "New": the button of the header, or its round one on a folded header. */
const newCard = (page: Page): Locator => page.locator('[data-testid="new-card"]:visible').first();

/** The titles and "Due Date" of the cards of the open board, as the store has them. */
async function dues(page: Page): Promise<Map<string, unknown>> {
  const pairs = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((each) => each.name === 'Day')?.id ?? '';
    return (
      store.cards as unknown as { id: string; fields: { properties: Record<string, unknown> } }[]
    ).map((card) => [card.id, card.fields.properties[due]] as const);
  });
  return new Map(pairs);
}

/** The card made since `before`, and its "Due Date". */
async function made(page: Page, before: Map<string, unknown>): Promise<unknown> {
  let value: unknown;
  await expect
    .poll(async () => {
      const now = await dues(page);
      const fresh = [...now.keys()].filter((id) => !before.has(id));
      value = fresh.length === 1 ? now.get(fresh[0] ?? '') : undefined;
      return fresh.length;
    })
    .toBe(1);
  return value;
}

test('the header’s “New” makes a card of one day on today, or on the first day in sight', async ({
  page,
}, info) => {
  await openGantt(page, info);
  let before = await dues(page);
  await newCard(page).click();
  await expect(dialog(page)).toBeVisible();
  expect(await made(page, before)).toEqual({ from: '2026-11-10' });
  // The first Escape leaves the title the new card asks for, the second closes the card.
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toHaveCount(0);

  // Today out of sight: the first day wholly in sight.
  await scroller(page).evaluate((element) => {
    element.scrollLeft = element.scrollWidth;
  });
  const first = (await gantt(page).getAttribute('data-first')) ?? '';
  const { x, width } = await dayScale(page);
  const titles = await gantt(page).locator('[data-gantt-corner]').boundingBox();
  if (!titles) throw new Error('no column of titles');
  const into = Math.ceil((titles.x + titles.width - x(first)) / width - 0.01);
  const expected = new Date(Date.parse(`${first}T00:00:00Z`) + into * 86_400_000)
    .toISOString()
    .slice(0, 10);
  before = await dues(page);
  await newCard(page).click();
  await expect(dialog(page)).toBeVisible();
  expect(expected > '2026-11-10').toBe(true);
  expect(await made(page, before)).toEqual({ from: expected });
});

test('a card cannot be made to come after a card that comes after it, and it says why', async ({
  page,
}, info) => {
  await openRelated(page, info);
  // "Keep your documents in a wiki" and "Attach files and open them right here" come after "Open a card".
  await bar(page, 'Open a card').click();
  await expect(dialog(page)).toBeVisible();
  const after = dialog(page)
    .locator('[data-property]')
    .filter({ has: page.locator('dt', { hasText: /^\s*After\s*$/ }) });
  await after.locator('[data-slot="relation-edit"]').click();
  const search = page.getByRole('combobox', { name: 'After' });
  await search.fill('Keep your documents');
  await expect(page.getByRole('listbox', { name: 'After' }).getByRole('option')).toHaveCount(0);
  await expect(page.getByText('Not offered: 1. Picking them would make a loop.')).toBeVisible();
  // A card that closes no loop is offered.
  await search.fill('Filter, sort and group');
  await expect(page.getByRole('listbox', { name: 'After' }).getByRole('option')).toHaveCount(1);
});

test('a loop of cards without a date is counted, and they are marked in their list', async ({
  page,
}, info) => {
  await openRelated(page, info);
  // Two cards without a date, the one that came after "Open a card" among them, after each other.
  const titles = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((each) => each.name === 'Day')?.id ?? '';
    const cards = store.cards as unknown as {
      title: string;
      fields: { isTemplate: boolean; properties: Record<string, unknown> };
    }[];
    return cards
      .filter(
        (card) => !card.fields.isTemplate && card.title !== 'Attach files and open them right here',
      )
      .filter((card) => card.fields.properties[due] === undefined && card.title !== '')
      .map((card) => card.title);
  });
  const other = titles[0] ?? '';
  await setAfter(page, {
    'Attach files and open them right here': [other],
    [other]: ['Attach files and open them right here'],
  });
  const loops = page.getByTestId('gantt-loops');
  // No card with a bar is on a loop: the two without a date are counted, and shown.
  await expect(loops).toHaveAccessibleName('On a loop without a date: 2. Show them.');
  await loops.click();
  const list = page.getByTestId('undated-list');
  await expect(list).toBeVisible();
  await expect(
    list
      .locator('[data-card]')
      .filter({ has: page.locator('[data-mark="loop"]') })
      .first()
      .locator('a'),
  ).toBeFocused();
  for (const title of ['Attach files and open them right here', other]) {
    await expect(
      list.locator('[data-card]').filter({ hasText: title }).locator('[data-mark="loop"]'),
    ).toHaveText('On a loop');
  }
  await expect(list.locator('[data-mark="loop"]')).toHaveCount(2);
});

test('cards without a date on a loop are told on "No date" while cards with a bar are counted', async ({
  page,
}, info) => {
  await openRelated(page, info);
  const other = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((each) => each.name === 'Day')?.id ?? '';
    const cards = store.cards as unknown as {
      title: string;
      fields: { isTemplate: boolean; properties: Record<string, unknown> };
    }[];
    return (
      cards
        .filter(
          (card) =>
            !card.fields.isTemplate && card.title !== 'Attach files and open them right here',
        )
        .find((card) => card.fields.properties[due] === undefined && card.title !== '')?.title ?? ''
    );
  });
  const undated = page.getByTestId('gantt-undated');
  await setAfter(page, {
    'Attach files and open them right here': [other],
    [other]: ['Attach files and open them right here'],
  });
  await expect(undated.locator('[data-mark="loop"]')).toHaveCount(0);
  // A loop of two cards with a bar as well: those are counted, and "No date" tells of its own.
  await setAfter(page, { 'Write in the body': ['Keep your documents in a wiki'] });
  await expect(page.getByTestId('gantt-loops')).toHaveAccessibleName(
    /^On a loop: \d+\. Show the next\.$/,
  );
  await expect(undated).toHaveAccessibleName(/^No date: \d+\. On a loop among them: 2\.$/);
  await expect(undated.locator('[data-mark="loop"]')).toBeVisible();
});

test('the counts above the chart say what they count in words, on a narrow page too', async ({
  page,
}, info) => {
  await openRelated(page, info);
  // "Open a card" (the 12th) after "Filter, sort and group" (to the 23rd): out of order.
  await setAfter(page, { 'Open a card': ['Filter, sort and group'] });
  const late = page.getByTestId('gantt-late');
  await expect(late).toHaveAccessibleName('Out of order: 1. Show the next.');
  await expect(late.getByText('Out of order', { exact: true })).toBeVisible();
  await setAfter(page, { 'Write in the body': ['Keep your documents in a wiki'] });
  await expect(
    page.getByTestId('gantt-loops').getByText('On a loop', { exact: true }),
  ).toBeVisible();
});

test('a bar of a grouped chart says its group', async ({ page }, info) => {
  await openGantt(page, info);
  await page.evaluate(async (viewId) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const found = store.board.cardProperties.find((each) => each.type === 'select');
    if (!found) throw new Error('no select property');
    await store.setViewFields(viewId, { groupBy: found.id });
  }, viewIdOf(page.url()));
  const heading = gantt(page).locator('[data-group]').first();
  await expect(heading).toBeVisible();
  // Each bar ends by naming the heading it is under.
  const name = (await bar(page, 'Open a card').getAttribute('aria-label')) ?? '';
  const groups = await gantt(page)
    .locator('[data-group] [data-slot="group-name"]')
    .allTextContents();
  expect(groups.some((group) => name.endsWith(`Group: ${group.trim()}.`))).toBe(true);
  expect(name.startsWith('number 1, Open a card, November 12. ')).toBe(true);
});
