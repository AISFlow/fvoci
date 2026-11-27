/** Two independent browser sessions see who is on their board. */
import { expect, test, type Page, type APIRequestContext, type Browser } from '@playwright/test';
import { screensDir } from '../e2e/screens.ts';
import {
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  watchErrors,
  type MadeBoard,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

test('board presence shows another person and removes their closed browser', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('present-owner');
  const other = person('present-reader');
  await register(base, owner);
  await register(base, other);
  const api = await signedIn(base, owner);
  const otherApi = await signedIn(base, other);
  const board = await makeBoard(api, {
    title: 'Here together',
    cards: [{ title: 'Discuss the draft', status: 'To do' }],
    members: [{ who: other, role: 'viewer' }],
  });
  const firstContext = await browser.newContext({
    baseURL: base,
    storageState: await api.storageState(),
    locale: 'en-US',
  });
  const secondContext = await browser.newContext({
    baseURL: base,
    storageState: await otherApi.storageState(),
    locale: 'en-US',
  });
  const first = await firstContext.newPage();
  const second = await secondContext.newPage();
  const errors = [watchErrors(first), watchErrors(second)];
  try {
    await first.goto(boardPath(board, board.viewId));
    // Nobody else here: the group says it, and there is no button of faces.
    await expect(first.getByTestId('board-people')).toHaveAccessibleName(
      /^People viewing this board: 0, members: \d+$/,
    );
    await expect(first.getByTestId('board-presence')).toBeHidden();
    await second.goto(boardPath(board, board.viewId));
    for (const page of [first, second]) {
      await expect(page.getByTestId('board-presence')).toHaveAccessibleName(
        'People viewing this board: 1',
      );
      await expect(page.getByTestId('board-presence').locator('[data-avatar]')).toHaveCount(1);
    }
    await first.getByTestId('board-presence').click();
    await expect(first.getByTestId('presence-list')).toContainText(other.username);
    await expect(first.getByTestId('presence-list')).not.toContainText(owner.username);
    await first.keyboard.press('Escape');
    await membersOfTheBoard(first, second, owner.username);
    await cardsInEveryView(first, second, api, board);
    await anonymousVisit(first, browser, base, api, board);
    await secondContext.close();
    // Nobody else here: the group says it, and there is no button of faces.
    await expect(first.getByTestId('board-people')).toHaveAccessibleName(
      /^People viewing this board: 0, members: \d+$/,
    );
    await expect(first.getByTestId('board-presence')).toBeHidden();
    expect(errors.flat()).toEqual([]);
  } finally {
    await firstContext.close();
    await secondContext.close();
    await api.dispose();
    await otherApi.dispose();
  }
});

/**
 * Beside the faces, the number of the board's members opens them in the sharing dialog:
 * to change for its admin, to read for a viewer.
 */
async function membersOfTheBoard(admin: Page, viewer: Page, adminName: string): Promise<void> {
  for (const page of [admin, viewer]) {
    await expect(page.getByTestId('board-members-count')).toHaveAccessibleName('Members 2');
    await expect(page.getByTestId('board-people')).toHaveAttribute(
      'aria-label',
      'People viewing this board: 1, members: 2',
    );
  }
  await viewer.getByTestId('board-members-count').click();
  const dialog = viewer.getByTestId('share-dialog');
  await expect(dialog.getByTestId('share-members-title')).toBeFocused();
  await expect(dialog.getByTestId('share-members')).toContainText(adminName);
  await expect(dialog.locator('[data-slot="dialog-title"]')).toHaveText('Members');
  for (const control of ['share-search', 'share-guest', 'share-open', 'share-public']) {
    await expect(dialog.getByTestId(control)).toHaveCount(0);
  }
  await expect(dialog.getByRole('combobox')).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: /^Remove / })).toHaveCount(0);
  await viewer.keyboard.press('Escape');
  await admin.getByTestId('board-members-count').click();
  await expect(admin.getByTestId('share-dialog').getByTestId('share-search')).toBeVisible();
  await admin.keyboard.press('Escape');
}

async function cardsInEveryView(
  first: Page,
  second: Page,
  api: APIRequestContext,
  board: MadeBoard,
): Promise<void> {
  const cardId = board.cards['Discuss the draft'] ?? '';
  await second.goto(boardPath(board, board.viewId, cardId));
  await expect(second.getByTestId('card-dialog')).toBeVisible();
  await expect(
    first.locator(`[data-card="${cardId}"]`).getByTestId('card-presence').locator('[data-avatar]'),
  ).toHaveCount(1);
  await first.getByTestId('board-presence').click();
  await expect(first.getByTestId('presence-list')).toContainText('Discuss the draft');
  await shoot(first, 'roster-wide');
  await first.keyboard.press('Escape');
  await shoot(first, 'kanban-wide');
  for (const kind of ['table', 'gallery']) {
    const response = await api.post(
      `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`,
      {
        data: [
          {
            type: 'view',
            containerId: board.boardId,
            title: kind,
            fields: { viewType: kind },
            createdAt: Date.now(),
            updatedAt: Date.now(),
          },
        ],
      },
    );
    expect(response.ok()).toBe(true);
    const [view] = (await response.json()) as Schemas['Block'][];
    if (!view) throw new Error('the new view');
    await first.goto(boardPath(board, view.id));
    await expect(
      first
        .locator(`[data-card="${cardId}"]`)
        .getByTestId('card-presence')
        .locator('[data-avatar]'),
    ).toHaveCount(1);
    await shoot(first, `${kind}-wide`);
    await first.setViewportSize({ width: 390, height: 844 });
    await shoot(first, `${kind}-390`);
    await first.setViewportSize({ width: 1280, height: 800 });
  }
  await first.goto(boardPath(board, board.viewId));
  await first.setViewportSize({ width: 390, height: 844 });
  await expect(first.locator(`[data-card="${cardId}"]`).getByTestId('card-presence')).toBeVisible();
  await shoot(first, 'kanban-390');
  await first.getByTestId('board-presence').click();
  await shoot(first, 'roster-390');
  await first.keyboard.press('Escape');
  await first.setViewportSize({ width: 1280, height: 800 });
  await second.keyboard.press('Escape');
  await expect(first.getByTestId('card-presence')).toHaveCount(0);
}

async function anonymousVisit(
  first: Page,
  browser: Browser,
  base: string,
  api: APIRequestContext,
  board: MadeBoard,
): Promise<void> {
  const response = await api.post(
    `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/public-link`,
    { data: { enabled: true } },
  );
  expect(response.ok()).toBe(true);
  const sharing = (await response.json()) as Schemas['PublicLink'];
  const context = await browser.newContext({ baseURL: base, locale: 'en-US' });
  try {
    const reader = await context.newPage();
    const received: unknown[] = [];
    reader.on('websocket', (socket) =>
      socket.on('framereceived', (frame) => received.push(frame.payload)),
    );
    await reader.goto(`/shared/${board.boardId}?r=${encodeURIComponent(sharing.token ?? '')}`);
    await expect(reader.getByTestId('board-page')).toBeVisible();
    await expect(first.getByTestId('board-presence')).toContainText('1 anonymous');
    await first.setViewportSize({ width: 390, height: 844 });
    await shoot(first, 'anonymous-390');
    await first.setViewportSize({ width: 1280, height: 800 });
    await shoot(first, 'anonymous-wide');
    await expect(reader.getByTestId('board-presence')).toHaveCount(0);
    expect(received).toEqual([]);
  } finally {
    await context.close();
  }
  await expect(first.getByTestId('board-presence')).not.toContainText('anonymous');
}

async function shoot(page: Page, name: string): Promise<void> {
  if (!process.env.FVOCI_SCREENS) return;
  await page.screenshot({ path: `${screensDir('presence')}/${name}.png`, animations: 'disabled' });
}
