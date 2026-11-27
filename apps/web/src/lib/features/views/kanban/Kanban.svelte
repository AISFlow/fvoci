<!--
  The board as columns of cards.

  The columns are the groups the board store gives for the view
  (`board.groupsOf`): the options of the property the view is grouped by, or
  the people of a person property. Cards are dragged within and between the
  columns and onto the hidden ones, and the columns themselves by their
  headings: two sortables on one root, told apart by their scope (lib/drag).
  Nothing is moved until a drop, and a drop is one call of the store.

  The board follows the width of the pane it is in, not of the window. Where
  two columns fit side by side they have a fixed width and the board scrolls
  freely. In a narrower pane it shows one column at a time: a column is as
  wide as the pane less a sliver of its neighbours, the board comes to rest
  on a column, and a row of tabs names the columns, says which is in view and
  jumps to one. The two are the same elements with other classes, so a pane
  that changes width keeps what was open, focused and scrolled, and comes
  back to the column that was looked at.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import { display } from '$lib/display.svelte.ts';
  import { readerPointer } from '$lib/components/drag/reader-pointer.ts';
  import { Sortable, type SortableMove } from '$lib/drag/sortable.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { moveItems, type ListOrder } from '$lib/logic/drop-target.ts';
  import type { BoardGroup } from '$lib/logic/group.ts';
  import { whileHere, type CardDrop } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import type { ViewProps } from '../props.ts';

  import { addGroup, faceContextOf, propertiesForColumn } from './board-data.ts';
  import ColumnTabs from './ColumnTabs.svelte';
  import DragLayer from './DragLayer.svelte';
  import { CARD_MESSAGES, COLUMN_MESSAGES } from './drag-messages.ts';
  import HiddenColumns from './HiddenColumns.svelte';
  import {
    cardLists,
    columnIdOf,
    drawnColumns,
    hiddenIdOf,
    listsAfterDrop,
    optionIdOf,
    ROW,
    sortedIndex,
    withColumnHidden,
    withColumnShown,
    withColumnStepped,
    type Activation,
    type ColumnLists,
    type MoveTarget,
  } from './kanban.ts';
  import KanbanColumn from './KanbanColumn.svelte';
  import { Paging } from './paging.svelte.ts';
  import { Selection } from './selection.svelte.ts';

  let { board, view, cards, cardHref, onOpenCard }: ViewProps = $props();

  const uid = $props.id();
  /** No cards on their way: always this one array, so that nothing takes it for a change. */
  const NONE: readonly string[] = [];

  const groupBy = $derived(board.groupingOf(view));
  const noValueName = $derived(m['BoardComponent.no-property']({ property: groupBy?.name ?? '' }));
  const groups = $derived(
    board.groupsOf(view, cards, (property) => m['BoardComponent.no-property']({ property })),
  );
  const editable = $derived(board.canEdit);
  /** The columns are options of a select property: they have names, colours and an order of their own. */
  const isOption = $derived(groupBy?.type === 'select');
  /** The value of a column can be set on a card. Who made a card, or changed it last, cannot. */
  const settable = $derived(
    groupBy !== undefined && groupBy.type !== 'createdBy' && groupBy.type !== 'updatedBy',
  );
  /** The view sorts its cards: a drop chooses the column, and the sort the place. */
  const sorted = $derived(view.fields.sort.length > 0);
  const face = $derived(faceContextOf(board, view));

  const cardOrder = $derived(cards.map((card) => card.id));
  const cardById = $derived(new Map(cards.map((card) => [card.id, card])));

  /**
   * The columns as a drop left them, while the store catches up. The store
   * changes the place of a dropped card first and its column once that was
   * answered; without this the card would show back where it came from for
   * that moment, and a card the keyboard dropped would lose the focus.
   */
  let settling = $state.raw<readonly ListOrder[] | undefined>(undefined);
  const visible = $derived.by((): BoardGroup[] => {
    const after = settling;
    if (!after) return groups.visible;
    return groups.visible.map((group) => {
      const list = after.find((candidate) => candidate.id === columnIdOf(group.option.id));
      if (!list) return group;
      return { ...group, cards: list.items.flatMap((id) => cardById.get(id) ?? []) };
    });
  });

  const shownIds = $derived(visible.map((group) => group.option.id));
  const lists = $derived(cardLists(visible));
  const row = $derived<ListOrder[]>([{ id: ROW, items: shownIds.map(columnIdOf) }]);

  /** The name a column shows: of its option, or of the person it stands for. */
  function nameOf(group: BoardGroup): string {
    if (isOption || group.option.id === '') return group.option.label;
    return board.workspace.userName(group.option.id) ?? m['centerPanel.unknown-user']();
  }
  const names = $derived(
    new Map([...visible, ...groups.hidden].map((group) => [group.option.id, nameOf(group)])),
  );
  const nameOfOption = (optionId: string): string => names.get(optionId) ?? optionId;
  const moveTargets = $derived(
    visible.map((group): MoveTarget => ({
      optionId: group.option.id,
      name: nameOf(group),
      current: false,
    })),
  );

  // ------------------------------------------------------------ selection

  const selection = new Selection(() => lists.flatMap((list) => list.items));
  const selectedIds = $derived(selection.ids);

  function activate(cardId: string, how: Activation): void {
    if (!editable || (how === 'open' && !selection.selecting)) onOpenCard(cardId);
    else if (how === 'range') selection.extendTo(cardId);
    else selection.toggle(cardId);
  }

  // -------------------------------------------------------------- changes

  /** Cards go to a column, at a place among the cards it shows or where the view's order puts them. */
  async function dropCards(
    cardIds: readonly string[],
    optionId: string,
    index: number | undefined,
    changeColumn: boolean,
  ): Promise<void> {
    if (!groupBy) return;
    const target = lists.find((list) => list.id === columnIdOf(optionId))?.items ?? [];
    const drop: CardDrop = { viewId: view.id, cardIds };
    if (changeColumn) drop.optionId = optionId;
    if (index !== undefined) {
      // A place among the cards the column shows: the store counts the hidden ones in.
      drop.index = index;
      drop.shown = target;
    }
    if (cardIds.some((id) => selectedIds.includes(id))) selection.clear();
    const place = index ?? sortedIndex(cardOrder, target, cardIds[0] ?? '', cardIds);
    settling = listsAfterDrop(lists, cardIds, columnIdOf(optionId), place);
    try {
      await board.dropCards(drop);
    } finally {
      settling = undefined;
    }
  }

  function onCardDrop({ itemIds, from, to }: SortableMove): void {
    const optionId = optionIdOf(to.kind === 'zone' ? to.zoneId : to.listId);
    const stays = to.kind !== 'zone' && to.listId === from.listId;
    // A column that is no value to set (who made the card) cannot be changed by a drop.
    if (!settable && !stays) return;
    void dropCards(itemIds, optionId, to.kind === 'index' ? to.index : undefined, settable);
  }

  /** "Move to another column" in the menu of a card: to the end of that column. */
  function moveCards(cardIds: readonly string[], optionId: string): void {
    void dropCards(cardIds, optionId, sorted ? undefined : Number.POSITIVE_INFINITY, true);
  }

  async function addCard(optionId: string): Promise<void> {
    if (!board.board || !groupBy) return;
    const properties = propertiesForColumn(board.board, view, groupBy.id, optionId);
    const card = await whileHere(board, () => board.createCard({ properties }));
    if (card) onOpenCard(card.id);
  }

  function setColumns(next: ColumnLists): void {
    // The columns of people are not listed by the view: only which are hidden.
    void board.setViewFields(
      view.id,
      isOption ? next : { hiddenGroups: next.hiddenGroups },
      'changeColumns',
    );
  }

  function hideColumn(optionId: string): void {
    setColumns(withColumnHidden(shownIds, view.fields.hiddenGroups, optionId));
  }

  function showColumn(optionId: string): void {
    setColumns(withColumnShown(shownIds, view.fields.hiddenGroups, optionId));
  }

  function stepColumn(optionId: string, step: -1 | 1): void {
    void board.setViewFields(
      view.id,
      { visibleGroups: withColumnStepped(shownIds, optionId, step) },
      'moveColumn',
    );
  }

  /** The column whose heading is a field for its name. */
  let renamingId = $state<string | undefined>(undefined);

  async function addNewGroup(): Promise<void> {
    if (!groupBy) return;
    renamingId = await addGroup(board, view, groupBy.id, m.kanban_new_group(), shownIds);
  }

  // ------------------------------------------------------------- dragging

  const cardSort = new Sortable({
    order: () => lists,
    onDrop: onCardDrop,
    messages: CARD_MESSAGES,
    itemLabel: (id) => cardById.get(id)?.title || m['KanbanCard.untitled'](),
    listLabel: (id) => nameOfOption(optionIdOf(id)),
    zoneLabel: (id) => nameOfOption(optionIdOf(id)),
    placement: () => (sorted ? 'list' : 'free'),
    sortedIndex: (itemId, listId, carried) =>
      sortedIndex(
        cardOrder,
        lists.find((list) => list.id === listId)?.items ?? [],
        itemId,
        carried,
      ),
    selection: () => selectedIds,
    zones: () => (settable ? groups.hidden.map((group) => hiddenIdOf(group.option.id)) : []),
    // The device buzzes when a resting finger has picked something up, unless the reader turned that off.
    pointer: readerPointer,
  });

  const columnSort = new Sortable({
    scope: 'column',
    axis: 'x',
    order: () => row,
    onDrop: ({ itemIds, to }) => {
      if (to.kind !== 'index') return;
      const ids = moveItems(row, itemIds, to)[0]?.items ?? [];
      void board.setViewFields(view.id, { visibleGroups: ids.map(optionIdOf) }, 'moveColumn');
    },
    messages: COLUMN_MESSAGES,
    itemLabel: (id) => nameOfOption(optionIdOf(id)),
    // The device buzzes when a resting finger has picked something up, unless the reader turned that off.
    pointer: readerPointer,
  });

  /**
   * Attached only where the reader may change things. The settings of the
   * pointer are read when a sortable is attached; reading the reader's
   * choice here attaches both anew when it changes.
   */
  const attachDrag = (root: HTMLElement): (() => void) => {
    root.dataset.haptics = String(display.resolved.haptics);
    const detachCards = editable && groupBy ? cardSort.attach(root) : undefined;
    const detachColumns = editable && isOption ? columnSort.attach(root) : undefined;
    return () => {
      detachCards?.();
      detachColumns?.();
    };
  };

  const cardDrag = $derived(cardSort.active);
  const columnDrag = $derived(columnSort.active);
  const dragging = $derived((cardDrag ?? columnDrag) !== undefined);
  const draggedCard = $derived(cardDrag ? cardById.get(cardDrag.itemId) : undefined);
  const draggedColumn = $derived(
    columnDrag
      ? visible.find((group) => columnIdOf(group.option.id) === columnDrag.itemId)
      : undefined,
  );
  const overHidden = $derived(
    cardDrag?.to?.kind === 'zone' ? optionIdOf(cardDrag.to.zoneId) : undefined,
  );

  const columnEntries = $derived(
    drawnColumns(
      visible,
      columnDrag?.gap && {
        mode: columnDrag.mode,
        itemId: columnDrag.itemId,
        index: columnDrag.gap.index,
      },
    ),
  );

  // ------------------------------------------------- one column at a time

  const hasMore = $derived(groups.hidden.length > 0 || (editable && isOption));
  /** In a pane that fits one column the board rests on a column, and the tabs show. */
  const paging = new Paging(
    () => pane.measured && !pane.columns,
    () => visible.length + (hasMore ? 1 : 0),
  );
  const tabs = $derived([
    ...visible.map((group, at) => ({
      key: group.option.id,
      name: nameOf(group),
      count: group.cards.length,
      panelId: `${uid}-panel-${at}`,
    })),
    ...(hasMore
      ? [
          {
            key: ' more',
            name: groups.hidden.length > 0 ? m['BoardComponent.hidden-columns']() : m.shared_more(),
            count: groups.hidden.length > 0 ? groups.hidden.length : undefined,
            panelId: `${uid}-more`,
          },
        ]
      : []),
  ]);

  // The page keeps this component when the reader goes to another board
  // view: that one starts at its first column, with nothing selected.
  let shownViewId = untrack(() => view.id);
  $effect(() => {
    if (view.id === shownViewId) return;
    shownViewId = view.id;
    untrack(() => {
      selection.clear();
      renamingId = undefined;
      paging.reset();
    });
  });

  /** Escape lets the selection go, unless something else (a drag, a menu) took the key. */
  const attachKeys = (root: HTMLElement): (() => void) => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape' || event.defaultPrevented || selectedIds.length === 0) return;
      selection.clear();
    };
    root.addEventListener('keydown', onKeyDown);
    return () => {
      root.removeEventListener('keydown', onKeyDown);
    };
  };
</script>

{#if !groupBy}
  <div class="grid h-full place-items-center p-6 text-center" data-testid="kanban">
    <p class="max-w-sm text-muted-foreground">{m.kanban_no_grouping()}</p>
  </div>
{:else}
  <div class="flex h-full min-h-0 flex-col" data-testid="kanban" {@attach attachKeys}>
    <ColumnTabs
      {tabs}
      current={paging.current}
      visible={pane.measured && !pane.columns}
      onShow={(at: number) => {
        paging.show(at);
      }}
    />

    {#if selectedIds.length > 0}
      <div
        data-testid="selection-bar"
        class="flex shrink-0 items-center gap-2 border-b px-4 py-1 text-sm"
      >
        <span role="status">{m.kanban_selected_count({ count: selectedIds.length })}</span>
        <Button
          variant="ghost"
          size="sm"
          class="roomy:min-h-touch"
          onclick={() => {
            selection.clear();
          }}
        >
          {m.kanban_clear_selection()}
        </Button>
      </div>
    {/if}

    <div
      bind:this={paging.scroller}
      data-testid="kanban-board"
      data-drag-list={ROW}
      data-drag-scope="column"
      data-drag-scroll="x"
      data-dragging={cardDrag?.mode ?? columnDrag?.mode}
      data-drop-index={cardDrag?.gap?.index}
      data-settling={settling ? '' : undefined}
      data-outside={(cardDrag ?? columnDrag)?.outside ? '' : undefined}
      class={[
        'relative flex min-h-0 flex-1 items-start gap-3 overflow-x-auto overflow-y-hidden px-[max(1.75rem,calc((100%-24rem)/2))] pt-3 pb-[calc(0.75rem+var(--safe-bottom))] @columns/pane:px-3 @header/pane:px-4',
        // A board that comes to rest on a column must not do so while the edge of the screen scrolls it under a drag.
        dragging ? 'snap-none' : 'snap-x snap-mandatory @columns/pane:snap-none',
        (cardDrag ?? columnDrag)?.mode === 'pointer' && 'cursor-grabbing',
      ]}
      onscroll={paging.onScroll}
      {@attach attachDrag}
    >
      {#each columnEntries as entry, drawnAt (entry.kind === 'gap' ? ' gap' : entry.group.option.id)}
        {#if entry.kind === 'column'}
          {@const id = columnIdOf(entry.group.option.id)}
          <KanbanColumn
            {board}
            viewId={view.id}
            {groupBy}
            group={entry.group}
            name={nameOf(entry.group)}
            panelId="{uid}-panel-{entry.at}"
            {drawnAt}
            {face}
            {editable}
            {settable}
            {isOption}
            canStep={editable && isOption
              ? { left: entry.at > 0, right: entry.at < visible.length - 1 }
              : undefined}
            {noValueName}
            columns={moveTargets}
            {cardHref}
            draggedIds={cardDrag?.itemIds ?? NONE}
            gapAt={cardDrag?.gap?.listId === id ? cardDrag.gap.index : -1}
            gapHeight={cardDrag?.height ?? 0}
            carried={cardDrag?.mode === 'keyboard' ? draggedCard : undefined}
            carriedCount={cardDrag?.itemIds.length ?? 1}
            {selectedIds}
            hidden={columnDrag?.mode === 'pointer' && columnDrag.itemId === id}
            lifted={columnDrag?.mode === 'keyboard' && columnDrag.itemId === id}
            renaming={renamingId === entry.group.option.id}
            cardHelpId="{uid}-card-help"
            columnHelpId="{uid}-column-help"
            onActivate={activate}
            onSelect={(cardId: string) => {
              selection.begin(cardId);
            }}
            onMove={moveCards}
            onAddCard={(optionId: string) => void addCard(optionId)}
            onHide={hideColumn}
            onStep={stepColumn}
            onRenaming={(optionId: string | undefined) => {
              renamingId = optionId;
            }}
          />
        {:else}
          <div
            data-column-gap
            aria-hidden="true"
            class="shrink-0 rounded-lg border-2 border-dashed border-input-border"
            style:width="{columnDrag?.width ?? 0}px"
            style:height="{columnDrag?.height ?? 0}px"
          ></div>
        {/if}
      {/each}

      {#if hasMore}
        <HiddenColumns
          panelId="{uid}-more"
          hidden={groups.hidden}
          {nameOf}
          {isOption}
          {editable}
          canAdd={editable && isOption}
          {face}
          overOptionId={overHidden}
          carried={cardDrag?.mode === 'keyboard' ? draggedCard : undefined}
          carriedCount={cardDrag?.itemIds.length ?? 1}
          cardHelpId="{uid}-card-help"
          onShow={showColumn}
          onAddGroup={() => void addNewGroup()}
        />
      {/if}
    </div>

    <p id="{uid}-card-help" class="sr-only">{m.kanban_drag_help()}</p>
    <p id="{uid}-column-help" class="sr-only">{m.kanban_column_help()}</p>
    <div data-testid="announcer" class="sr-only" aria-live="assertive" aria-atomic="true">
      {cardSort.announcement}
    </div>
    <div data-testid="column-announcer" class="sr-only" aria-live="assertive" aria-atomic="true">
      {columnSort.announcement}
    </div>
  </div>

  <DragLayer
    notice={cardSort.notice || columnSort.notice}
    {face}
    {cardDrag}
    cardAt={cardSort.overlay}
    card={draggedCard}
    {columnDrag}
    columnAt={columnSort.overlay}
    column={draggedColumn}
    columnName={draggedColumn ? nameOf(draggedColumn) : ''}
    {isOption}
  />
{/if}
