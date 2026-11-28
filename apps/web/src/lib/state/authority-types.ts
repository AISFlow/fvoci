/**
 * Server-authority types and backend surface extensions: caller-scoped
 * `ContainerPermissions`, atomic board view/card operations (one property
 * value on many cards among them), cross-workspace
 * container copy, and the account/workspace `'Today'` hub.
 */
import type { BoardView, Container, MemberRole, PropertyValue } from '../logic/wire.ts';

import type { CardPatch, CardSummary, CardTelling, Comment, ViewDraft } from './backend.ts';
import type { LearningTodayItem, TodayReviewItem } from './course-types.ts';

/** Effective permissions of the current caller on a single container (`my-permissions`). */
export interface ContainerPermissions {
  canEdit: boolean;
  canComment: boolean;
  canManageProperties: boolean;
  canManageVisibility: boolean;
  canManageRoles: boolean;
  canDelete: boolean;
  canShare: boolean;
  canManageOthersComments: boolean;
}

export const READ_ONLY_PERMISSIONS: ContainerPermissions = {
  canEdit: false,
  canComment: false,
  canManageProperties: false,
  canManageVisibility: false,
  canManageRoles: false,
  canDelete: false,
  canShare: false,
  canManageOthersComments: false,
};

/** Derives fallback `ContainerPermissions` from a local `MemberRole` before or alongside server reads. */
export function permissionsOfRole(
  role: MemberRole,
  options: { isGuest?: boolean; isWorkspaceAdmin?: boolean; isTemplate?: boolean } = {},
): ContainerPermissions {
  const effective: MemberRole = options.isGuest && role === 'admin' ? 'editor' : role;
  const isAdmin = effective === 'admin';
  const isEditor = isAdmin || effective === 'editor';
  const isCommenter = isEditor || effective === 'commenter';
  const wsAdmin = Boolean(options.isWorkspaceAdmin) && !options.isGuest;
  const isTemplate = Boolean(options.isTemplate);
  return {
    canEdit: isEditor,
    canComment: isCommenter && !isTemplate,
    canManageProperties: isEditor,
    canManageVisibility: isAdmin,
    canManageRoles: isAdmin || wsAdmin,
    canDelete: isAdmin || wsAdmin,
    canShare: isAdmin && !isTemplate,
    canManageOthersComments: isAdmin && !isTemplate,
  };
}

/** Result of creating, moving, or duplicating a board view in one fractional-indexing transaction. */
export interface BoardViewWriteResult {
  view: BoardView;
  companionViews: BoardView[];
}

/** Atomic multi-card move request (`POST /boards/{boardId}/views/{viewId}/card-moves`). */
export interface CardMovesInput {
  cardIds: readonly string[];
  groupPropertyId?: string | null;
  groupValue?: PropertyValue | null;
  afterCardId?: string | null;
}

export interface CardMovesOutput {
  view: BoardView;
  cards: CardSummary[];
}

/** The most cards one request of the board's batch routes takes (`batchPatchCards`, `batchDeleteCards`, card moves). */
export const CARD_BATCH_LIMIT = 200;

/** What `batchPatchCards` sets and clears on every card it is given alike: property values only. */
export type CardPropertiesPatch = Pick<CardPatch, 'updatedProperties' | 'deletedProperties'>;

/** Request and response for `POST /w/{ws}/containers/{containerId}/copy-to-workspace`. */
export interface CopyToWorkspaceInput {
  targetWorkspaceId: string;
  title?: string | null;
}

export interface CopiedContainerOutput {
  container: Container;
  missingFilesCount: number;
}

export interface TodayWorkspaceMeta {
  workspaceId: string;
  workspaceName: string;
  orgId: string;
  orgName: string;
  isPersonal: boolean;
}

export interface TodayCardItem {
  workspace: TodayWorkspaceMeta;
  boardId: string;
  boardTitle: string;
  boardIcon: string;
  cardId: string;
  cardNumber?: number | null;
  title: string;
  icon: string;
  dueDate: string;
  dueAtMillis?: number | null;
  overdueDays: number;
}

export type TodayUnitEntry = LearningTodayItem & { workspace: TodayWorkspaceMeta };
export type TodayReviewEntry = TodayReviewItem & { workspace: TodayWorkspaceMeta };

export interface TodayMeetingItem {
  workspace: TodayWorkspaceMeta;
  containerId: string;
  courseTitle: string;
  meetingId: string;
  title: string;
  startsAt: number;
  endsAt?: number | null;
  location: string;
}

export interface AccountToday {
  today: string;
  lockedWorkspaceCount: number;
  cards: TodayCardItem[];
  hasMoreCards: boolean;
  units: TodayUnitEntry[];
  hasMoreUnits: boolean;
  reviews: TodayReviewEntry[];
  hasMoreReviews: boolean;
  meetings: TodayMeetingItem[];
  hasMoreMeetings: boolean;
}

export interface WorkspaceToday {
  today: string;
  workspace: TodayWorkspaceMeta;
  cards: TodayCardItem[];
  hasMoreCards: boolean;
  units: TodayUnitEntry[];
  hasMoreUnits: boolean;
  reviews: TodayReviewEntry[];
  hasMoreReviews: boolean;
  meetings: TodayMeetingItem[];
  hasMoreMeetings: boolean;
}

/** Server-authority endpoints on `Backend`. */
export interface ServerAuthorityOfBackend {
  /** Number of days deleted items stay in the trash before permanent deletion (`ClientConfig`). */
  trashRetentionDays(): Promise<number>;
  /** Caller-specific effective permissions on `containerId` (`GET .../my-permissions`). */
  myPermissions(containerId: string): Promise<ContainerPermissions>;
  /** Copies a live board, wiki, or course into `request.targetWorkspaceId`. */
  copyContainerToWorkspace(
    containerId: string,
    request: CopyToWorkspaceInput,
  ): Promise<CopiedContainerOutput>;
  /** Account-wide `'Today'` hub summary across session-opened workspaces. */
  accountToday(): Promise<AccountToday>;
  /** Single-workspace `'Today'` summary. */
  workspaceToday(workspaceId: string): Promise<WorkspaceToday>;
  /** Creates a board view via fractional-indexing (`POST /boards/{boardId}/views`). */
  writeView(boardId: string, draft: ViewDraft): Promise<BoardViewWriteResult>;
  /** Moves a board view right after `after` (`""` or `null` for start). */
  moveView(boardId: string, viewId: string, after: string | null): Promise<BoardViewWriteResult>;
  /** Duplicates a board view (`POST /boards/{boardId}/views/{viewId}/duplicate`). */
  duplicateView(
    boardId: string,
    viewId: string,
    title?: string,
    after?: string | null,
  ): Promise<BoardViewWriteResult>;
  /** Atomically duplicates a card and its files (`POST /boards/{boardId}/cards/{cardId}/duplicate`). */
  duplicateCard(boardId: string, cardId: string, asTemplate?: boolean): Promise<CardSummary>;
  /** Atomically moves cards in a view and optionally updates their grouping property. */
  moveViewCards(
    boardId: string,
    viewId: string,
    move: CardMovesInput,
    telling?: CardTelling,
  ): Promise<CardMovesOutput>;
  /**
   * Sets and clears the same property values on `1..=200` distinct cards of
   * a board, all or none (`POST /boards/{boardId}/cards/batch-patch`): each
   * card as `patchCard` with only those values would change it. The cards as
   * they are then, in the order of `cardIds`.
   */
  batchPatchCards(
    boardId: string,
    cardIds: readonly string[],
    patch: CardPropertiesPatch,
    telling?: CardTelling,
  ): Promise<CardSummary[]>;
  /** Deletes up to 200 cards in a single transaction as individually restorable entries. */
  batchDeleteCards(boardId: string, cardIds: readonly string[]): Promise<void>;
  /** Restores up to 200 deleted cards in a single transaction. */
  batchRestoreCards(boardId: string, cardIds: readonly string[]): Promise<CardSummary[]>;
  /** Deletes a card comment via the formal card comments endpoint. */
  deleteComment(boardId: string, cardId: string, commentId: string): Promise<void>;
  /** Restores a deleted card comment via the formal card comments endpoint. */
  restoreComment(boardId: string, cardId: string, commentId: string): Promise<Comment>;
}
