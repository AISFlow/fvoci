/**
 * A board with one long column, on the in-memory backend: the view "Long
 * column" of the seeded board "Project schedule", grouped by its status, with
 * `LONG` more cards in "In progress". Every card has a person, a date, two
 * selects and the badges of a body with tasks, as a busy board's cards do.
 *
 * The cards are made on a board the page is not showing, and the view is
 * shown after, by the app's own way of going to an address: a store that
 * took 1,600 changes one by one would be measured instead of the column.
 */
import { expect, type Page } from '@playwright/test';

import { boardPath } from './address.ts';

/** How many cards the long column gets, besides the one the seed has there. */
export const LONG = 1600;
export const LONG_COLUMN = 'In progress';
export const LONG_VIEW = 'Long column';

/** The title of the long column's `n`th card of those made here, from 1. */
export const longTitle = (n: number): string => `Long ${String(n)}`;

interface Backend {
  latency: number;
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  users(workspaceId: string): Promise<{ id: string }[]>;
  container(containerId: string): Promise<{
    cardProperties: { id: string; name: string; options: { id: string; label: string }[] }[];
  }>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  createCard(boardId: string, draft: Record<string, unknown>): Promise<{ id: string }>;
  createView(boardId: string, draft: Record<string, unknown>): Promise<{ id: string }>;
}

/**
 * Makes the long column on the page that is open, without showing it, and
 * answers the address of its view. The page must have the app open. The app
 * opens on "Project schedule": it is left for another board first.
 */
export async function makeLongColumn(
  page: Page,
  count = LONG,
  /** Every third card gets a title of two lines or more, as cards made by people have. */
  wordy = false,
): Promise<string> {
  const elsewhere = await page.evaluate(async () => {
    const backend = (window as unknown as { fvociDev: { backend: Backend } }).fvociDev.backend;
    const workspaces = await backend.workspaces();
    const boards = await backend.boards(workspaces[0]?.id ?? '');
    return boards.find((board) => board.title === 'Issue tracker')?.id ?? '';
  });
  await page.evaluate((address) => {
    history.pushState(null, '', address);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, boardPath(elsewhere));
  await page.waitForFunction(
    (id) =>
      (window as unknown as { fvociDev: { workspace: { board?: { board?: { id: string } } } } })
        .fvociDev.workspace.board?.board?.id === id,
    elsewhere,
  );
  const made = await page.evaluate(
    async ([many, columnName, viewTitle, wordier]) => {
      const backend = (window as unknown as { fvociDev: { backend: Backend } }).fvociDev.backend;
      backend.latency = 0;
      const workspaces = await backend.workspaces();
      const workspaceId = workspaces[0]?.id ?? '';
      const boards = await backend.boards(workspaceId);
      const boardId = boards.find((board) => board.title === 'Project schedule')?.id ?? '';
      const people = await backend.users(workspaceId);
      const properties = (await backend.container(boardId)).cardProperties;
      const named = (name: string) => {
        const found = properties.find((property) => property.name === name);
        if (!found) throw new Error(`the board has no property "${name}"`);
        return found;
      };
      const status = named('Status');
      const phase = named('Phase');
      const owner = named('Owner');
      const period = named('Period');
      const columnId = status.options.find((option) => option.label === columnName)?.id;
      if (!columnId) throw new Error(`no status "${columnName}"`);
      const day = (offset: number): string =>
        new Date(Date.UTC(2026, 10, 1) + offset * 86_400_000).toISOString().slice(0, 10);
      const task = (text: string, checked: boolean) => ({
        type: 'taskItem',
        attrs: { checked },
        content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
      });
      for (let at = 0; at < many; at += 1) {
        await backend.createCard(boardId, {
          title:
            wordier && at % 3 === 0
              ? `Long ${String(at + 1)}: agree the scope and the budget with everyone involved in it`
              : `Long ${String(at + 1)}`,
          properties: {
            [status.id]: columnId,
            [phase.id]: phase.options[at % phase.options.length]?.id ?? '',
            [owner.id]: people[at % people.length]?.id ?? '',
            [period.id]: { from: day(at % 60) },
          },
          body: {
            type: 'doc',
            content: [
              {
                type: 'taskList',
                content: [task('Draft', at % 2 === 0), task('Review', false)],
              },
            ],
          },
        });
      }
      const view = await backend.createView(boardId, {
        title: viewTitle,
        fields: {
          viewType: 'board',
          groupBy: status.id,
          sort: [],
          shownProperties: [phase.id, status.id, owner.id, period.id],
          visibleGroups: [
            columnId,
            ...status.options.map((option) => option.id).filter((id) => id !== columnId),
          ],
          // The column of cards without a status is hidden: the long one is the first.
          hiddenGroups: [''],
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
    },
    [count, LONG_COLUMN, LONG_VIEW, wordy] as const,
  );
  return boardPath(made.boardId, made.viewId);
}

/**
 * Shows a view by the app's way of going to an address, and answers how long
 * it took, in milliseconds: from the change of address to the second frame
 * after a column counts `count` cards or more. The count, and not the cards
 * on the page: a column may draw only the cards near the view.
 */
export async function showView(page: Page, path: string, count: number): Promise<number> {
  const drawn = await page.evaluate(
    ([address, many]) =>
      new Promise<number>((resolve) => {
        const started = performance.now();
        history.pushState(null, '', address);
        window.dispatchEvent(new PopStateEvent('popstate'));
        const look = (): void => {
          const counted = [...document.querySelectorAll('[data-column-count]')].map((count) =>
            Number.parseInt(count.textContent, 10),
          );
          if (!counted.some((cards) => cards >= many)) {
            requestAnimationFrame(look);
            return;
          }
          requestAnimationFrame(() => {
            requestAnimationFrame(() => {
              resolve(Math.round(performance.now() - started));
            });
          });
        };
        requestAnimationFrame(look);
      }),
    [path, count] as const,
  );
  await expect(page.getByTestId('kanban-board')).toBeVisible();
  return drawn;
}

/** The id of a card of "Project schedule" by its title, whether the column draws it or not. */
export async function cardIdOf(page: Page, title: string): Promise<string> {
  return page.evaluate(async (wanted) => {
    const backend = (window as unknown as { fvociDev: { backend: Backend } }).fvociDev.backend;
    const workspaces = await backend.workspaces();
    const boards = await backend.boards(workspaces[0]?.id ?? '');
    const boardId = boards.find((board) => board.title === 'Project schedule')?.id ?? '';
    return (await backend.cards(boardId)).find((card) => card.title === wanted)?.id ?? '';
  }, title);
}
