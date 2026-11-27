/**
 * The dependencies of a Gantt view on the in-memory backend: the card
 * relation the header picks, one arrow for each edge, from the end of one
 * bar to the start of the other, and what is out of order or on a loop.
 */
import { expect, test, type Page } from '@playwright/test';

import { viewIdOf } from './address.ts';
import {
  addRelation,
  arrowEnds,
  arrows,
  bar,
  bars,
  chooseDependencies,
  expectDependencies,
  gantt,
  openGantt,
  openRelated,
  setAfter,
  setDates,
  storedFields,
  type Dev,
} from './gantt.support.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

test('a new Gantt view reads the first card relation, and draws one arrow for each edge', async ({
  page,
}, info) => {
  await openRelated(page, info);
  await expectDependencies(page, 'After');
  expect((await storedFields(page)).dependencyPropertyId).toBe('after');
  // Three edges between cards that have a bar; the deleted card and the card
  // without a date give none.
  await expect(gantt(page)).toHaveAttribute('data-edges', '3');
  await expect(arrows(page)).toHaveCount(3);
  const ends = await arrows(page).evaluateAll((drawn) =>
    drawn.map((arrow) => [arrow.getAttribute('data-from'), arrow.getAttribute('data-to')]),
  );
  const ids = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    return Object.fromEntries(store.cards.map((card) => [card.title, card.id]));
  });
  expect(ends).toEqual([
    [ids['Write in the body'], ids['Filter, sort and group']],
    [ids['Filter, sort and group'], ids['Keep your documents in a wiki']],
    [ids['Open a card'], ids['Keep your documents in a wiki']],
  ]);
  // A bar says which cards it comes after.
  await expect(bar(page, 'Keep your documents in a wiki')).toHaveAccessibleName(
    'number 7, Keep your documents in a wiki, December 3 – December 4. After Filter, sort and group and Open a card.',
  );
  await expect(bar(page, 'Open a card')).toHaveAccessibleName('number 1, Open a card, November 12');
});

test('an arrow goes from the end of the card that comes first to the start of the one after', async ({
  page,
}, info) => {
  await openRelated(page, info);
  await expect(arrows(page)).toHaveCount(3);
  /** The arrow between two cards leaves the end of the first bar and reaches the start of the second. */
  async function joins(from: string, to: string): Promise<void> {
    await bar(page, to).scrollIntoViewIfNeeded();
    const line = await arrowEnds(page, from, to);
    const first = await bar(page, from).boundingBox();
    const second = await bar(page, to).boundingBox();
    if (!first || !second) throw new Error('no bars to measure');
    expect(Math.abs(line.start.x - (first.x + first.width)), `${from} → ${to}`).toBeLessThan(1.5);
    expect(Math.abs(line.start.y - (first.y + first.height / 2))).toBeLessThan(1.5);
    expect(Math.abs(line.end.x - second.x)).toBeLessThan(1.5);
    expect(Math.abs(line.end.y - (second.y + second.height / 2))).toBeLessThan(1.5);
  }
  // One that turns back (the second card starts two days after the first ends, less than
  // two gaps at this zoom), and one that goes straight in, two rows down.
  await joins('Write in the body', 'Filter, sort and group');
  await joins('Open a card', 'Keep your documents in a wiki');
  // Closer in, with a heading of a group between the rows, the arrows still meet the bars.
  await page.getByTestId('gantt-zoom-in').click();
  await expect(gantt(page)).toHaveAttribute('data-zoom', 'day');
  await page.evaluate(async (id) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const select = store.board.cardProperties.find((property) => property.type === 'select');
    if (!select) throw new Error('nothing to group by');
    await store.setViewFields(id, { groupBy: select.id });
  }, viewIdOf(page.url()));
  await expect(gantt(page).locator('[data-group]').first()).toBeVisible();
  await joins('Write in the body', 'Filter, sort and group');
  await joins('Filter, sort and group', 'Keep your documents in a wiki');
});

test('"dependencies" in the header turns the arrows off and on, and the view keeps the choice', async ({
  page,
}, info) => {
  await openRelated(page, info);
  await expect(arrows(page)).toHaveCount(3);
  await chooseDependencies(page, '(none)');
  await expect(arrows(page)).toHaveCount(0);
  await expect(page.getByTestId('gantt-arrows')).toHaveCount(0);
  await expectDependencies(page, 'None');
  await expect.poll(async () => (await storedFields(page)).dependencyPropertyId).toBe('');
  await chooseDependencies(page, 'after');
  await expect(arrows(page)).toHaveCount(3);
  await expect.poll(async () => (await storedFields(page)).dependencyPropertyId).toBe('after');
  // One undo takes the last choice back.
  await page.evaluate(() => (window as unknown as { fvociDev: Dev }).fvociDev.workspace.undoLast());
  await expect(arrows(page)).toHaveCount(0);
  await expect.poll(async () => (await storedFields(page)).dependencyPropertyId).toBe('');
});

test('a Gantt view made on a board without a card relation has no arrows until one is chosen', async ({
  page,
}, info) => {
  await openGantt(page, info);
  await expectDependencies(page, 'None');
  expect((await storedFields(page)).dependencyPropertyId).toBeUndefined();
  await addRelation(page);
  await setAfter(page, { 'Filter, sort and group': ['Write in the body'] });
  await expect(arrows(page)).toHaveCount(0);
  await chooseDependencies(page, 'after');
  await expect(arrows(page)).toHaveCount(1);
  await expect(gantt(page)).toHaveAttribute('data-edges', '1');
  await expect.poll(async () => (await storedFields(page)).dependencyPropertyId).toBe('after');
  // The first choice taken back: the view names no relation again, as before it.
  await page.evaluate(() => (window as unknown as { fvociDev: Dev }).fvociDev.workspace.undoLast());
  await expect(arrows(page)).toHaveCount(0);
  await expectDependencies(page, 'None');
  await expect.poll(async () => 'dependencyPropertyId' in (await storedFields(page))).toBe(false);
});

test('the arrows of a card go with it when it is deleted', async ({ page }, info) => {
  await openRelated(page, info);
  await expect(arrows(page)).toHaveCount(3);
  await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const gone = store.cards.find((card) => card.title === 'Filter, sort and group');
    if (!gone) throw new Error('no card to delete');
    await store.deleteCards([gone.id]);
  });
  // The value of "Keep your documents in a wiki" still names the deleted card: no arrow for it.
  await expect(bars(page)).toHaveCount(3);
  await expect(arrows(page)).toHaveCount(1);
  await expect(gantt(page)).toHaveAttribute('data-edges', '1');
});

// ------------------------------------------- out of order, and loops

async function dashes(page: Page, from: string, to: string): Promise<string | null> {
  const ids = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    return Object.fromEntries(store.cards.map((card) => [card.title, card.id]));
  });
  return gantt(page)
    .locator(`[data-arrow][data-from="${ids[from] ?? ''}"][data-to="${ids[to] ?? ''}"] path`)
    .first()
    .getAttribute('stroke-dasharray');
}

test('a card that starts before a card it comes after has ended is marked, counted and said', async ({
  page,
}, info) => {
  await openRelated(page, info);
  // "Filter, sort and group" starts on the day "Open a card" ends: that is in order.
  await setAfter(page, { 'Filter, sort and group': ['Write in the body', 'Open a card'] });
  await expect(arrows(page)).toHaveCount(4);
  await expect(gantt(page)).toHaveAttribute('data-late', '0');
  await expect(page.getByTestId('gantt-late')).toHaveCount(0);
  await expect(gantt(page).locator('[data-mark]')).toHaveCount(0);

  // "Keep your documents in a wiki" now starts three days before "Filter, sort and group" ends.
  await setDates(page, {
    'Keep your documents in a wiki': { from: '2026-11-20', to: '2026-11-21' },
  });
  await expect(gantt(page)).toHaveAttribute('data-late', '1');
  await expect(arrows(page)).toHaveCount(4);
  await expect(gantt(page).locator('[data-arrow][data-late]')).toHaveCount(1);
  // Not by colour alone: the line is dashed, the others are not. The dashes
  // are in pixels, as the thickness of the line is, whatever the drawing is stretched to.
  expect(await dashes(page, 'Filter, sort and group', 'Keep your documents in a wiki')).toBe('6 4');
  expect(await dashes(page, 'Open a card', 'Keep your documents in a wiki')).toBeNull();
  await expect(gantt(page).locator('[data-arrow][data-late] path').first()).toHaveAttribute(
    'vector-effect',
    'non-scaling-stroke',
  );
  // A mark next to the title, which stays in sight while the days scroll.
  const row = gantt(page)
    .locator('[data-row]')
    .filter({
      has: page.locator('[data-row-title]', { hasText: 'Keep your documents in a wiki' }),
    });
  await expect(row.locator('[data-row-title] [data-mark="late"]')).toBeVisible();
  await expect(gantt(page).locator('[data-mark]')).toHaveCount(1);
  await expect(bar(page, 'Keep your documents in a wiki')).toHaveAccessibleName(
    'number 7, Keep your documents in a wiki, November 20 – November 21. After Filter, sort and group and Open a card. Out of order: starts before the end of Filter, sort and group.',
  );
  // The row above the chart counts them, and goes to the card.
  const count = page.getByTestId('gantt-late');
  await expect(count).toHaveAccessibleName('Out of order: 1. Show the next.');
  await expect(count.locator('[data-count]')).toHaveText('1');
  await count.click();
  await expect(bar(page, 'Keep your documents in a wiki')).toBeFocused();
  await expect(bar(page, 'Keep your documents in a wiki')).toBeInViewport();

  // What changes is said to a screen reader, which sees no button come or go.
  await expect(page.getByTestId('gantt-problems')).toHaveText('Out of order: 1. On a loop: 0.');

  // Dates are not moved for the reader: the card is where its value says.
  const stored = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((property) => property.name === 'Day');
    const card = store.cards.find((each) => each.title === 'Keep your documents in a wiki');
    return (card as unknown as { fields: { properties: Record<string, unknown> } }).fields
      .properties[due?.id ?? ''];
  });
  expect(stored).toEqual({ from: '2026-11-20', to: '2026-11-21' });

  // Earlier still, it starts before both cards it comes after have ended: two
  // relations out of order, on one card. The count is of relations.
  await setDates(page, { 'Keep your documents in a wiki': { from: '2026-11-11' } });
  await expect(gantt(page)).toHaveAttribute('data-late', '2');
  await expect(count).toHaveAccessibleName('Out of order: 2. Show the next.');
  await expect(gantt(page).locator('[data-arrow][data-late]')).toHaveCount(2);
  await expect(gantt(page).locator('[data-bar][data-late]')).toHaveCount(1);
  await expect(gantt(page).locator('[data-mark]')).toHaveCount(1);
  await expect(bar(page, 'Keep your documents in a wiki')).toHaveAccessibleName(
    'number 7, Keep your documents in a wiki, November 11. After Filter, sort and group and Open a card. Out of order: starts before the end of Filter, sort and group and Open a card.',
  );

  // Put back in order while the count has the focus: the count stays, at
  // nothing, for as long as it has it, and then goes. Nothing is marked.
  await count.focus();
  await setDates(page, {
    'Keep your documents in a wiki': { from: '2026-11-23', to: '2026-11-24' },
  });
  await expect(gantt(page)).toHaveAttribute('data-late', '0');
  await expect(page.getByTestId('gantt-problems')).toHaveText('Out of order: 0. On a loop: 0.');
  await expect(count).toBeFocused();
  await expect(count.locator('[data-count]')).toHaveText('0');
  await page.getByTestId('gantt-today').focus();
  await expect(page.getByTestId('gantt-late')).toHaveCount(0);
  await expect(gantt(page).locator('[data-mark]')).toHaveCount(0);
});

test('"show the next" goes on from the bar the reader is on, and shows which bar it means', async ({
  page,
}, info) => {
  await openRelated(page, info);
  // Out of order: the first row ("Open a card", after "Filter, sort and group", which ends later)
  // and the last ("Keep your documents in a wiki", moved to before "Filter, sort and group" ends).
  await setAfter(page, { 'Open a card': ['Filter, sort and group'] });
  await setDates(page, {
    'Keep your documents in a wiki': { from: '2026-11-20', to: '2026-11-21' },
  });
  const count = page.getByTestId('gantt-late');
  await expect(count).toHaveAccessibleName('Out of order: 2. Show the next.');
  const late = gantt(page).locator('[data-bar][data-late]');
  await expect(late).toHaveCount(2);
  const rows = await bars(page).evaluateAll((all) =>
    all.map((each) => each.hasAttribute('data-late')),
  );
  expect(rows[0]).toBe(true);
  expect(rows.at(-1)).toBe(true);
  expect(rows.slice(1, -1)).toEqual([false, false]);

  // From a bar between the two that is not out of order: on to the one below, not back to the top.
  await bars(page).nth(1).focus();
  await count.click();
  await expect(late.last()).toBeFocused();
  // Pressed with a pointer, the bar still shows which one is meant.
  const ring = (): Promise<string> =>
    late.last().evaluate((element) => getComputedStyle(element).boxShadow);
  expect(await ring()).not.toBe('none');
  // From the last, round to the first.
  await count.click();
  await expect(late.first()).toBeFocused();
});

test('cards on a loop are marked, counted and said, the arrows between them dotted', async ({
  page,
}, info) => {
  await openRelated(page, info);
  await expect(page.getByTestId('gantt-loops')).toHaveCount(0);
  // "Write in the body" → "Filter, sort and group" → "Keep your documents in a wiki" → "Write in the body".
  await setAfter(page, { 'Write in the body': ['Keep your documents in a wiki'] });
  await expect(arrows(page)).toHaveCount(4);
  await expect(gantt(page).locator('[data-arrow][data-loop]')).toHaveCount(3);
  // In order and on the loop: dotted. The one that closes it runs backwards in time: a dot after each dash.
  expect(await dashes(page, 'Write in the body', 'Filter, sort and group')).toBe('0.1 4.5');
  expect(await dashes(page, 'Keep your documents in a wiki', 'Write in the body')).toBe(
    '6 4.5 0.1 4.5',
  );
  // A dot is a dash of no length with a round end.
  await expect(gantt(page).locator('[data-arrow][data-loop] path').first()).toHaveAttribute(
    'stroke-linecap',
    'round',
  );
  expect(await dashes(page, 'Open a card', 'Keep your documents in a wiki')).toBeNull();
  await expect(gantt(page).locator('[data-bar][data-loop]')).toHaveCount(3);
  // The mark of going round is by the title of each card of the loop.
  await expect(gantt(page).locator('[data-row-title] [data-mark="loop"]')).toHaveCount(3);
  await expect(page.getByTestId('gantt-problems')).toHaveText('Out of order: 1. On a loop: 3.');
  await expect(bar(page, 'Open a card')).not.toHaveAttribute('data-loop');
  await expect(bar(page, 'Filter, sort and group')).toHaveAccessibleName(
    'number 4, Filter, sort and group, November 12 – November 23. After Write in the body. On a loop: following what it comes after leads back to it.',
  );
  const count = page.getByTestId('gantt-loops');
  await expect(count).toHaveAccessibleName('On a loop: 3. Show the next.');
  await expect(page.getByTestId('gantt-late')).toHaveAccessibleName(
    'Out of order: 1. Show the next.',
  );
  // The button walks the cards of the loop, from the top and round again.
  const onLoop = gantt(page).locator('[data-bar][data-loop]');
  for (const at of [0, 1, 2, 0]) {
    await count.click();
    await expect(onLoop.nth(at)).toBeFocused();
  }
});

test('a loop through a card without a date still marks the cards that have a bar', async ({
  page,
}, info) => {
  await openRelated(page, info);
  // "Open a card" ↔ "Attach files and open them right here", which has no date and so no bar and no arrow.
  await setAfter(page, { 'Open a card': ['Attach files and open them right here'] });
  await expect(arrows(page)).toHaveCount(3);
  await expect(gantt(page).locator('[data-arrow][data-loop]')).toHaveCount(0);
  await expect(bar(page, 'Open a card')).toHaveAttribute('data-loop', '');
  // The title and the days are a sentence of their own before what follows.
  await expect(bar(page, 'Open a card')).toHaveAccessibleName(
    'number 1, Open a card, November 12. On a loop: following what it comes after leads back to it.',
  );
  await expect(page.getByTestId('gantt-loops')).toHaveAccessibleName(
    'On a loop: 1. Show the next.',
  );
});
