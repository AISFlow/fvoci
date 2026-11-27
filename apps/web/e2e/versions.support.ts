/**
 * What the specs of the versions of a body share (body-versions.spec.ts,
 * body-versions-compare.spec.ts): a card with a body opened on the
 * in-memory backend, the dialog of its versions, and a body written whole by
 * somebody else, which keeps the body before and after it as versions.
 */
import { expect, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';

/** What `(window as unknown as { fvociDev: Dev }).fvociDev` offers a test (src/app/backend.ts). Only what is used here. */
export interface DevBackend {
  me(): Promise<{ id: string; username: string }>;
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  card(boardId: string, cardId: string): Promise<{ updatedAt: number }>;
  addMember(boardId: string, userId: string, role: 'admin'): Promise<unknown>;
  setMemberRole(boardId: string, userId: string, role: 'viewer' | 'admin'): Promise<unknown>;
  writeCardBody(boardId: string, cardId: string, write: object): Promise<unknown>;
  failNext(method: string, error: unknown): void;
}
export interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

export interface Place {
  boardId: string;
  viewId: string;
  cardId: string;
}

export const paragraph = (text: string): object => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

/** Opens a card that has a body, by its address, and waits until its editor is there. */
export async function openTaskCard(page: Page): Promise<Place> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title.startsWith('Project schedule'));
    if (!board) throw new Error('no board of tasks');
    const views = await backend.views(board.id);
    const cards = await backend.cards(board.id);
    const card = cards.find((each) => each.title === 'Agree on the scope');
    if (!card) throw new Error('no card');
    const first = [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
    return { boardId: board.id, viewId: first?.id ?? '', cardId: card.id };
  });
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  return place;
}

export const editor = (page: Page): Locator =>
  page.getByTestId('card-body').locator('[contenteditable]').first();
export const versions = (page: Page): Locator => page.getByTestId('body-versions');
export const rows = (page: Page): Locator => versions(page).locator('[data-version]');
export const shown = (page: Page): Locator => page.getByTestId('body-version');

export async function openVersions(page: Page): Promise<void> {
  await page.getByTestId('card-menu').click();
  await page.locator('[data-action="versions"]').click();
  await expect(versions(page)).toBeVisible();
}

export async function closeVersions(page: Page): Promise<void> {
  await versions(page).getByRole('button', { name: 'Close' }).click();
  await expect(versions(page)).toHaveCount(0);
  // The card under it stays.
  await expect(page.getByTestId('card-dialog')).toBeVisible();
}

/** Narrow, the body of a version takes the place of the list. */
export const narrow = (page: Page): boolean => (page.viewportSize()?.width ?? 0) < 640;

/** Somebody else, made an admin of the board, writes the card's body whole: a paragraph of `text`, or a document. */
export async function writtenWholeElsewhere(
  page: Page,
  place: Place,
  text: string | object,
): Promise<void> {
  await page.evaluate(
    async ({ at, body }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const other = dev.someoneElse();
      const otherId = (await other.me()).id;
      await dev.backend.addMember(at.boardId, otherId, 'admin');
      await dev.backend.setMemberRole(at.boardId, otherId, 'admin');
      const card = await other.card(at.boardId, at.cardId);
      await other.writeCardBody(at.boardId, at.cardId, { body, readAt: card.updatedAt });
    },
    { at: place, body: typeof text === 'string' ? paragraph(text) : text },
  );
}
