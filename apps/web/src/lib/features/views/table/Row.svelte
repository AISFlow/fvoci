<!--
  One row of the table: a card. Its title, pinned at the left edge, and the
  value of each property the view shows. The row carries `data-card`, as a
  card does on every view: the page and its tests find a card by it.

  Every value is drawn for reading (`PropertyValue` in its `display` mode).
  Only the cell the table says is being edited (`editingColumn`) gets the
  editor of its property, opened at once; see cell-editing.svelte.ts.

  The title cell is also what the row is dragged by, where the view has no
  sort: a mouse that travels with the button down, a finger that rests, or
  Space on the cell. A press that does neither is a click and edits the
  title in place.
-->
<script lang="ts">
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import { tick } from 'svelte';
  import CardPresence from '../CardPresence.svelte';
  import type { BoardStore } from '$lib/state/index.ts';

  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import { PropertyValue } from '$lib/components/properties/index.ts';
  import TextField from '$lib/components/properties/parts/TextField.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    TITLE_COLUMN_ID,
    type Card,
    type PropertyValue as StoredValue,
  } from '$lib/logic/wire.ts';

  import type { TableColumn } from './columns.ts';
  import {
    CELL_FOCUS,
    DROP_LINE_ROW,
    MORE_BUTTON,
    PINNED,
    ROW,
    ROW_HEIGHT,
    ROW_SKIPPED,
    type RowHost,
  } from './host.ts';

  interface Props {
    card: Card;
    board: BoardStore;
    columns: readonly TableColumn[];
    host: RowHost;
    /** The column of this row that is being edited, if one is. */
    editingColumn: string | undefined;
    /** The row can be moved: the view has no sort and the reader may edit. */
    draggable: boolean;
    /** A pointer is dragging this row. */
    dragging: boolean;
    /** The keyboard carries this row: its title cell is the stand-in that holds the focus. */
    carried: boolean;
    /** The dragged row would land just above this one. */
    dropBefore: boolean;
    /** Nesting level in the visible table (0, 1, or 2). */
    level?: number;
    /** Whether this card has visible sub-cards below it. */
    hasChildren?: boolean;
    /** Whether its sub-cards are currently collapsed. */
    collapsed?: boolean;
    /** Whether the table displays hierarchy indentation / toggles. */
    showIndent?: boolean;
  }

  let {
    card,
    board,
    columns,
    host,
    editingColumn,
    draggable,
    dragging,
    carried,
    dropBefore,
    level = 0,
    hasChildren = false,
    collapsed = false,
    showIndent = false,
  }: Props = $props();

  /**
   * The menu is made when it is first asked for: a table of hundreds of rows
   * would otherwise carry hundreds of menus nobody opens.
   */
  let menuMade = $state(false);
  let menuOpen = $state(false);

  let row = $state<HTMLElement | null>(null);
  /**
   * Makes the menu and opens it. The plain button is replaced by the menu's
   * own, and the focus is put on that one first: a menu gives the focus back
   * to where it was when it opened.
   */
  async function openMenu(): Promise<void> {
    menuMade = true;
    await tick();
    row?.querySelector<HTMLElement>('[data-more]')?.focus({ preventScroll: true });
    menuOpen = true;
  }

  const title = $derived(card.title || m['KanbanCard.untitled']());
  const reader = $derived(host.reader);

  /** A press on the title cell itself, not on one of the controls in it. */
  function onTitleClick(event: MouseEvent): void {
    if (event.target instanceof Element && event.target.closest('a, button, input')) return;
    if (host.canEdit) host.editTitle(card.id);
  }
</script>

<!-- The three dots of "more". Drawn here, not by an icon component: there is one per row. -->
{#snippet dots()}
  <svg class="size-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
    <circle cx="3" cy="8" r="1.25" /><circle cx="8" cy="8" r="1.25" />
    <circle cx="13" cy="8" r="1.25" />
  </svg>
{/snippet}

<div
  bind:this={row}
  role="row"
  data-nav-row
  data-row={card.id}
  data-card={card.id}
  data-level={showIndent ? level : undefined}
  data-drag-item={draggable ? card.id : undefined}
  data-dragging={dragging ? '' : undefined}
  data-drop-before={dropBefore ? '' : undefined}
  class={[
    'group/row relative border-t',
    ROW,
    ROW_SKIPPED,
    dragging && 'opacity-50',
    dropBefore && DROP_LINE_ROW,
    carried && 'z-10 ring-2 ring-ring ring-inset',
  ]}
>
  <!-- svelte-ignore a11y_click_events_have_key_events (the table's key handler turns Enter on a cell into this click) -->
  <div
    role="rowheader"
    tabindex="-1"
    data-cell
    data-kind="title"
    data-col={TITLE_COLUMN_ID}
    data-pinned
    data-drag-handle={draggable ? '' : undefined}
    data-drag-carried={carried ? card.id : undefined}
    class={[
      // The lines between rows belong to the rows. This cell is drawn on a layer of its own, which
      // can sit a fraction of a pixel off: its fill stops short of its top and bottom edge, so that
      // the lines show whichever way it is off.
      'flex min-w-0 items-center border-y border-e border-y-transparent bg-clip-padding ps-1 pe-1 group-hover/row:bg-muted',
      ROW_HEIGHT,
      PINNED,
      CELL_FOCUS,
      host.canEdit && 'cursor-text',
      // A resting finger picks the row up; it must not select the text or call the browser's own menu instead.
      draggable && 'select-none [-webkit-touch-callout:none]',
    ]}
    onclick={onTitleClick}
  >
    {#if draggable}
      <svg
        class="hidden size-4 shrink-0 cursor-grab text-muted-foreground opacity-0 group-hover/row:opacity-100 can-hover:block"
        viewBox="0 0 16 16"
        fill="currentColor"
        aria-hidden="true"
      >
        <circle cx="6" cy="3.5" r="1.1" /><circle cx="10" cy="3.5" r="1.1" />
        <circle cx="6" cy="8" r="1.1" /><circle cx="10" cy="8" r="1.1" />
        <circle cx="6" cy="12.5" r="1.1" /><circle cx="10" cy="12.5" r="1.1" />
      </svg>
    {/if}
    {#if showIndent}
      {#if level > 0}
        <span class="shrink-0" style:width="{level * 1.25}rem" aria-hidden="true"></span>
      {/if}
      {#if hasChildren}
        <button
          type="button"
          data-inner
          data-hierarchy-toggle={card.id}
          tabindex="-1"
          aria-expanded={!collapsed}
          aria-label={collapsed
            ? m.card_hierarchy_expand({ title })
            : m.card_hierarchy_collapse({ title })}
          class="inline-flex size-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-background/80 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring roomy:size-touch"
          onclick={() => {
            host.toggleParent(card.id);
          }}
        >
          {#if collapsed}
            <ChevronRightIcon class="size-3.5" aria-hidden="true" />
          {:else}
            <ChevronDownIcon class="size-3.5" aria-hidden="true" />
          {/if}
        </button>
      {:else}
        <span class="size-6 shrink-0 roomy:size-touch" aria-hidden="true"></span>
      {/if}
    {/if}
    <CardNumber number={card.number} class="ps-1" />
    {#if card.fields.icon}
      <span class="shrink-0 ps-1" aria-hidden="true">{card.fields.icon}</span>
    {/if}
    {#if editingColumn === TITLE_COLUMN_ID}
      <TextField
        label={m['TableComponent.name']()}
        value={card.title}
        placeholder={m['KanbanCard.untitled']()}
        spellcheck
        autofocus
        class="mx-0.5"
        onSave={(text: string) => {
          host.setTitle(card.id, text);
        }}
      />
    {:else}
      <span
        data-slot="row-title"
        class={['min-w-0 flex-1 truncate px-2', card.title === '' && 'text-muted-foreground']}
        {title}>{title}</span
      >
    {/if}
    <CardPresence {board} cardId={card.id} class="px-1" />
    <span
      class="flex shrink-0 items-center gap-0.5 bg-inherit can-hover:absolute can-hover:inset-y-0 can-hover:end-1 can-hover:opacity-0 can-hover:group-focus-within/row:opacity-100 can-hover:group-hover/row:opacity-100 can-hover:has-[[aria-expanded=true]]:opacity-100"
    >
      <a
        href={host.cardHref(card.id)}
        data-open-card
        data-inner
        tabindex="-1"
        class="hidden h-7 shrink-0 items-center gap-1 rounded-md border bg-background px-2 text-xs font-medium text-foreground hover:bg-muted can-hover:flex"
        onclick={(event) => {
          // A plain click opens the card over the table; with a modifier the browser opens the address.
          if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
          event.preventDefault();
          host.open(card.id);
        }}
      >
        {m['TableRow.open']()}
      </a>
      {#if menuMade}
        <ActionMenu
          bind:open={menuOpen}
          title={m['TableRow.MoreOption']()}
          subtitle={title}
          actions={host.rowActions(card)}
        >
          {#snippet trigger(props: Record<string, unknown>)}
            <button
              {...props}
              type="button"
              aria-label={m['TableRow.MoreOption']()}
              title={m['TableRow.MoreOption']()}
              data-inner
              data-more={card.id}
              tabindex="-1"
              class={MORE_BUTTON}
            >
              {@render dots()}
            </button>
          {/snippet}
        </ActionMenu>
      {:else}
        <button
          type="button"
          aria-label={m['TableRow.MoreOption']()}
          title={m['TableRow.MoreOption']()}
          aria-haspopup="menu"
          data-inner
          data-more={card.id}
          tabindex="-1"
          class={MORE_BUTTON}
          onclick={() => void openMenu()}
        >
          {@render dots()}
        </button>
      {/if}
    </span>
  </div>

  {#each columns as column (column.id)}
    {@const template = column.template}
    {@const editing = editingColumn === column.id}
    <!-- svelte-ignore a11y_click_events_have_key_events (the table's key handler turns Enter on a cell into this click) -->
    <div
      role="gridcell"
      tabindex="-1"
      data-cell
      data-kind={template.type === 'checkbox' ? 'checkbox' : 'value'}
      data-col={column.id}
      data-editing={editing ? '' : undefined}
      class={['flex min-w-0 items-center border-e', CELL_FOCUS, !editing && 'overflow-hidden']}
      onclick={(event) => {
        if (editing) return;
        if (event.target instanceof Element && event.target.closest('a, button')) return;
        host.activate(card, template);
      }}
    >
      {#if editing}
        <PropertyValue
          {template}
          {card}
          value={card.fields.properties[column.id]}
          mode="edit"
          startOpen
          locale={reader.locale}
          regional={reader.regional}
          users={reader.people}
          named={reader.users}
          presentation={reader.presentation}
          cards={template.type === 'relation' ? host.cards : undefined}
          preventLoops={host.ordering(column.id)}
          onOpenCard={host.open}
          onChange={(value: StoredValue | undefined) => {
            host.setValue(card.id, column.id, value);
          }}
        />
      {:else}
        <!-- A checkbox that only shows swallows a press; the cell takes it instead. -->
        <div class={['min-w-0 flex-1', template.type === 'checkbox' && 'pointer-events-none']}>
          <PropertyValue
            {template}
            {card}
            value={card.fields.properties[column.id]}
            mode="display"
            locale={reader.locale}
            regional={reader.regional}
            users={reader.users}
            cards={template.type === 'relation' ? host.cards : undefined}
            onOpenCard={host.open}
            innerTabindex={-1}
          />
        </div>
      {/if}
    </div>
  {/each}
  <div role="presentation"></div>
</div>
