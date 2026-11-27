/**
 * Opening a card with a body of the test's own, for the tests of formulas and
 * diagrams (embeds.spec.ts, diagram-place.spec.ts).
 */
import { expect, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';

/** What `window.fvociDev` offers a test (src/app/backend.ts). Only what is used here. */
export interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  card(boardId: string, cardId: string): Promise<{ updatedAt: number }>;
  writeCardBody(boardId: string, cardId: string, write: object): Promise<unknown>;
  patchSettings(patch: object): Promise<unknown>;
}
export interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

/**
 * Opens the first card of a board, and then somebody else gives it `body`.
 * In that order: the data lives in memory and a page that loads starts from
 * the seed again, so what was written before the card was opened is gone. It
 * is written as another member because on the board the reader may only look
 * at, the reader cannot.
 */
export async function openCardWithBody(page: Page, board: string, doc: object): Promise<Locator> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async (boardTitle) => {
    const reader = (window as unknown as { fvociDev: Dev }).fvociDev.backend;
    const [workspace] = await reader.workspaces();
    const boards = await reader.boards(workspace?.id ?? '');
    const found = boards.find((each) => each.title.startsWith(boardTitle));
    if (!found) throw new Error(`no board ${boardTitle}`);
    const cards = await reader.cards(found.id);
    // Pick deterministically by title so boards with code/math showcase cards (such as
    // "Write in the body") do not race the replacement body written below.
    const [card] = [...cards].sort((one, other) => one.title.localeCompare(other.title));
    const [view] = await reader.views(found.id);
    if (!card || !view) throw new Error('the board has no card or no view');
    return { boardId: found.id, viewId: view.id, cardId: card.id };
  }, board);
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  const body = page.getByTestId('card-body');
  await expect(body).toBeVisible();
  await expect(page.getByTestId('card-comments')).toBeVisible();
  await expect
    .poll(async () => ((await body.textContent()) ?? '').trim().length)
    .toBeGreaterThan(0);
  const seedSnippet = ((await body.textContent()) ?? '').trim().slice(0, 40);
  await page.evaluate(
    async ([at, written]) => {
      const handle = (window as unknown as { fvociDev: Dev }).fvociDev;
      const { updatedAt } = await handle.backend.card(at.boardId, at.cardId);
      await handle
        .someoneElse()
        .writeCardBody(at.boardId, at.cardId, { body: written, readAt: updatedAt });
    },
    [place, doc] as const,
  );
  if (seedSnippet.length > 0 && !JSON.stringify(doc).includes(seedSnippet)) {
    await expect(body).not.toContainText(seedSnippet);
  }
  return body;
}

/** A body of one diagram, with this source. */
export function diagramBody(source: string): { type: 'doc'; content: object[] } {
  return {
    type: 'doc',
    content: [
      {
        type: 'codeBlock',
        attrs: { language: 'mermaid' },
        content: [{ type: 'text', text: source }],
      },
    ],
  };
}

/** The one diagram of a body is drawn, and the whole of it is in its picture. */
export async function drawnDiagram(body: Locator): Promise<void> {
  const diagram = body.locator('.doc-diagram[data-state="drawn"]');
  await expect(diagram).toHaveCount(1, { timeout: 30_000 });
  await expect(diagram.locator('svg')).toBeVisible();
  // Nothing drawn outside what the picture shows.
  const outside = await diagram.locator('svg').evaluate((svg) => {
    const frame = svg.getBoundingClientRect();
    const content = svg.querySelector('g')?.getBoundingClientRect();
    if (!content) return Number.POSITIVE_INFINITY;
    return Math.max(
      frame.left - content.left,
      frame.top - content.top,
      content.right - frame.right,
      content.bottom - frame.bottom,
    );
  });
  expect(outside).toBeLessThanOrEqual(1);
}

/**
 * Puts the cursor at the end of the body and the focus in it, through the
 * editor: a key (Ctrl+End) and an arrow right behind it can race the editor
 * reading where the browser moved the cursor, which no hand is quick enough for.
 */
export async function focusAtEnd(body: Locator): Promise<void> {
  // The editor of the room's document, not the view shown before it came.
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await expect(body).not.toHaveAttribute('data-settling');
  await body.locator('.ProseMirror').evaluate(
    (element: HTMLElement & { editor?: { commands: { focus: (at: string) => void } } }) =>
      new Promise<void>((resolve) => {
        element.editor?.commands.focus('end');
        // The editor takes the focus a frame after it is asked to (in WebKit at once, and
        // then again in that frame): a key pressed before goes elsewhere, and a field opened
        // before loses the focus to the editor, which closes it. A frame asked for after
        // the editor's comes after it.
        requestAnimationFrame(() => {
          resolve();
        });
      }),
  );
  await expect(body.locator('.ProseMirror')).toBeFocused();
}
