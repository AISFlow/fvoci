/**
 * Templates on the real server: a board made from a built-in template, a
 * template made from a board and changed on its own page, the workspace's
 * template offered to be changed again from "Add board", the welcome
 * board with its views, its pictures and its document, in either language,
 * and a board from each of the other templates, opened on its first view.
 */
import path from 'node:path';

import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

import { screensDir } from '../e2e/screens.ts';

import { boardLink, boardMenu, chooseFromMenu, headCuts, sidebar } from '../e2e/shell.support.ts';

import {
  boardPath,
  makeBoard,
  person,
  placeOf,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

async function templates(api: APIRequestContext, workspaceId: string, language?: string) {
  const answer = await api.get(`/api/v1/w/${workspaceId}/templates`, {
    headers: language === undefined ? {} : { 'Accept-Language': language },
  });
  expect(answer.ok()).toBe(true);
  return (await answer.json()) as Schemas['Container'][];
}

async function views(api: APIRequestContext, workspaceId: string, boardId: string) {
  return (await (
    await api.get(`/api/v1/w/${workspaceId}/boards/${boardId}/blocks`, { params: { type: 'view' } })
  ).json()) as Schemas['Block'][];
}

test('a board is made from a template, and a template from a board is changed on its page', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('templater');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const mine = await makeBoard(api, { title: 'Weekly plan', cards: [{ title: 'Water' }] });
  const builtIn = (await templates(api, mine.workspaceId)).find(
    (template) => template.title === 'Issue tracker',
  );
  expect(builtIn?.isBuiltin).toBe(true);

  const errors = watchErrors(page);
  await page.goto(boardPath(mine, mine.viewId));
  await signInOnPage(page, owner);

  // "Add board" offers an empty board and the templates; a built-in one is used.
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await expect(picker.getByTestId('empty-board')).toBeVisible();
  const issueTracker = picker.locator('[data-template="Issue tracker"]');
  // A built-in template is nobody's to change.
  await expect(issueTracker.getByRole('button', { name: /^Change the template/ })).toHaveCount(0);
  await issueTracker.getByRole('button', { name: 'Use this template: Issue tracker' }).click();
  await expect(boardLink(page, 'Issue tracker')).toHaveAttribute('aria-current', 'page');
  const made = placeOf(page.url()).boardId;
  const copy = (await (
    await api.get(`/api/v1/w/${mine.workspaceId}/containers/${made}`)
  ).json()) as Schemas['Container'];
  // In the reader's workspace (sent as `toWorkspace`: a built-in template is kept apart from every workspace),
  // open to it as the window of new things makes it in a shared one: the copy is made private, then opened.
  expect(copy).toMatchObject({
    title: 'Issue tracker',
    isTemplate: false,
    visibility: 'open',
    workspaceId: mine.workspaceId,
  });
  expect((await views(api, mine.workspaceId, made)).length).toBe(
    (await views(api, mine.workspaceId, builtIn?.id ?? '')).length,
  );

  // A template from a board: it opens on its page, and is changed there.
  await chooseFromMenu(page, boardMenu(page, 'Weekly plan'), 'template-from-board');
  await expect(page.getByTestId('template-note')).toBeVisible();
  const title = page.getByTestId('top-bar').getByTestId('board-title');
  await expect(title).toHaveValue('New board template');
  await title.fill('Our weekly plan');
  await title.press('Enter');
  await expect
    .poll(async () =>
      (await templates(api, mine.workspaceId))
        .filter((one) => !one.isBuiltin)
        .map((one) => one.title),
    )
    .toEqual(['Our weekly plan']);
  // No board of the sidebar.
  await expect(boardLink(page, 'Our weekly plan')).toHaveCount(0);
  const templateId = placeOf(page.url()).boardId;

  // Offered again from "Add board", to be changed or used.
  await page.goto(boardPath(mine, mine.viewId));
  await sidebar(page).getByTestId('add-board').click();
  await picker.getByRole('button', { name: 'Change the template Our weekly plan' }).click();
  await expect(page).toHaveURL(new RegExp(boardPath({ ...mine, boardId: templateId })));
  await expect(page.getByTestId('template-note')).toBeVisible();

  await sidebar(page).getByTestId('add-board').click();
  await picker.getByRole('button', { name: 'Use this template: Our weekly plan' }).click();
  await expect(boardLink(page, 'Our weekly plan')).toHaveAttribute('aria-current', 'page');
  await expect(page.getByText('Water')).toBeVisible();
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  await api.dispose();
  expect(errors).toEqual([]);
});

/** Pictures the page as wide as it is and 390 pixels wide, with `FVOCI_SCREENS=1`: `target/screens/templates/`. */
async function shot(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS !== '1') return;
  const folder = screensDir('templates');
  const wide = page.viewportSize() ?? { width: 1280, height: 720 };
  await page.screenshot({ path: path.join(folder, `${name}-wide.png`) });
  await page.setViewportSize({ width: 390, height: 844 });
  // A narrow screen has the sidebar in a drawer, which is shut: the picture waits
  // for the sidebar that stood beside the page to slide away.
  await expect(page.getByTestId('sidebar')).not.toBeInViewport();
  await page.screenshot({ path: path.join(folder, `${name}-390.png`) });
  await page.setViewportSize(wide);
}

/** Makes the welcome board for whoever `api` is signed in as, by a request that accepts `language`, and answers where it is. */
async function welcomeBoard(
  api: APIRequestContext,
  workspaceId: string,
  language: string,
): Promise<{ workspaceId: string; boardId: string }> {
  const answer = await api.post(`/api/v1/w/${workspaceId}/onboard`, {
    headers: { 'Accept-Language': language },
  });
  expect(answer.ok()).toBe(true);
  const { boardId } = (await answer.json()) as Schemas['WelcomeBoard'];
  return { workspaceId, boardId };
}

/** The welcome board whoever `api` is signed in as was given as they came into the workspace: one, of that title. */
async function givenWelcomeBoard(
  api: APIRequestContext,
  workspaceId: string,
  title: string,
): Promise<{ workspaceId: string; boardId: string }> {
  const answer = await api.get(`/api/v1/w/${workspaceId}/boards`);
  expect(answer.ok()).toBe(true);
  const given = ((await answer.json()) as Schemas['Container'][]).filter(
    (board) => board.title === title,
  );
  expect(given).toHaveLength(1);
  return { workspaceId, boardId: given[0]?.id ?? '' };
}

test('the welcome board shows its five views, its pictures and its document, in English and in Korean', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('welcomed');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Before the welcome' });
  // A member has the board from the moment a link let them in; nothing asks for it.
  const english = await givenWelcomeBoard(api, workspaceId, 'Getting started with fvoci');

  const errors = watchErrors(page);
  await page.goto(boardPath(english));
  await signInOnPage(page, owner);
  await expect(boardLink(page, 'Getting started with fvoci')).toHaveAttribute(
    'aria-current',
    'page',
  );

  // Five views, each of its own kind, behind the button that names the one shown. The list
  // is in the order of their titles, and the board is the first: what the welcome board
  // opens on.
  const views = page.getByTestId('view-header').locator('[data-tool="views"]');
  const listed = page.getByRole('menu', { name: 'Views' }).locator('a[role="menuitem"]');
  await expect(views).toHaveText('Board');
  await views.click();
  await expect(listed).toHaveText(['Board', 'Calendar', 'Gallery', 'Gantt', 'Table']);
  await expect(listed.first()).toHaveAttribute('aria-current', 'page');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('kanban')).toBeVisible();
  await expect(page.getByText('Open a card', { exact: true })).toBeVisible();
  await shot(page, 'welcome-en-board');
  // A window of 1,000 cuts no name of a view, beside the sidebar or with it folded to its
  // strip.
  await page.setViewportSize({ width: 1000, height: 800 });
  await expect(page.getByTestId('view-header')).toHaveAttribute('data-layout', 'row');
  await expect.poll(() => headCuts(page)).toEqual({ cut: [], scroll: 0 });
  await sidebar(page).getByRole('button', { name: 'Collapse sidebar' }).click();
  await expect(views).toHaveText('Board');
  await expect.poll(() => headCuts(page)).toEqual({ cut: [], scroll: 0 });
  await page.getByTestId('expand-sidebar').click();
  await page.setViewportSize({ width: 1280, height: 720 });
  for (const [view, shown] of [
    ['Calendar', 'calendar'],
    ['Gallery', 'gallery'],
    ['Gantt', 'gantt'],
    ['Table', 'table-view'],
  ] as const) {
    await views.click();
    await listed.filter({ hasText: view }).click();
    await expect(views).toHaveText(view);
    await expect(page.getByTestId(shown)).toBeVisible();
    if (view === 'Gallery') {
      // The first picture of a body is the face of its card: a file of this board's own.
      const picture = page.getByTestId('gallery').locator('img').first();
      await expect
        .poll(() => picture.evaluate((image: HTMLImageElement) => image.naturalWidth))
        .toBeGreaterThan(0);
    }
    await shot(page, `welcome-en-${view.toLowerCase()}`);
  }

  // The document of a card opens in the viewer, where it is.
  await views.click();
  await listed.filter({ hasText: 'Board' }).click();
  await page.getByText('Attach files and open them right here', { exact: true }).click();
  const block = page.locator('[data-file-block]');
  await expect(block).toHaveAttribute('data-state', 'shown');
  await shot(page, 'welcome-en-card');
  await block.getByRole('button', { name: 'View Try the viewer.docx' }).click();
  const viewer = page.getByTestId('file-viewer');
  await expect(viewer).toHaveAttribute('data-status', 'shown');
  await expect(
    page.frameLocator('[data-testid="file-viewer-frame"]').locator('section.docx'),
  ).toContainText('What you are looking at');
  await shot(page, 'welcome-en-viewer');
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer).toHaveCount(0);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  expect(errors).toEqual([]);

  // The same account asks for one on a Korean screen: the board is written in Korean.
  const korean = await welcomeBoard(api, workspaceId, 'ko');
  const context = await browser.newContext({
    locale: 'ko-KR',
    baseURL: base,
    storageState: await page.context().storageState(),
  });
  const koreanPage = await context.newPage();
  const koreanErrors = watchErrors(koreanPage);
  await koreanPage.goto(boardPath(korean));
  await expect(boardLink(koreanPage, 'fvoci 시작하기')).toHaveAttribute('aria-current', 'page');
  await koreanPage.getByTestId('view-header').locator('[data-tool="views"]').click();
  await expect(
    koreanPage.getByRole('menu', { name: '뷰' }).locator('a[role="menuitem"]'),
  ).toHaveText(['가이드', '간트', '갤러리', '달력', '표']);
  await koreanPage.keyboard.press('Escape');
  await expect(koreanPage.getByText('카드를 열어 보세요', { exact: true })).toBeVisible();
  await shot(koreanPage, 'welcome-ko-board');
  await koreanPage.getByText('파일을 올리고 그 자리에서 여세요', { exact: true }).click();
  await expect(koreanPage.locator('[data-file-block]')).toHaveAttribute('data-state', 'shown');
  await shot(koreanPage, 'welcome-ko-card');
  expect(koreanErrors).toEqual([]);
  await context.close();
  await api.dispose();
});

/** The view each of the other templates opens on: the test id of what it draws. */
const OPENS_ON: Record<string, string> = {
  'project-schedule': 'gantt',
  'issue-tracker': 'kanban',
  'meeting-notes': 'table-view',
  'content-calendar': 'calendar',
  'customer-contacts': 'table-view',
  'study-notes': 'kanban',
};

test('a board from each of the other templates opens on the view meant for it, in English and in Korean', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('browsing');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Before the templates' });

  /** A board of the account's own from each template written in `language`, by key. */
  const boardsFrom = async (language: string): Promise<[string, string][]> => {
    const made: [string, string][] = [];
    for (const template of await templates(api, workspaceId, language)) {
      const key = template.properties.templateKey;
      if (!template.isBuiltin || typeof key !== 'string' || !(key in OPENS_ON)) continue;
      const copied = await api.post(
        `/api/v1/w/${workspaceId}/containers/${template.id}/duplicate`,
        {
          params: { toWorkspace: workspaceId },
        },
      );
      expect(copied.ok()).toBe(true);
      const copy = ((await copied.json()) as Schemas['ContainersAndBlocks']).containers[0];
      made.push([key, copy?.id ?? '']);
    }
    expect(made.map(([key]) => key).sort()).toEqual(Object.keys(OPENS_ON).sort());
    return made;
  };

  const errors = watchErrors(page);
  let signedInOnPage = false;
  for (const [key, boardId] of await boardsFrom('en')) {
    await page.goto(boardPath({ workspaceId, boardId }));
    if (!signedInOnPage) await signInOnPage(page, owner);
    signedInOnPage = true;
    await expect(page.getByTestId(OPENS_ON[key] ?? ''), key).toBeVisible();
    await expect(page.locator('[data-problem]')).toHaveCount(0);
    await shot(page, `${key}-en`);
  }
  expect(errors).toEqual([]);

  const context = await browser.newContext({
    locale: 'ko-KR',
    baseURL: base,
    storageState: await page.context().storageState(),
  });
  const koreanPage = await context.newPage();
  const koreanErrors = watchErrors(koreanPage);
  for (const [key, boardId] of await boardsFrom('ko')) {
    await koreanPage.goto(boardPath({ workspaceId, boardId }));
    await expect(koreanPage.getByTestId(OPENS_ON[key] ?? ''), key).toBeVisible();
    await expect(koreanPage.locator('[data-problem]')).toHaveCount(0);
    await shot(koreanPage, `${key}-ko`);
  }
  expect(koreanErrors).toEqual([]);
  await context.close();
  await api.dispose();
});
