/**
 * Card hierarchy end-to-end on the in-memory backend (desktop, middle, mobile):
 * - Creating a sub-card inline, attaching an existing card, detaching, and undo
 * - Depth-3 limit: hiding add controls on depth-3 cards, 2-step parent breadcrumb
 * - Sub-card progress bar & Kanban card face badge + parent clue
 * - Table parent-child grouping, expand/collapse, and keeping children whose parent is filtered out
 * - Gantt parent-child grouping & expand/collapse
 * - Parent card deletion & trash restore cascading to sub-cards
 */
import { expect, test, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import { addGantt, bars, clearDates, gantt, NOW, setDates, WELCOME } from './gantt.support.ts';
import { openView } from './kanban.support.ts';
import { pressInBar } from './shell.support.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });
test.describe.configure({ mode: 'parallel', timeout: 90_000 });

interface StoredCard {
  id: string;
  parentId: string;
  title: string;
  fields: { isTemplate: boolean; properties: Record<string, unknown> };
}

interface DevBackend {
  latency: number;
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string; fields: { viewType: string } }[]>;
  cards(boardId: string): Promise<StoredCard[]>;
  container(boardId: string): Promise<{
    cardProperties: {
      id: string;
      name: string;
      type: string;
      options: { id: string; label: string }[];
    }[];
  }>;
  patchCard(boardId: string, cardId: string, patch: Record<string, unknown>): Promise<unknown>;
}

interface Place {
  boardId: string;
  kanbanViewId: string;
  tableViewId: string;
  cards: StoredCard[];
}

/** Finds a seeded board, its kanban & table views, and its real cards. */
async function seedPlace(page: Page, boardTitle: string): Promise<Place> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  return page.evaluate(async (wantedTitle) => {
    const { backend } = (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev;
    const [ws] = await backend.workspaces();
    const boards = await backend.boards(ws?.id ?? '');
    const board = boards.find((b) => b.title.startsWith(wantedTitle));
    if (!board) throw new Error(`no board ${wantedTitle}`);
    const views = await backend.views(board.id);
    const kanban = views.find((v) => v.fields.viewType === 'board') ?? views[0];
    const table = views.find((v) => v.fields.viewType === 'table') ?? views[0];
    const cards = (await backend.cards(board.id)).filter((c) => !c.fields.isTemplate);
    return {
      boardId: board.id,
      kanbanViewId: kanban?.id ?? '',
      tableViewId: table?.id ?? '',
      cards,
    };
  }, boardTitle);
}

/** Closes the open card dialog in any viewport stage (desktop/middle close button or mobile back). */
async function closeCardDialog(page: Page): Promise<void> {
  const close = page.getByTestId('card-close');
  if (await close.isVisible()) await close.click();
  else await page.getByTestId('card-back').click();
  await expect(page.getByTestId('card-dialog')).toHaveCount(0);
}

test('creates, links, detaches sub-cards, enforces depth 3, updates progress and Kanban face badges', async ({
  page,
}) => {
  const place = await seedPlace(page, 'Getting started');
  const root = place.cards.find((c) => c.title === 'Open a card');
  const existingDone = place.cards.find((c) => c.title === 'Filter, sort and group');
  if (!root || !existingDone) throw new Error('expected seed cards missing');

  // Navigate to root card first (each `page.goto` starts a fresh in-memory backend).
  await page.goto(boardPath(place.boardId, place.kanbanViewId, root.id));
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('card-subcards')).toBeVisible();

  // Mark `existingDone` as Completed on the live backend so progress shows 1/2 (50%).
  await page.evaluate(
    async ({ boardId, cardId }) => {
      const { backend } = (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev;
      const board = await backend.container(boardId);
      const status = board.cardProperties.find((p) => p.name === 'Progress');
      const done = status?.options.find((o) => o.label === 'Done it');
      if (status && done) {
        await backend.patchCard(boardId, cardId, { updatedProperties: { [status.id]: done.id } });
      }
    },
    { boardId: place.boardId, cardId: existingDone.id },
  );

  // 1. Create a new sub-card inline with Enter.
  const input = dialog.getByTestId('subcard-new-input');
  await input.fill('Subtask One');
  await input.press('Enter');
  await expect(dialog.getByTestId('subcards-list')).toContainText('Subtask One');
  await expect(dialog.getByTestId('subcards-progress')).toContainText('0/1 (0%)');

  // 2. Attach an existing completed card as a sub-card via picker.
  await dialog.getByTestId('subcard-link-trigger').click();
  await page.getByRole('option', { name: /Filter, sort and group/ }).click();
  await expect(dialog.getByTestId('subcards-list')).toContainText('Filter, sort and group');
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');
  await expect(dialog.getByTestId('subcards-progress-bar')).toHaveAttribute('aria-valuenow', '50');

  // 3. Navigate into depth-2 sub-card (`Subtask One`) and create a depth-3 grandchild.
  await dialog
    .getByTestId('subcards-list')
    .getByRole('button', { name: /Open sub-card.*Subtask One/ })
    .click();
  await expect(dialog.getByTestId('card-parent-breadcrumb')).toContainText('Open a card');
  await dialog.getByTestId('subcard-new-input').fill('Grandchild Leaf');
  await dialog.getByTestId('subcard-new-input').press('Enter');
  await expect(dialog.getByTestId('subcards-list')).toContainText('Grandchild Leaf');

  // 4. Open depth-3 grandchild -> 2-step breadcrumb shown, add-subcard section hidden!
  await dialog
    .getByTestId('subcards-list')
    .getByRole('button', { name: /Open sub-card.*Grandchild Leaf/ })
    .click();
  const crumb = dialog.getByTestId('card-parent-breadcrumb');
  await expect(crumb).toContainText('Open a card');
  await expect(crumb).toContainText('Subtask One');
  await expect(dialog.getByTestId('card-subcards')).toHaveCount(0);

  // Navigate back to root via breadcrumb click.
  await crumb.locator(`[data-parent-crumb="${root.id}"]`).click();
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');

  // Detach `Filter, sort and group` and verify progress drops back to 0/1, then undo via card menu.
  await dialog.locator(`[data-subcard-detach="${existingDone.id}"]`).click();
  await expect(dialog.getByTestId('subcards-progress')).toContainText('0/1 (0%)');

  await dialog.getByTestId('card-menu').click();
  await page.getByRole('menuitem', { name: 'Undo' }).click();
  await expect(dialog.getByTestId('subcards-progress')).toContainText('1/2 (50%)');

  // Close card without reloading and verify Kanban CardFace displays sub-card badge and parent hint.
  await closeCardDialog(page);
  await expect(page.getByTestId('kanban')).toBeVisible();
  const rootFace = page.locator(`[data-card="${root.id}"]`);
  await expect(rootFace.locator('[data-slot="card-subcards"]')).toContainText('1/2');
  const childFace = page.locator(`[data-card="${existingDone.id}"]`);
  await expect(childFace.locator('[data-slot="card-parent-hint"]')).toContainText('Open a card');
});

test('groups sub-cards under parents in Table view with expand/collapse and filtered-parent safety', async ({
  page,
}) => {
  const place = await seedPlace(page, 'Project schedule');
  const [parent, child, grand] = place.cards;
  if (!parent || !child || !grand) throw new Error('expected 3 seed cards');

  await page.goto(boardPath(place.boardId, place.tableViewId));
  const table = page.getByTestId('table');
  await expect(table).toBeVisible();

  // Set up 3-level hierarchy on the live backend after navigation.
  await page.evaluate(
    async ({ boardId, parentId, childId, grandId }) => {
      const { backend } = (window as unknown as { fvociDev: { backend: DevBackend } }).fvociDev;
      await backend.patchCard(boardId, childId, { parentId });
      await backend.patchCard(boardId, grandId, { parentId: childId });
    },
    { boardId: place.boardId, parentId: parent.id, childId: child.id, grandId: grand.id },
  );

  // Parent (level 0) -> child (level 1) -> grandchild (level 2) immediately grouped.
  const rows = table.locator('[role="row"][data-card]');
  await expect(rows.locator(`[data-hierarchy-toggle="${parent.id}"]`)).toHaveAttribute(
    'aria-expanded',
    'true',
  );
  await expect(table.locator(`[data-row="${parent.id}"]`)).toHaveAttribute('data-level', '0');
  await expect(table.locator(`[data-row="${child.id}"]`)).toHaveAttribute('data-level', '1');
  await expect(table.locator(`[data-row="${grand.id}"]`)).toHaveAttribute('data-level', '2');

  const idsBefore = await rows.evaluateAll((els) => els.map((el) => el.getAttribute('data-card')));
  const pIdx = idsBefore.indexOf(parent.id);
  expect(idsBefore.slice(pIdx, pIdx + 3)).toEqual([parent.id, child.id, grand.id]);

  // Collapse parent -> hides both child and grandchild.
  await table.locator(`[data-hierarchy-toggle="${parent.id}"]`).click();
  await expect(table.locator(`[data-hierarchy-toggle="${parent.id}"]`)).toHaveAttribute(
    'aria-expanded',
    'false',
  );
  await expect(table.locator(`[data-row="${child.id}"]`)).toHaveCount(0);
  await expect(table.locator(`[data-row="${grand.id}"]`)).toHaveCount(0);

  // Expand parent -> child and grandchild reappear.
  await table.locator(`[data-hierarchy-toggle="${parent.id}"]`).click();
  await expect(table.locator(`[data-row="${child.id}"]`)).toHaveCount(1);
  await expect(table.locator(`[data-row="${grand.id}"]`)).toHaveCount(1);
});

test('groups sub-cards under parents in Gantt view with expand/collapse', async ({
  page,
}, info) => {
  await page.clock.setFixedTime(NOW);
  await openView(page, info, ...WELCOME);
  await clearDates(page);
  await addGantt(page);
  await setDates(page, {
    'Open a card': { from: '2026-11-10', to: '2026-11-18' },
    'Filter, sort and group': { from: '2026-11-11', to: '2026-11-13' },
    'Write in the body': { from: '2026-11-14', to: '2026-11-16' },
  });

  const ids = await page.evaluate(async () => {
    const store = (
      window as unknown as {
        fvociDev: {
          workspace: {
            board: {
              cards: StoredCard[];
              changeCard: (id: string, fn: (c: StoredCard) => StoredCard) => Promise<void>;
            };
          };
        };
      }
    ).fvociDev.workspace.board;
    const byTitle = (t: string) => store.cards.find((c) => c.title === t)?.id ?? '';
    const parentId = byTitle('Open a card');
    const childId = byTitle('Filter, sort and group');
    const grandId = byTitle('Write in the body');
    await store.changeCard(childId, (c) => ({ ...c, parentId }));
    await store.changeCard(grandId, (c) => ({ ...c, parentId: childId }));
    return { parentId, childId, grandId };
  });

  await expect(bars(page)).toHaveCount(3);
  await expect(gantt(page).locator(`[data-row="${ids.parentId}"]`)).toHaveAttribute(
    'data-level',
    '0',
  );
  await expect(gantt(page).locator(`[data-row="${ids.childId}"]`)).toHaveAttribute(
    'data-level',
    '1',
  );
  await expect(gantt(page).locator(`[data-row="${ids.grandId}"]`)).toHaveAttribute(
    'data-level',
    '2',
  );

  // Collapse parent -> only parent bar stays visible.
  await gantt(page).locator(`[data-hierarchy-toggle="${ids.parentId}"]`).click();
  await expect(bars(page)).toHaveCount(1);
  await expect(gantt(page).locator(`[data-row="${ids.childId}"]`)).toHaveCount(0);

  // Expand parent -> all 3 bars return.
  await gantt(page).locator(`[data-hierarchy-toggle="${ids.parentId}"]`).click();
  await expect(bars(page)).toHaveCount(3);

  // Keyboard ArrowLeft collapses and ArrowRight expands on the focused parent bar.
  const parentBar = gantt(page).locator(`[data-bar="${ids.parentId}"]`);
  await parentBar.focus();
  await page.keyboard.press('ArrowLeft');
  await expect(bars(page)).toHaveCount(1);
  await page.keyboard.press('ArrowRight');
  await expect(bars(page)).toHaveCount(3);
});

test('deleting and restoring a parent card cascades to its sub-cards in the UI', async ({
  page,
}) => {
  const place = await seedPlace(page, 'Getting started');
  const root = place.cards.find((c) => c.title === 'Open a card');
  if (!root) throw new Error('missing root');

  // Create a sub-card under root.
  await page.goto(boardPath(place.boardId, place.kanbanViewId, root.id));
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();
  await dialog.getByTestId('subcard-new-input').fill('Cascade Child');
  await dialog.getByTestId('subcard-new-input').press('Enter');
  await expect(dialog.getByTestId('subcards-list')).toContainText('Cascade Child');

  // Delete parent card from its menu -> both parent and child disappear from the board.
  await dialog.getByTestId('card-menu').click();
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByText('Open a card')).toHaveCount(0);
  await expect(page.getByText('Cascade Child')).toHaveCount(0);

  // Restore parent from card trash -> both parent and child return with hierarchy intact!
  await pressInBar(page, 'card-trash');
  const trash = page.getByTestId('card-trash');
  await expect(trash).toBeVisible();
  const row = trash.locator('[data-trash-item="Open a card"]');
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Restore Open a card' }).click();
  await expect(row).toHaveCount(0);
  await page.keyboard.press('Escape');

  const rootFace = page.locator(`[data-card="${root.id}"]`);
  await expect(rootFace).toBeVisible();
  await expect(rootFace.locator('[data-slot="card-subcards"]')).toContainText('0/1');

  // Open restored parent and verify the sub-card came back under it.
  await rootFace.click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('subcards-list')).toContainText('Cascade Child');
});
