/**
 * Four dependency relation kinds (`FS`, `SS`, `FF`, `SF`), lag days, arrow
 * click and keyboard editing, read-only visibility, and hierarchy collapse
 * compatibility in the Gantt view.
 */
import { expect, test } from '@playwright/test';

import { boardPath } from './address.ts';
import {
  arrowEnds,
  arrows,
  bar,
  gantt,
  goTo,
  NOW,
  openRelated,
  setAfterLinks,
  setDates,
  type Backend,
  type Dev,
} from './gantt.support.ts';
import { fitPane, paneOf } from './kanban.support.ts';
import { tabKey } from './keyboard.support.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

test('all four relation kinds connect the right bar ends and show kind/lag badges and screen-reader wording', async ({
  page,
}, info) => {
  await openRelated(page, info);
  await setDates(page, {
    'Write in the body': { from: '2026-11-10', to: '2026-11-14' },
    'Open a card': { from: '2026-11-11', to: '2026-11-15' },
    'Filter, sort and group': { from: '2026-11-12', to: '2026-11-16' },
    'Keep your documents in a wiki': { from: '2026-11-12', to: '2026-11-14' },
  });
  await setAfterLinks(page, {
    'Open a card': [{ card: 'Write in the body', kind: 'ss', lagDays: 2 }],
    'Filter, sort and group': [{ card: 'Open a card', kind: 'ff', lagDays: -1 }],
    'Keep your documents in a wiki': [{ card: 'Filter, sort and group', kind: 'sf', lagDays: 1 }],
  });

  await expect(arrows(page)).toHaveCount(3);
  const checkEnds = async (
    from: string,
    to: string,
    fromEnd: boolean,
    toStart: boolean,
  ): Promise<void> => {
    const line = await arrowEnds(page, from, to);
    const first = await bar(page, from).boundingBox();
    const second = await bar(page, to).boundingBox();
    if (!first || !second) throw new Error('missing bars');
    const expectedStart = fromEnd ? first.x + first.width : first.x;
    const expectedEnd = toStart ? second.x : second.x + second.width;
    expect(Math.abs(line.start.x - expectedStart), `${from} → ${to} start`).toBeLessThan(2);
    expect(Math.abs(line.end.x - expectedEnd), `${from} → ${to} end`).toBeLessThan(2);
  };
  await checkEnds('Write in the body', 'Open a card', false, true); // SS
  await checkEnds('Open a card', 'Filter, sort and group', true, false); // FF
  await checkEnds('Filter, sort and group', 'Keep your documents in a wiki', false, false); // SF

  // SS +2d is out of order (starts Nov 11, fewer than 2 days after Nov 10).
  await expect(bar(page, 'Open a card')).toHaveAccessibleName(
    'number 1, Open a card, November 11 – November 15. After Write in the body. Out of order: starts fewer than 2 days after the start of Write in the body.',
  );
  // Arrow button has accessible name with kind, lag, and out-of-order status.
  const ssTrigger = gantt(page).locator('[data-arrow-trigger][data-kind="ss"]');
  await expect(ssTrigger).toHaveText('SS +2d');
  await expect(ssTrigger).toHaveAccessibleName(
    'Write in the body to Open a card, Start to start, +2 days lag. Out of order.',
  );
  await expect(gantt(page).locator('[data-arrow-trigger][data-kind="ff"]')).toHaveText('FF -1d');
  await expect(gantt(page).locator('[data-arrow-trigger][data-kind="sf"]')).toHaveText('SF +1d');
});

test('clicking or keyboard-activating an arrow edits kind and lagDays, preserves hidden links, and deletes', async ({
  page,
}, info) => {
  await openRelated(page, info);
  const ids = await page.evaluate(() => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    return Object.fromEntries(store.cards.map((card) => [card.title, card.id]));
  });
  const fromId = ids['Write in the body'] ?? '';
  const toId = ids['Filter, sort and group'] ?? '';

  // Click the SVG hit path of a default FS 0 arrow to open the editor, change FS -> SS and lagDays -> +2.
  const trigger = gantt(page).locator(
    `[data-arrow-trigger][data-from="${fromId}"][data-to="${toId}"]`,
  );
  const hitPath = gantt(page).locator(`[data-arrow-hit][data-from="${fromId}"][data-to="${toId}"]`);
  await hitPath.click({ force: true });
  const editor = page.getByTestId('gantt-relation-editor');
  await expect(editor).toBeVisible();
  await editor.locator('[data-relation-kind="ss"]').click();
  await page.getByTestId('relation-lag-inc').click();
  await page.getByTestId('relation-lag-inc').click();
  await page.getByTestId('relation-save').click();
  await expect(editor).toHaveCount(0);

  await expect(trigger).toHaveText('SS +2d');
  await expect(
    gantt(page).locator(`[data-arrow][data-from="${fromId}"][data-to="${toId}"]`),
  ).toHaveAttribute('data-kind', 'ss');

  // Verify the hidden deleted card link on "Filter, sort and group" was preserved untouched.
  const storedAfterEdit = await page.evaluate((cardId) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const card = store.cards.find((each) => each.id === cardId) as unknown as {
      fields: { properties: Record<string, unknown> };
    };
    return card.fields.properties.after;
  }, toId);
  expect(storedAfterEdit).toHaveLength(2);
  expect(storedAfterEdit).toEqual(
    expect.arrayContaining([{ card: fromId, kind: 'ss', lagDays: 2 }]),
  );

  // Keyboard flow: activate with Enter and remove the relation.
  await trigger.focus();
  await expect(trigger).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(editor).toBeVisible();
  await page.getByTestId('relation-remove').click();
  await expect(editor).toHaveCount(0);
  await expect(
    gantt(page).locator(`[data-arrow][data-from="${fromId}"][data-to="${toId}"]`),
  ).toHaveCount(0);
  // Focus returns to the target card's bar after deletion.
  await expect(bar(page, 'Filter, sort and group')).toBeFocused();
  // Hidden link still preserved even after deleting the visible link.
  const storedAfterDelete = await page.evaluate((cardId) => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const card = store.cards.find((each) => each.id === cardId) as unknown as {
      fields: { properties: Record<string, unknown> };
    };
    return card.fields.properties.after;
  }, toId);
  expect(storedAfterDelete).toHaveLength(1);

  // Roving Tab stop: one Tab from a bar reaches the arrow triggers, ArrowDown walks them, Space opens editor.
  await expect(gantt(page).locator('[data-arrow-trigger][tabindex="0"]')).toHaveCount(1);
  await bar(page, 'Filter, sort and group').focus();
  await page.keyboard.press(await tabKey(page));
  await expect(gantt(page).locator('[data-arrow-trigger]').first()).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(gantt(page).locator('[data-arrow-trigger]').nth(1)).toBeFocused();
  await page.keyboard.press('Space');
  await expect(editor).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(editor).toHaveCount(0);
});

test('read-only Gantt shows non-default relation badges without edit or delete controls', async ({
  page,
}, info) => {
  await page.clock.setFixedTime(NOW);
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  await fitPane(page, paneOf(info));
  const made = await page.evaluate(async () => {
    const dev = (
      window as unknown as { fvociDev: { backend: Backend; someoneElse?: () => Backend } }
    ).fvociDev;
    const workspaces = await dev.backend.workspaces();
    const boards = await dev.backend.boards(workspaces[0]?.id ?? '');
    const boardId = boards.find((board) => board.title === 'Study notes')?.id ?? '';
    const author = dev.someoneElse?.() ?? dev.backend;
    const board = await author.container(boardId);
    const due = board.cardProperties.find((property) => property.name === 'Review on');
    if (!due) throw new Error('the board has no Review on');
    await author.patchContainer(boardId, {
      updatedCardProperties: [{ id: 'after', name: 'After', type: 'relation', options: [] }],
    });
    const first = await author.createCard(boardId, {
      title: 'Alpha',
      properties: { [due.id]: { from: '2026-11-09', to: '2026-11-12' } },
    });
    await author.createCard(boardId, {
      title: 'Beta',
      properties: {
        [due.id]: { from: '2026-11-11', to: '2026-11-14' },
        after: [{ card: first.id, kind: 'ss', lagDays: 3 }],
      },
    });
    const view = await author.createView(boardId, {
      title: 'Read only relations',
      fields: {
        viewType: 'gantt',
        dateProperty: due.id,
        dependencyPropertyId: 'after',
        ganttZoom: 'day',
        sort: [],
        shownProperties: ['__title'],
        visibleGroups: [],
        hiddenGroups: [],
        collapsedGroups: [],
        filter: { operation: 'and', filters: [] },
        cardOrder: [],
        columnWidths: {},
        columnCalculations: {},
        groupCalculations: {},
        defaultTemplateId: '',
      },
    });
    return { boardId, viewId: view.id };
  });
  await goTo(page, boardPath(made.boardId, made.viewId));
  await expect(arrows(page)).toHaveCount(1);
  await expect(gantt(page).locator('[data-arrow-trigger]')).toHaveCount(0);
  await expect(gantt(page).locator('[data-arrow-hit]')).toHaveCount(0);
  const badge = gantt(page).locator('[data-arrow-badge]');
  await expect(badge).toHaveText('SS +3d');
  await expect(badge).toHaveAccessibleName(
    'Alpha to Beta, Start to start, +3 days lag. Out of order.',
  );
  await expect(page.getByTestId('gantt-relation-editor')).toHaveCount(0);
});

test('collapsing a parent row while the relation editor is open closes it cleanly and hides sub-card arrows', async ({
  page,
}, info) => {
  await openRelated(page, info);
  const ids = await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace
      .board as Dev['workspace']['board'] & {
      changeCard(
        cardId: string,
        fn: (card: { id: string; title: string; parentId?: string }) => {
          id: string;
          title: string;
          parentId?: string;
        },
      ): Promise<void>;
    };
    const parent = store.cards.find((card) => card.title === 'Write in the body');
    const child = store.cards.find((card) => card.title === 'Filter, sort and group');
    if (!parent || !child) throw new Error('missing cards');
    await store.changeCard(child.id, (card) => ({ ...card, parentId: parent.id }));
    return { parentId: parent.id, childId: child.id };
  });

  const hitPath = gantt(page).locator(
    `[data-arrow-hit][data-from="${ids.parentId}"][data-to="${ids.childId}"]`,
  );
  await expect(hitPath).toHaveCount(1);
  await hitPath.click({ force: true });
  await expect(page.getByTestId('gantt-relation-editor')).toBeVisible();

  // Collapse the parent row directly while the editor is open (works on both popover and mobile sheet).
  const toggle = gantt(page).locator(`[data-hierarchy-toggle="${ids.parentId}"]`);
  await toggle.evaluate((button: HTMLElement) => {
    button.click();
  });
  await expect(page.getByTestId('gantt-relation-editor')).toHaveCount(0);
  await expect(
    gantt(page).locator(`[data-arrow][data-from="${ids.parentId}"][data-to="${ids.childId}"]`),
  ).toHaveCount(0);

  // Expanding restores the child bar and its arrows.
  await toggle.click();
  await expect(
    gantt(page).locator(`[data-arrow][data-from="${ids.parentId}"][data-to="${ids.childId}"]`),
  ).toHaveCount(1);
});
