<!--
  A view of a board as a table: a row a card, a column a property.

  The heading row stays at the top, the summary row at the bottom and the
  title column at the left edge while the rest scrolls under them, at every
  width: on a phone it is still a table.
  All of that is CSS. Nothing here asks how wide the page is, so a window
  that changes size does not swap a part of the table for another, and what
  the reader was doing in it (the focus, a text being typed, the place it is
  scrolled to) stays.

  The parts: HeaderCell (the menu and the width of a column), Row (a card),
  GroupHeader, CalculationCell. What they need of the table is in the
  controller, table-controller.svelte.ts; this file lays them out and reads the keys.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import { isComposing, primaryModifier } from '$lib/keys.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalculationOption } from '$lib/logic/properties.ts';
  import { TITLE_COLUMN_ID } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';

  import type { ViewProps } from '../props.ts';

  import CalculationCell from './CalculationCell.svelte';
  import { calculationOf, calculationOptions, calculationResult } from './calculation-names.ts';
  import { gridTemplate, sortDirection } from './columns.ts';
  import { isMoveKey } from './grid-focus.ts';
  import GroupHeader from './GroupHeader.svelte';
  import HeaderCell from './HeaderCell.svelte';
  import { CELL_FOCUS, DROP_LINE_ROW, ROW, ROW_HEIGHT } from './host.ts';
  import Row from './Row.svelte';
  import { TableController, type RowList } from './table-controller.svelte.ts';

  let { board, view, cards, cardHref, onOpenCard }: ViewProps = $props();

  const uid = $props.id();
  const table = new TableController({
    board: () => board,
    view: () => view,
    cards: () => cards,
    cardHref: (cardId) => cardHref(cardId),
    openCard: (cardId) => {
      onOpenCard(cardId);
    },
  });
  const { editing, grid, keeper, rows, columnSort, host } = table;

  const rowDrag = $derived(rows.active);
  const columnDrag = $derived(columnSort.active);
  const notice = $derived(rows.notice || columnSort.notice);
  const draggedCard = $derived(rowDrag ? board.card(rowDrag.itemId) : undefined);
  const draggedColumn = $derived(
    columnDrag ? table.columns.find((column) => column.id === columnDrag.itemId) : undefined,
  );
  const titleName = $derived(m['TableComponent.name']());

  /** While the keyboard carries a column, the headings alone stand in the order it would give. */
  const headerTemplate = $derived(
    table.headerColumns === table.columns
      ? undefined
      : gridTemplate(table.headerColumns.map((column) => column.width)),
  );

  // Another view starts at its top left corner, with nothing being edited.
  $effect(() => {
    // Read the identifier: that is what makes this run again for another view.
    const shown = view.id;
    untrack(() => {
      if (shown === '') return;
      keeper.reset();
      editing.stop();
      table.resetCollapsed();
    });
  });
  // After the rows or the columns were drawn anew, one cell is still the Tab stop.
  $effect(() => {
    grid.ensure(table.shownLists, table.headerColumns);
  });
  $effect(() => editing.watch());

  /** Enter on a cell does what a press on it does. */
  function activate(cell: HTMLElement): void {
    (cell.querySelector<HTMLElement>('[data-activate]') ?? cell).click();
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.defaultPrevented || isComposing(event)) return;
    const target = event.target;
    if (!(target instanceof HTMLElement)) return;
    if (editing.current) {
      // The keys are the editor's. Escape that it left alone ends the editing.
      if (event.key === 'Escape') {
        event.preventDefault();
        editing.stop(true);
      }
      return;
    }
    const cell = grid.cellOf(target);
    if (!cell) return;
    if (target !== cell) {
      // A control inside the cell has the focus: Escape goes back to the cell.
      if (event.key === 'Escape') {
        event.preventDefault();
        grid.focus(cell);
      }
      return;
    }
    if (event.altKey || event.shiftKey) return;
    const primary = primaryModifier(event);
    if ((event.ctrlKey || event.metaKey) && !primary) return;
    if (isMoveKey(event.key)) {
      event.preventDefault();
      const next = grid.next(cell, event.key, primary);
      if (next && next !== cell) grid.focus(next);
    } else if (primary) {
      return;
    } else if (event.key === 'Enter' || event.key === 'F2') {
      event.preventDefault();
      activate(cell);
    } else if (event.key === ' ' && cell.dataset.kind === 'checkbox') {
      event.preventDefault();
      activate(cell);
    }
  }

  /** The row dragged by a pointer is drawn at the pointer, but never left of the table. */
  function overlayX(x: number): number {
    return Math.max(x, table.scroller?.getBoundingClientRect().left ?? x);
  }
</script>

{#snippet addRow(list: RowList | undefined)}
  {@const dropHere = list !== undefined && table.rowMarker?.endOf === list.id}
  <div
    role="row"
    data-nav-row
    data-row="add:{list?.id ?? ''}"
    data-drop-before={dropHere ? '' : undefined}
    class={['relative flex border-t', dropHere && DROP_LINE_ROW]}
  >
    <div
      role="gridcell"
      tabindex="-1"
      data-cell
      data-kind="add"
      data-col="*"
      class={['sticky start-0 flex items-center px-1', ROW_HEIGHT, CELL_FOCUS]}
    >
      <Button
        variant="ghost"
        size="sm"
        class="text-muted-foreground roomy:min-h-touch"
        data-activate
        data-inner
        tabindex={-1}
        onclick={() => void table.addCard(list)}
      >
        {m['TableComponent.plus-new']()}
      </Button>
    </div>
  </div>
{/snippet}

<div class="relative h-full" data-testid="table-view">
  <p id="{uid}-help" class="sr-only">{m.table_keys_help()}</p>
  <!-- svelte-ignore a11y_interactive_supports_focus (the cells of a grid take the focus, not the grid) -->
  <div
    bind:this={table.scroller}
    role="grid"
    aria-label={view.title || m['Sidebar.untitled-view']()}
    aria-describedby="{uid}-help"
    aria-readonly={table.canEdit ? undefined : 'true'}
    data-testid="table"
    data-scrolled={table.sideways ? '' : undefined}
    data-drag-scroll={columnDrag ? 'x' : 'y'}
    data-dragging={(rowDrag ?? columnDrag)?.mode}
    class={[
      'group/table h-full scroll-py-9 overflow-auto overscroll-contain [--table-title:10rem] @columns/pane:[--table-title:14rem] @header/pane:[--table-title:22rem] webkit:[overflow-anchor:none] roomy:scroll-py-11',
      (rowDrag ?? columnDrag)?.mode === 'pointer' && 'cursor-grabbing',
    ]}
    style:--table-columns={table.template}
    style:--table-title={table.liveTitleWidth === undefined
      ? undefined
      : `${table.liveTitleWidth}px`}
    onkeydown={onKeyDown}
    {@attach keeper.attach}
    {@attach grid.attach}
    {@attach rows.attach}
    {@attach columnSort.attach}
  >
    <div class="w-max min-w-full">
      <!-- The rows draw the line above them; the first one's lies under the line of the headings. -->
      <div role="rowgroup" data-sticky="top" class="sticky top-0 z-20 -mb-px">
        <div
          role="row"
          data-nav-row
          data-row="header"
          data-drag-list="columns"
          data-drag-scope="column"
          class={ROW}
          style:grid-template-columns={headerTemplate}
        >
          <HeaderCell
            id={TITLE_COLUMN_ID}
            name={titleName}
            pinned
            sorted={sortDirection(view.fields.sort, TITLE_COLUMN_ID)}
            canEdit={table.canEdit}
            actions={table.headerActions(TITLE_COLUMN_ID)}
            width={table.liveTitleWidth}
            {...table.resize.handlers(TITLE_COLUMN_ID)}
          />
          {#each table.headerColumns as column (column.id)}
            <HeaderCell
              id={column.id}
              name={column.template.name}
              sorted={sortDirection(view.fields.sort, column.id)}
              canEdit={table.canEdit}
              actions={table.headerActions(column.id)}
              dragging={columnDrag?.mode === 'pointer' && columnDrag.itemId === column.id}
              carried={columnDrag?.mode === 'keyboard' && columnDrag.itemId === column.id}
              drop={table.columnMarker?.id === column.id ? table.columnMarker.side : undefined}
              width={table.resize.live?.id === column.id ? table.resize.live.width : column.width}
              {...table.resize.handlers(column.id)}
            />
          {/each}
          <div role="presentation" class="border-b bg-background"></div>
        </div>
      </div>

      {#each table.shownLists as list (list.id)}
        <div role="rowgroup" data-drag-list={list.id} data-list={list.id}>
          {#if list.group}
            <GroupHeader
              rowId="group:{list.group.option.id}"
              name={table.groupName(list.group)}
              color={table.groupColor(list.group)}
              count={list.group.cards.length}
              collapsed={list.collapsed}
              canAdd={table.canAddTo(list)}
              dropInto={list.collapsed && table.rowMarker?.endOf === list.id}
              onToggle={() => {
                table.toggleGroup(list);
              }}
              onAdd={() => void table.addCard(list)}
            />
          {/if}
          {#each list.rows as entry (entry.card.id)}
            {@const card = entry.card}
            {@const editingColumn =
              editing.current?.rowId === card.id ? editing.current.columnId : undefined}
            {@const dragging = rowDrag?.mode === 'pointer' && rowDrag.itemIds.includes(card.id)}
            {@const carried = rowDrag?.mode === 'keyboard' && rowDrag.itemId === card.id}
            {@const dropBefore = table.rowMarker?.before === card.id}
            <Row
              {board}
              {card}
              columns={table.columns}
              {host}
              {editingColumn}
              draggable={table.canDragRows}
              {dragging}
              {carried}
              {dropBefore}
              level={entry.level}
              hasChildren={entry.hasChildren}
              collapsed={entry.collapsed}
              showIndent={table.hasHierarchy}
            />
          {/each}
          {#if !list.collapsed && table.canAddTo(list)}
            {@render addRow(list)}
          {/if}
        </div>
      {/each}
      {#if table.canAddLoose}
        <div role="rowgroup">{@render addRow(undefined)}</div>
      {/if}

      <div role="rowgroup" data-sticky="bottom" class="sticky bottom-0 z-20">
        <div role="row" data-nav-row data-row="calculations" class={['group/calculations', ROW]}>
          {#each [table.titleColumn, ...table.columns] as column (column.id)}
            {@const option = calculationOf(view.fields.columnCalculations, column.template)}
            <CalculationCell
              id={column.id}
              name={column.template.name}
              pinned={column.id === TITLE_COLUMN_ID}
              {option}
              result={calculationResult(option, cards, column.template, table.context)}
              options={calculationOptions(column.template)}
              canEdit={table.canEdit}
              onChoose={(chosen: CalculationOption) => {
                table.chooseCalculation(column.id, chosen);
              }}
            />
          {/each}
          <div role="presentation" class="border-t bg-background"></div>
        </div>
      </div>
    </div>
  </div>

  <div class="sr-only" aria-live="assertive" aria-atomic="true" data-testid="table-row-announcer">
    {rows.announcement}
  </div>
  <div
    class="sr-only"
    aria-live="assertive"
    aria-atomic="true"
    data-testid="table-column-announcer"
  >
    {columnSort.announcement}
  </div>
  <div class="sr-only" role="status" data-testid="table-status">{table.said}</div>
</div>

{#if notice}
  <!-- What letting go would do when that is not plain to see. -->
  <div
    data-testid="table-notice"
    class="pointer-events-none fixed inset-x-0 top-[calc(var(--safe-top)+0.5rem)] z-60 flex justify-center px-3"
  >
    <span
      class="rounded-full bg-foreground px-3 py-1 text-center text-sm font-medium text-background shadow-lg"
    >
      {notice}
    </span>
  </div>
{/if}

{#if rowDrag?.mode === 'pointer' && rows.overlay}
  <!-- Under the pointer, above everything, and never in the way of what is beneath. -->
  <div
    data-drag-overlay={rowDrag.itemId}
    aria-hidden="true"
    class={[
      'pointer-events-none fixed top-0 left-0 z-50 flex max-w-64 items-center gap-1 rounded-md border bg-background px-3 shadow-lg ring-2 ring-ring will-change-transform',
      ROW_HEIGHT,
      rowDrag.outside && 'opacity-50',
    ]}
    style:translate="{overlayX(rows.overlay.x)}px {rows.overlay.y}px"
  >
    {#if draggedCard?.fields.icon}<span class="shrink-0">{draggedCard.fields.icon}</span>{/if}
    <span class="truncate">{draggedCard?.title || m['KanbanCard.untitled']()}</span>
  </div>
{/if}

{#if columnDrag?.mode === 'pointer' && columnSort.overlay && draggedColumn}
  <div
    data-drag-overlay={columnDrag.itemId}
    aria-hidden="true"
    class={[
      'pointer-events-none fixed top-0 left-0 z-50 flex items-center rounded-md border bg-background px-3 font-medium text-muted-foreground shadow-lg ring-2 ring-ring will-change-transform',
      ROW_HEIGHT,
      columnDrag.outside && 'opacity-50',
    ]}
    style:width="{columnDrag.width}px"
    style:translate="{columnSort.overlay.x}px {columnSort.overlay.y}px"
  >
    <span class="truncate">{draggedColumn.template.name}</span>
  </div>
{/if}
