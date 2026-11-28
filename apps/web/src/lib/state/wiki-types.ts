/**
 * The wikis of a workspace and their pages, part of the data boundary
 * (`backend.ts`, `Backend.wiki`).
 *
 * A wiki is a board of the kind `wiki` (`Board.kind`): it is read, changed,
 * deleted, brought back, joined and filed in the sidebar as a board is, by
 * the board's operations. What only a wiki has is here: its pages, a tree
 * (each names the page it is below), each with a title, an icon and a body.
 * The body is written together in the page's room, as a card's is
 * (`Backend.openDoc` with the page in place of the card). A page has
 * comments and versions of its body as a card has, by the card's rules.
 */
import type { DocNode } from '../editor/schema.ts';
import type { Container, ContainerVisibility, MemberRole } from '../logic/wire.ts';

import type { BodyWrite, Comment, CommentVersion } from './backend.ts';
import type { BodyVersions } from './body-versions.ts';
import type { WithReactions } from './reaction-types.ts';
import type { DeletedItem, Paging } from './trash-reads.ts';

/** A page as the tree of its wiki lists it: without its body. */
export interface WikiPage {
  id: string;
  wikiId: string;
  /** The page it is below; empty at the top of the wiki. */
  parentId: string;
  title: string;
  /** At most one grapheme; empty for none. */
  icon: string;
  /**
   * Its place among the pages of the same parent: they are in the order of
   * these as strings compare by code unit, then of `id` (`logic/page-tree.ts`).
   * The backend makes it; the screens only sort by it.
   */
  order: string;
  createdAt: number;
  updatedAt: number;
}

/** A page read on its own: with its body and who made and last changed it. */
export interface WikiPageDetail extends WikiPage {
  body: DocNode | null;
  createdBy: string;
  modifiedBy: string;
}

/** What a wiki is made from. Whoever makes it is its admin. */
export interface WikiDraft {
  title: string;
  description?: string;
  icon?: string;
  /** `open`: for whoever works in the workspace to find and join; `private`: for whom it is shared with. */
  visibility: ContainerVisibility;
  minimumRole?: Exclude<MemberRole, '' | 'admin'>;
}

/** A page to make; the backend gives it its identifier, times and place (`order`). */
export interface PageDraft {
  title?: string;
  icon?: string;
  /** The page it goes below, of the same wiki; empty or left out, the top of the wiki. */
  parentId?: string;
  /**
   * The page it goes right after, of the same parent: empty for the first
   * place, left out for the last.
   */
  after?: string;
}

/**
 * Where a page moves to, with every page below it, within its wiki: below
 * `parentId` (a page of it; left out or empty, the top), at the place
 * `after` says, as `PageDraft.after`.
 */
export interface PageMove {
  parentId?: string;
  after?: string;
}

/** What a page is changed to; what is left out stays. Where it is in the tree is not changed here. */
export interface PagePatch {
  title?: string;
  icon?: string;
  /** A whole new body, sent with `readAt`. */
  body?: DocNode | null;
  /** The `updatedAt` the page was read at: a page changed since is refused (409) and left as it is. */
  readAt?: number;
}

export interface Wikis {
  /** The wikis of the workspace the reader can see: as the boards of `Backend.boards`, no templates. */
  wikis(workspaceId: string): Promise<Container[]>;
  /** Makes a wiki, filed under the reader's default category; its members are told. */
  createWiki(workspaceId: string, draft: WikiDraft): Promise<Container>;
  /** Every page of the wiki, in no promised order. For who reads it (an open one: who works there). */
  pages(wikiId: string): Promise<WikiPage[]>;
  page(wikiId: string, pageId: string): Promise<WithReactions<WikiPageDetail>>;
  /** For the wiki's editors. A parent or `after` that is no such page is 400. */
  createPage(wikiId: string, draft: PageDraft): Promise<WikiPageDetail>;
  patchPage(wikiId: string, pageId: string, patch: PagePatch): Promise<WikiPageDetail>;
  /**
   * Moves the page and every page below it to the trash, for the wiki's
   * editors. The wiki's listeners are told of the page alone (`blocksDeleted`).
   */
  deletePage(wikiId: string, pageId: string): Promise<void>;

  // The comments on a page, as a card's (`Backend.comments` and the rest).
  /** Oldest first. For who reads the wiki (an open one: who works there); a page that is not there is 404. */
  comments(wikiId: string, pageId: string): Promise<WithReactions<Comment>[]>;
  /** For the wiki's commenters and up. A comment has a text: an empty one is 400. */
  createComment(wikiId: string, pageId: string, body: DocNode): Promise<Comment>;
  /**
   * Replaces the text of a comment: its author's, from the commenter's role,
   * and anyone's for the wiki's admins. 409 when it changed since `readAt`.
   */
  patchComment(
    wikiId: string,
    pageId: string,
    commentId: string,
    write: BodyWrite,
  ): Promise<Comment>;
  /**
   * Moves a comment to the trash, by whom may change it. A page brought back
   * from the trash has the comments that went with it.
   */
  deleteComment(wikiId: string, pageId: string, commentId: string): Promise<void>;
  /**
   * Brings back a comment deleted on its own, onto its page: its author's if
   * they deleted it, anyone's for the wiki's admins (403 otherwise); one that
   * went with its page, or is not in the trash, is 404. Its listeners are told.
   */
  restoreComment(wikiId: string, pageId: string, commentId: string): Promise<Comment>;
  /** What a comment said before each change, the first text first. For who reads the wiki. */
  commentVersions(wikiId: string, pageId: string, commentId: string): Promise<CommentVersion[]>;
  /**
   * The versions of a page's body, as a card's: with the wiki in place of the
   * board and the page in place of the card. Restored by the wiki's editors.
   */
  readonly bodyVersions: BodyVersions;
  /**
   * The wiki's pages in the trash, for its editors: a page, the last deleted
   * first, each with how many pages below it went with it (`below`). A page
   * that went with the page it was below comes back with that page, and is
   * not listed.
   */
  deletedPages(wikiId: string, paging?: Paging): Promise<DeletedItem[]>;
  /**
   * Brings a page back from the trash with the pages that went with it, for
   * the wiki's editors: below the page it was below, or, that one gone, last
   * at the top. Past the wiki's limits it is 400 (`page_too_deep`,
   * `too_many_pages`); one the trash does not have, 404. The comments on the
   * pages come back with them. Each page that came back is told (`pages`),
   * and each comment (`blocks`); the answer is the page asked for.
   */
  restorePage(wikiId: string, pageId: string): Promise<WikiPageDetail>;
  /**
   * Moves the page with every page below it, for the wiki's editors: below
   * itself, below a page below it or deeper than the limit (`page_too_deep`)
   * is 400 at `parentId`, an `after` that is no other page there 400 at
   * `after`. Siblings numbered anew to leave it room are told (`pages`).
   */
  movePage(wikiId: string, pageId: string, to: PageMove): Promise<WikiPageDetail>;
  /**
   * Copies the page with every page below it, for the wiki's editors: the
   * copy right after the page, titled `title` (left out, the page's own), the
   * pages below it copied below it as they are, bodies and their images too;
   * not their versions or comments. Past the wiki's limits it is 400
   * (`page_too_deep`, `too_many_pages`). Each copy is told (`pages`); the
   * answer is the copy of the page.
   */
  duplicatePage(wikiId: string, pageId: string, title?: string): Promise<WikiPageDetail>;
}

/** Where a backend keeps its wikis. One by a public link reads only its wiki's tree and pages. */
export interface WikisOfBackend {
  readonly wiki?: Wikis;
}
