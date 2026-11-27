/**
 * What the store of an open wiki (`wiki.svelte.ts`) leans on: making a wiki,
 * and a step of undo on a page that is gone since.
 */
import type { Container } from '../logic/wire.ts';

import { isBackendError } from './backend.ts';
import type { WikiDraft, WikiPage, Wikis } from './wiki-types.ts';
import type { PageLooks, WikiLooks } from './wiki.svelte.ts';
import type { Workspace } from './workspace.svelte.ts';

/** Joins an open wiki and reloads it on success; handles 404 by evicting cache and marking missing. */
export async function joinOpenWiki(host: {
  readonly wikiId: string;
  readonly workspace: Workspace;
  joining: boolean;
  missing: boolean;
  readonly status: string;
  isClosed(): boolean;
  load(): Promise<void>;
}): Promise<boolean> {
  if (host.joining) return false;
  host.joining = true;
  try {
    await host.workspace.backend.joinContainer(host.wikiId);
  } catch (error) {
    if (isBackendError(error) && error.status === 404) {
      host.workspace.evictContainer(host.wikiId);
      host.missing = true;
    } else host.workspace.report('save', error);
    return false;
  } finally {
    host.joining = false;
  }
  if (host.isClosed()) return false;
  await host.load();
  return host.status === 'ready';
}

/** Inverse of a page title/icon patch, for undo and refusal rollback. */
export function pageLooksBack(before: WikiPage, looks: PageLooks): PageLooks {
  return {
    ...(looks.title === undefined ? {} : { title: before.title }),
    ...(looks.icon === undefined ? {} : { icon: before.icon }),
  };
}

/** Inverse of a wiki metadata patch, for undo and refusal rollback. */
export function wikiLooksBack(before: Container, looks: WikiLooks): WikiLooks {
  return {
    ...(looks.title === undefined ? {} : { title: before.title }),
    ...(looks.description === undefined ? {} : { description: before.description }),
    ...(looks.icon === undefined ? {} : { icon: before.icon ?? '' }),
    ...(looks.visibility === undefined ? {} : { visibility: before.visibility }),
    ...(looks.minimumRole === undefined ? {} : { minimumRole: before.minimumRole }),
  };
}

/** Optimistically updates a page's title or icon with undo and rollback on refusal. */
export async function performPageLooks(
  host: {
    readonly wikiId: string;
    readonly workspace: Workspace;
    pages: WikiPage[];
    readonly openPage: { readonly pageId: string; meta: WikiPage | undefined } | undefined;
    page(pageId: string | undefined): WikiPage | undefined;
    receivePage(page: WikiPage): void;
    writeJoined<T>(write: () => Promise<T>): Promise<T>;
  },
  wikis: Wikis,
  pageId: string,
  looks: PageLooks,
): Promise<void> {
  const before =
    host.page(pageId) ?? (host.openPage?.pageId === pageId ? host.openPage.meta : undefined);
  if (!before) return;
  const back = pageLooksBack(before, looks);
  if (Object.entries(looks).every(([key, value]) => back[key as keyof PageLooks] === value)) return;
  const write = async (to: PageLooks, from: PageLooks): Promise<void> => {
    const show = (patch: PageLooks): void => {
      host.pages = host.pages.map((page) => (page.id === pageId ? { ...page, ...patch } : page));
      if (host.openPage?.pageId === pageId && host.openPage.meta) {
        host.openPage.meta = { ...host.openPage.meta, ...patch };
      }
    };
    show(to);
    try {
      host.receivePage(await host.writeJoined(() => wikis.patchPage(host.wikiId, pageId, to)));
    } catch (error) {
      show(from);
      throw error;
    }
  };
  try {
    await host.workspace.undo.perform(
      () => unlessGone(() => write(looks, back)),
      () => unlessGone(() => write(back, looks)),
      looks.title === undefined ? 'changePageIcon' : 'changePageTitle',
    );
  } catch (error) {
    host.workspace.report('save', error);
  }
}

/** Optimistically updates a wiki's metadata with undo and rollback on refusal. */
export async function performWikiLooks(
  host: {
    readonly wikiId: string;
    readonly workspace: Workspace;
    wiki: Container | undefined;
    takeWiki(wiki: Container): void;
  },
  looks: WikiLooks,
  step: 'changeWiki' | 'changeVisibility' | 'changeMinimumRole',
): Promise<void> {
  const before = host.wiki;
  if (!before) return;
  const back = wikiLooksBack(before, looks);
  const write = async (to: WikiLooks, from: WikiLooks): Promise<void> => {
    if (host.wiki) host.wiki = { ...host.wiki, ...to };
    try {
      const changed = await host.workspace.backend.patchContainer(host.wikiId, to);
      host.takeWiki(changed);
      host.workspace.receiveContainer(changed);
    } catch (error) {
      if (host.wiki) host.wiki = { ...host.wiki, ...from };
      throw error;
    }
  };
  try {
    await host.workspace.undo.perform(
      () => write(looks, back),
      () => write(back, looks),
      step,
    );
  } catch (error) {
    host.workspace.report('save', error);
  }
}

/** A step of undo on a page deleted since (by the reader too): nothing is left to do. */
export async function unlessGone(step: () => Promise<void>): Promise<void> {
  try {
    await step();
  } catch (error) {
    if (!isBackendError(error) || error.status !== 404) throw error;
  }
}

/**
 * Makes a wiki in the workspace shown, in the sidebar at once; undefined when
 * it was refused (told) or the backend has no wikis.
 */
export async function createWiki(
  workspace: Workspace,
  draft: WikiDraft,
): Promise<Container | undefined> {
  const wikis = workspace.backend.wiki;
  const shown = workspace.info;
  if (!wikis || !shown) return undefined;
  try {
    const made = await wikis.createWiki(shown.id, draft);
    workspace.receiveContainer(made);
    return made;
  } catch (error) {
    workspace.report('save', error);
    return undefined;
  }
}
