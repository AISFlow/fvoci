/**
 * What the shared tree components (`PageTree`, `MovePageDialog`, `PageViewers`)
 * read from a store: implemented by both `WikiStore` and `CourseStore`.
 */
import type { PageTreeNode, PlacedPage } from '../logic/page-tree.ts';

import type { ContainerPresence } from './board-presence.svelte.ts';
import type { UnitKind } from './course-types.ts';
import type { Workspace } from './workspace.svelte.ts';

/** A node of a tree: a wiki page, or a course's chapter or unit. */
export interface TreeItem extends PlacedPage {
  title: string;
  icon?: string;
  /** Present on course nodes (`'chapter'` or `'unit'`); omitted on wiki pages. */
  kind?: UnitKind;
}

/** What `PageViewers` needs to show who else is on a page or unit. */
export interface PresenceSource {
  readonly workspace: Workspace;
  readonly presence: ContainerPresence;
}

/**
 * The five tree attributes plus presence/workspace that `PageTree` and
 * `MovePageDialog` read from a store (`WikiStore`, `CourseStore`).
 */
export interface TreeSource<P extends TreeItem> extends PresenceSource {
  readonly tree: readonly PageTreeNode<P>[];
  readonly pages: readonly P[];
  readonly canEdit: boolean;
  readonly settling: boolean;
  page(id: string | undefined): P | undefined;
}
