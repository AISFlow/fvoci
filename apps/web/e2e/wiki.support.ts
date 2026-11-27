/**
 * What the tests of the wikis share: making one through "New", its address,
 * and where its parts are on the page.
 */
import { expect, type Locator, type Page } from '@playwright/test';

import { WORKSPACE } from './address.ts';
import { showSidebar, sidebar } from './shell.support.ts';

const ID = '[0-9a-f-]{36}';

/** A wiki of the world in memory, and a page of it. */
export function wikiPath(wikiId: string, pageId?: string): string {
  return `/w/${WORKSPACE}/wikis/${wikiId}${pageId ? `/${pageId}` : ''}`;
}

/** Any wiki's address, with or without a page. */
export const ON_A_WIKI = new RegExp(`/w/${ID}/wikis/(${ID})(/(${ID}))?$`);
/** Any page of any wiki. */
export const ON_A_PAGE = new RegExp(`/w/${ID}/wikis/(${ID})/(${ID})$`);

/** The wiki and the page an address names; empty strings for what it does not. */
export function wikiPlaceOf(address: string): { wikiId: string; pageId: string } {
  const found = ON_A_WIKI.exec(new URL(address, 'http://here').pathname);
  return { wikiId: found?.[1] ?? '', pageId: found?.[3] ?? '' };
}

export const wikiScreen = (page: Page): Locator => page.getByTestId('wiki-screen');

/** Makes a wiki through "New" and waits for its page; gives its identifier. */
export async function newWiki(page: Page, title: string, open = true): Promise<string> {
  await showSidebar(page);
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await picker.getByTestId('new-title').fill(title);
  await picker.getByTestId(open ? 'new-visibility-open' : 'new-visibility-private').check();
  await picker.getByTestId('new-wiki').click();
  await expect(page).toHaveURL(ON_A_WIKI);
  await expect(wikiScreen(page)).toBeVisible();
  return wikiPlaceOf(page.url()).wikiId;
}

/**
 * Goes to an address inside the page, as a link of the app would: the world
 * in memory starts anew on a load, and with it whatever a test made.
 */
export async function goInside(page: Page, path: string): Promise<void> {
  await page.evaluate((address) => {
    history.pushState(null, '', address);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, path);
}

/** The tree of pages: its items as drawn, each with its level and name. */
export const pageTree = (page: Page): Locator => page.getByTestId('page-tree');

export async function treeShape(page: Page): Promise<string[]> {
  return pageTree(page)
    .getByRole('treeitem')
    .evaluateAll((items) =>
      items.map(
        (item) =>
          `${'  '.repeat(Number(item.getAttribute('aria-level')) - 1)}${item.getAttribute('aria-label') ?? ''}`,
      ),
    );
}

/** A page of the tree by its name. */
export const treeItem = (page: Page, name: string): Locator =>
  pageTree(page).getByRole('treeitem', { name, exact: true });

/** Gives the page shown its title, as typed, and leaves the field. */
export async function nameThePage(page: Page, title: string): Promise<void> {
  const field = page.getByTestId('page-title');
  await expect(field).toBeFocused();
  await page.keyboard.type(title);
  await page.keyboard.press('Enter');
  // In the tree, which may be a drawer that is away (inert, so out of the roles' reach).
  await expect(pageTree(page).locator(`[role="treeitem"][aria-label="${title}"]`)).toBeAttached();
}

/** Brings the tree into view: nothing to do in a wide pane, the drawer in a narrow one. */
export async function showTree(page: Page): Promise<void> {
  const opener = page.getByTestId('open-page-tree');
  // Not when the drawer is out already (the button is behind it then).
  if ((await opener.isVisible()) && (await opener.getAttribute('aria-expanded')) !== 'true') {
    await opener.click();
  }
  await expect(pageTree(page)).toBeVisible();
}

/**
 * Puts the tree's drawer away, where it is one: Escape, as a keyboard would,
 * in any language; done once it has slid all the way out.
 */
export async function hideTree(page: Page): Promise<void> {
  const opener = page.getByTestId('open-page-tree');
  if (!(await opener.isVisible()) || (await opener.getAttribute('aria-expanded')) !== 'true') {
    return;
  }
  await page.getByTestId('page-tree-panel').press('Escape');
  await expect(opener).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByTestId('page-tree-panel')).toBeHidden();
}

/** Opens a page's menu in the tree and presses one of its lines. */
export async function pageAction(page: Page, name: string, action: string): Promise<void> {
  await showTree(page);
  await pageTree(page)
    .getByRole('button', { name: `Menu of the page ${name}` })
    .click();
  await page.locator(`[data-action="${action}"]`).click();
}

/** The editor of the page shown's body. */
export const pageBody = (page: Page): Locator =>
  page.getByTestId('page-body').locator('[contenteditable]').first();

/**
 * What the tests ask of "somebody else" in the world in memory (`fvociDev`):
 * the same data as another person of the workspace, through the same backend
 * as the app's (only what these tests use is named here).
 */
interface Yish {
  length: number;
  get(index: number): Yish | undefined;
  insert(index: number, text: string): void;
  toString(): string;
}
interface WikiDev {
  backend: {
    me(): Promise<{ id: string }>;
    addMember(boardId: string, userId: string, role: string): Promise<unknown>;
  };
  someoneElse(): {
    me(): Promise<{ id: string }>;
    addMember(boardId: string, userId: string, role: string): Promise<unknown>;
    wiki: {
      createWiki(workspaceId: string, draft: object): Promise<{ id: string }>;
      createPage(wikiId: string, draft: object): Promise<{ id: string }>;
      deletePage(wikiId: string, pageId: string): Promise<void>;
      restorePage(wikiId: string, pageId: string): Promise<unknown>;
    };
    presence: { setPresence(visit: object | null): void };
    openDoc(
      workspaceId: string,
      boardId: string,
      cardId: string,
      listener: { state(state: object): void; discarded(why: string): void },
    ): { state: { doc?: { getXmlFragment(name: string): Yish } } };
  };
}
interface DevWindow {
  fvociDev: WikiDev;
}

/** Makes somebody else of the workspace an editor of the wiki, and gives their identifier. */
export async function letSomebodyElseIn(page: Page, wikiId: string): Promise<string> {
  return page.evaluate(async (id) => {
    const dev = (window as unknown as DevWindow).fvociDev;
    const other = await dev.someoneElse().me();
    await dev.backend.addMember(id, other.id, 'editor');
    return other.id;
  }, wikiId);
}

/**
 * Somebody else opens the page's body in its room, waits until it has
 * `waitFor`, adds `typed` at the end of its first paragraph, and gives the
 * body's text as their room has it then. They stay on the page (their visit).
 */
export async function typeAsSomebodyElse(
  page: Page,
  place: { wikiId: string; pageId: string },
  waitFor: string,
  typed: string,
): Promise<string> {
  return page.evaluate(
    async ({ wikiId, pageId, waitFor: expected, typed: text }) => {
      const workspaceId = location.pathname.split('/')[2] ?? '';
      const other = (window as unknown as DevWindow).fvociDev.someoneElse();
      other.presence.setPresence({ workspaceId, containerId: wikiId, cardId: pageId });
      const opened = other.openDoc(workspaceId, wikiId, pageId, {
        state: () => undefined,
        discarded: () => undefined,
      });
      const deadline = Date.now() + 5000;
      for (;;) {
        const body = opened.state.doc?.getXmlFragment('default');
        if (body?.toString().includes(expected)) {
          const words = body.get(0)?.get(0);
          words?.insert(words.length, text);
          return body.toString();
        }
        if (Date.now() > deadline) return body?.toString() ?? '(no document)';
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    },
    { ...place, waitFor, typed },
  );
}

/** Somebody else makes a page of the wiki, below `parentId` if given; gives its identifier. */
export async function pageBySomebodyElse(
  page: Page,
  wikiId: string,
  draft: { title: string; parentId?: string },
): Promise<string> {
  return page.evaluate(
    async ({ id, made }) =>
      (await (window as unknown as DevWindow).fvociDev.someoneElse().wiki.createPage(id, made)).id,
    { id: wikiId, made: draft },
  );
}

/** Somebody else deletes a page of the wiki, with those below it. */
export async function deleteBySomebodyElse(page: Page, wikiId: string, pageId: string) {
  await page.evaluate(
    async ({ id, gone }) => {
      await (window as unknown as DevWindow).fvociDev.someoneElse().wiki.deletePage(id, gone);
    },
    { id: wikiId, gone: pageId },
  );
}

/** Somebody else brings a page of the wiki back from the trash, with those below it. */
export async function restoreBySomebodyElse(page: Page, wikiId: string, pageId: string) {
  await page.evaluate(
    async ({ id, back }) => {
      await (window as unknown as DevWindow).fvociDev.someoneElse().wiki.restorePage(id, back);
    },
    { id: wikiId, back: pageId },
  );
}

/**
 * Somebody else makes a wiki with a page, open to the workspace (`'open'`, to be
 * joined) or for the invited (`'private'`), the reader in it as `readerRole` if given;
 * gives the wiki's and the page's identifiers.
 */
export async function wikiBySomebodyElse(
  page: Page,
  plan: { title: string; visibility: 'open' | 'private'; page: string; readerRole?: string },
): Promise<{ wikiId: string; pageId: string }> {
  return page.evaluate(async (made) => {
    const dev = (window as unknown as DevWindow).fvociDev;
    const other = dev.someoneElse();
    const workspaceId = location.pathname.split('/')[2] ?? '';
    const wiki = await other.wiki.createWiki(workspaceId, {
      title: made.title,
      visibility: made.visibility,
    });
    const first = await other.wiki.createPage(wiki.id, { title: made.page });
    if (made.readerRole) {
      const reader = await dev.backend.me();
      await other.addMember(wiki.id, reader.id, made.readerRole);
    }
    return { wikiId: wiki.id, pageId: first.id };
  }, plan);
}
