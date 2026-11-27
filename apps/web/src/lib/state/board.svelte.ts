/**
 * One open board: its definition, views, cards and members, kept up to date
 * while it is open, and every change a screen can make to them.
 */
import {
  buildHierarchyIndex,
  statusPropertyOf,
  type HierarchyIndex,
} from '../logic/card-hierarchy.ts';
import type { BoardGroups, NoValueLabel } from '../logic/group.ts';
import type {
  BlockPatch,
  BoardView,
  Card,
  Container,
  ContainerMember,
  MemberRole,
  PropertyTemplate,
  PropertyValue,
} from '../logic/wire.ts';

import {
  permissionsOfRole,
  READ_ONLY_PERMISSIONS,
  type CardDraft,
  type CardSummary,
  type Change,
  type ContainerPermissions,
  type ContainerRole,
  type ViewDraft,
} from './backend.ts';
import { handleBoardLoadFailure, joinOpenBoard, readBoard } from './board-load.ts';
import { joinedPermissions, JoinOnWrite, joinsOnWrite } from './join-on-write.ts';
import { noteMembership } from './memberships.svelte.ts';
import {
  changeBoardCard,
  changeBoardContainer,
  changeBoardView,
  createBoardCard,
  createBoardView,
  deleteBoardView,
  duplicateBoardCard,
  rereadBoardViews,
} from './board-mutations.ts';
import {
  cardsOfBoardView,
  filteredCardsOfBoardView,
  groupingPropertyOf,
  groupsOfView,
  narrowBoardCards,
  roleOf,
  withCardProperty,
  withVisibility,
  type CardDrop,
} from './board-parts.ts';
import {
  applyBoardChange,
  deleteBoardCards,
  performBoardAction,
  receiveBoardCard,
  refreshBoard,
  type BoardSyncInternals,
} from './board-sync.ts';
import { BoardWrites, cardWritesOf, viewWritesOf } from './board-writes.ts';
import { dropCardsOn } from './card-drop.ts';
import { CardPreviews } from './card-previews.svelte.ts';
import { CardStore } from './card.svelte.ts';
import { lastCommentsOf } from './last-comments.ts';
import { MemberWrites } from './member-writes.ts';
import { orderMembers, orderViews } from './order.ts';
import { propertyMovesOf } from './property-moves.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';
import { receiveView } from './view-moves.ts';

export type { CardDrop } from './board-parts.ts';

export class BoardStore {
  readonly kind = 'board' as const;
  readonly workspace: Workspace;
  readonly boardId: string;

  get containerId(): string {
    return this.boardId;
  }
  get container(): Container | undefined {
    return this.board;
  }
  set container(board: Container | undefined) {
    this.board = board;
  }

  status = $state<LoadStatus>('idle');
  missing = $state(false);
  board = $state.raw<Container | undefined>(undefined);
  views = $state.raw<BoardView[]>([]);
  cards = $state.raw<CardSummary[]>([]);
  #members = $state.raw<ContainerMember[]>([]);
  /** Its members as read; whether the reader is one is told to the sidebar (`noteMembership`). */
  get members(): ContainerMember[] {
    return this.#members;
  }
  set members(members: ContainerMember[]) {
    this.#members = members;
    noteMembership(this.workspace, this.boardId, members);
  }
  permissions = $state.raw<ContainerPermissions>({ ...READ_ONLY_PERMISSIONS });
  permissionsRole = $state<MemberRole | undefined>(undefined);
  openCard = $state.raw<CardStore | undefined>(undefined);

  readonly orderedViews = $derived.by(() => orderViews(this.views, this.workspace.locale));
  readonly realCards = $derived(this.cards.filter((card) => !card.fields.isTemplate));
  readonly cardsById: ReadonlyMap<string, CardSummary> = $derived(
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- made anew from the cards, never mutated
    new Map(this.realCards.map((card) => [card.id, card])),
  );
  readonly hierarchy: HierarchyIndex<CardSummary> = $derived(buildHierarchyIndex(this.realCards));
  readonly statusProperty = $derived(statusPropertyOf(this.board?.cardProperties ?? []));

  readonly orderedMembers = $derived.by(() => {
    const nameOf = (id: string) => this.workspace.userName(id) ?? '';
    return orderMembers(this.members, nameOf, this.workspace.locale);
  });
  readonly people = $derived(
    this.orderedMembers.flatMap((member) => this.workspace.user(member.userId) ?? []),
  );

  readonly role = $derived<MemberRole>(
    roleOf(this.members.find((member) => member.userId === this.workspace.me?.id)),
  );
  readonly joinable = $derived(
    this.status === 'ready' &&
      this.board?.visibility === 'open' &&
      !this.board.isTemplate &&
      this.role === '',
  );
  /** Joined by the reader's first write (`join-on-write.ts`): what they may do is a member's. */
  readonly joinsOnWrite = $derived.by(() =>
    joinsOnWrite(this.joinable, this.workspace, this.board?.minimumRole),
  );
  /** What the reader may do: as joined where a write joins, else as the role last read. */
  readonly effectivePermissions = $derived.by<ContainerPermissions>(() => {
    const workspaceAdmin = this.workspace.info?.role === 'admin';
    if (this.joinsOnWrite) return joinedPermissions(this.board?.minimumRole, workspaceAdmin);
    if (this.permissionsRole === this.role) return this.permissions;
    return permissionsOfRole(this.role, {
      isGuest: this.workspace.info?.role === 'guest',
      isWorkspaceAdmin: workspaceAdmin,
      isTemplate: this.board?.isTemplate,
    });
  });
  readonly canEdit = $derived(this.effectivePermissions.canEdit);
  readonly canManageProperties = $derived(this.effectivePermissions.canManageProperties);
  readonly canManageVisibility = $derived(this.effectivePermissions.canManageVisibility);
  readonly canManageRoles = $derived(this.effectivePermissions.canManageRoles);
  readonly canDelete = $derived(this.effectivePermissions.canDelete);
  readonly canShare = $derived(this.effectivePermissions.canShare);
  readonly canComment = $derived(this.effectivePermissions.canComment);
  readonly canManageOthersComments = $derived(this.effectivePermissions.canManageOthersComments);
  readonly canReadCommentHistory = $derived(this.role !== '' || this.joinable);

  canDeleteComment(createdBy: string): boolean {
    return (
      this.effectivePermissions.canManageOthersComments ||
      (this.effectivePermissions.canComment && createdBy === this.workspace.me?.id)
    );
  }
  canEditComment(createdBy: string): boolean {
    return this.canDeleteComment(createdBy);
  }

  readonly #lastComments = lastCommentsOf(this);
  readonly #boardWrites = new BoardWrites(this);
  readonly #cardWrites = cardWritesOf(this);
  readonly #viewWrites = viewWritesOf(this);
  readonly #people = new MemberWrites(this);
  readonly #propertyMoves = propertyMovesOf(this);
  #previews: CardPreviews | undefined;
  #refreshing = 0;
  #stopWatching: (() => void) | undefined;
  visit = 0;
  evicted = false;
  #evictionGen = 0;
  joining = $state(false);
  readonly #joinOnWrite = new JoinOnWrite(this, (role) => this.#refreshPermissions(role));
  revalidating = $state(false);
  #closed = false;
  #parked = false;
  readonly #sync: BoardSyncInternals;

  constructor(workspace: Workspace, boardId: string) {
    this.workspace = workspace;
    this.boardId = boardId;
    this.#sync = {
      store: this,
      lastComments: this.#lastComments,
      boardWrites: this.#boardWrites,
      cardWrites: this.#cardWrites,
      viewWrites: this.#viewWrites,
      people: this.#people,
      isInactive: () => this.#isInactive(),
      nextRefresh: () => ++this.#refreshing,
      currentRefresh: () => this.#refreshing,
      evictionGen: () => this.#evictionGen,
      takeBoard: (board) => {
        if (!this.board || board.updatedAt >= this.board.updatedAt) this.board = board;
      },
      receiveCard: (card) => {
        receiveBoardCard(this, this.#lastComments, this.#cardWrites, card);
      },
      receiveView: (view) => {
        receiveView(this, this.#viewWrites, view);
      },
      refreshPermissions: () => this.#refreshPermissions(),
      refresh: () => this.#refresh(),
    };
  }

  /** Reads what the reader may do, as one of `role` (the role they have now, or the one a join gave). */
  async #refreshPermissions(role: MemberRole = this.role): Promise<void> {
    const visit = this.visit;
    try {
      const permissions = await this.#backend.myPermissions(this.boardId);
      if (!this.#isInactive() && this.visit === visit) {
        [this.permissions, this.permissionsRole] = [permissions, role];
      }
    } catch {
      // Handled on next load/refresh if access ended.
    }
  }

  get #backend(): Workspace['backend'] {
    return this.workspace.backend;
  }

  get previews(): CardPreviews {
    return (this.#previews ??= new CardPreviews(this.#backend, this.boardId));
  }

  async load(): Promise<void> {
    this.visit += 1;
    [this.#parked, this.revalidating, this.evicted] = [false, false, false];
    [this.status, this.missing] = ['loading', false];
    try {
      this.#listen();
      const [board, views, cards, members, permissions] = await readBoard(
        this.workspace,
        this.boardId,
      );
      if (this.#isInactive()) return;
      [this.board, this.views, this.cards, this.members, this.permissions] = [
        board,
        views,
        cards,
        members,
        permissions,
      ];
      this.permissionsRole = this.role;
      this.status = 'ready';
    } catch (error) {
      if (this.isClosed()) return;
      await handleBoardLoadFailure(this, error);
    }
  }

  markEvicted(): void {
    this.#evictionGen += 1;
    this.evicted = true;
  }

  #listen(): void {
    this.#stopWatching?.();
    this.#stopWatching = this.#backend.watchContainer(this.boardId, (change: Change) => {
      applyBoardChange(this.#sync, change);
    });
  }

  isClosed(): boolean {
    return this.#closed;
  }

  #isInactive(): boolean {
    return this.#closed || this.#parked;
  }

  #detach(): void {
    this.#refreshing += 1;
    this.#lastComments.cancel();
    this.#previews?.dispose();
    this.#stopWatching?.();
    this.#stopWatching = undefined;
    this.closeCard();
  }

  join(): Promise<boolean> {
    return joinOpenBoard(this);
  }

  /** Sends `write`, once the reader has joined where it is their first write on an open board. */
  writeJoined<T>(write: () => Promise<T>): Promise<T> {
    return this.#joinOnWrite.run(this.boardId, write);
  }

  park(): void {
    [this.#parked, this.revalidating] = [true, false];
    this.#detach();
  }

  async resume(): Promise<void> {
    if (this.#closed) return;
    [this.#parked, this.revalidating] = [false, true];
    this.visit += 1;
    const visit = this.visit;
    this.#listen();
    try {
      await this.#refresh();
    } finally {
      if (this.visit === visit) this.revalidating = false;
    }
  }

  close(): void {
    [this.#closed, this.#parked, this.revalidating] = [true, false, false];
    this.#detach();
  }

  view(viewId: string | undefined): BoardView | undefined {
    return this.views.find((view) => view.id === viewId);
  }
  card(cardId: string | undefined): CardSummary | undefined {
    return this.cards.find((card) => card.id === cardId);
  }
  filteredCardsOf(view: BoardView): Card[] {
    return filteredCardsOfBoardView(this, view);
  }
  narrowCards(filtered: readonly Card[], searchText: string): Card[] {
    return narrowBoardCards(this, filtered, searchText);
  }
  cardsOf(view: BoardView, searchText = ''): Card[] {
    return cardsOfBoardView(this, view, searchText);
  }
  groupingOf(view: BoardView): PropertyTemplate | undefined {
    return groupingPropertyOf(this.board?.cardProperties ?? [], view);
  }
  groupsOf(view: BoardView, cards: readonly Card[], noValueLabel: NoValueLabel): BoardGroups {
    return groupsOfView(this.board?.cardProperties ?? [], view, cards, noValueLabel);
  }

  #refresh(): Promise<void> {
    return refreshBoard(this.#sync);
  }
  deleting(blockId: string, cardId?: string): void {
    this.#lastComments.deleting(blockId, cardId);
  }
  receiveCard(card: CardSummary): void {
    receiveBoardCard(this, this.#lastComments, this.#cardWrites, card);
  }
  inOneStep<T>(gesture: (groupId: string) => Promise<T>, placedAtOnce = false): Promise<T> {
    return this.workspace.undo.group(gesture, placedAtOnce);
  }

  changeCard(
    cardId: string,
    change: (card: CardSummary) => Card,
    description = 'changeCard',
    groupId?: string,
  ): Promise<void> {
    return changeBoardCard(this, this.#cardWrites, cardId, change, description, groupId);
  }

  async setCardProperty(
    cardId: string,
    propertyId: string,
    value: PropertyValue | undefined,
    groupId?: string,
  ): Promise<void> {
    await this.changeCard(
      cardId,
      (card) => withCardProperty(card, propertyId, value),
      'changeProperty',
      groupId,
    );
  }

  async setCardTitle(cardId: string, title: string): Promise<void> {
    await this.changeCard(cardId, (card) => ({ ...card, title }), 'changeTitle');
  }

  async setCardIcon(cardId: string, icon: string): Promise<void> {
    await this.changeCard(
      cardId,
      (card) => ({ ...card, fields: { ...card.fields, icon } }),
      'changeIcon',
    );
  }

  createCard(draft: CardDraft, groupId?: string): Promise<CardSummary | undefined> {
    return createBoardCard(this, draft, groupId);
  }
  duplicateCard(cardId: string, asTemplate?: boolean): Promise<CardSummary | undefined> {
    return duplicateBoardCard(this, cardId, asTemplate);
  }
  deleteCards(cardIds: readonly string[]): Promise<void> {
    return deleteBoardCards(this, cardIds);
  }
  async dropCards(drop: CardDrop): Promise<void> {
    await dropCardsOn(this, drop);
  }

  changeView(
    viewId: string,
    change: (view: BoardView) => BoardView,
    description = 'changeView',
    groupId?: string,
  ): Promise<void> {
    return changeBoardView(this, this.#viewWrites, viewId, change, description, groupId);
  }

  async setViewFields(
    viewId: string,
    fields: Partial<BoardView['fields']>,
    description = 'changeView',
    groupId?: string,
  ): Promise<void> {
    await this.changeView(
      viewId,
      (view) => ({ ...view, fields: { ...view.fields, ...fields } }),
      description,
      groupId,
    );
  }

  writeView(viewId: string, forward: BlockPatch, backward: BlockPatch): Promise<void> {
    return this.#viewWrites.write(viewId, forward, backward);
  }
  keepViewInverse(viewId: string, backward: BlockPatch): () => Promise<void> | undefined {
    return this.#viewWrites.keep(viewId, backward);
  }
  rereadViews(waitForId?: string): Promise<void> {
    return rereadBoardViews(this, this.#viewWrites, () => this.#isInactive(), waitForId);
  }
  createView(draft: ViewDraft): Promise<BoardView | undefined> {
    return createBoardView(this, draft);
  }
  deleteView(viewId: string): Promise<void> {
    return deleteBoardView(this, viewId);
  }

  changeBoard(
    change: (board: Container) => Container,
    description = 'changeBoard',
    groupId?: string,
  ): Promise<void> {
    return changeBoardContainer(this, this.#boardWrites, change, description, groupId);
  }

  async moveProperty(propertyId: string, step: -1 | 1, among?: readonly string[]): Promise<void> {
    let moved = false;
    await performBoardAction(
      this,
      async () => {
        moved = await this.#propertyMoves.move(propertyId, step, among);
      },
      async () => {
        if (moved) await this.#propertyMoves.move(propertyId, step === 1 ? -1 : 1, among);
      },
      'moveProperty',
    );
  }

  async setOpen(open: boolean): Promise<void> {
    await this.changeBoard((board) => withVisibility(board, open), 'changeVisibility');
  }
  async setMinimumRole(role: ContainerRole): Promise<void> {
    await this.changeBoard((board) => ({ ...board, minimumRole: role }), 'changeMinimumRole');
  }
  addMember(userId: string, role: ContainerRole): Promise<void> {
    return this.#people.add(userId, role);
  }
  setMemberRole(userId: string, role: ContainerRole): Promise<void> {
    return this.#people.setRole(userId, role);
  }
  removeMember(userId: string): Promise<void> {
    return this.#people.remove(userId);
  }

  async showCard(cardId: string): Promise<CardStore> {
    if (this.openCard?.cardId === cardId) return this.openCard;
    this.openCard?.close();
    const store = new CardStore(this, cardId, () => void this.#refresh());
    this.openCard = store;
    await store.load();
    return store;
  }

  closeCard(): void {
    this.openCard?.close();
    this.openCard = undefined;
  }
}
