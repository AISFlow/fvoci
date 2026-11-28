<!--
  One column of the board: a heading and a list of cards that scrolls on its
  own.

  The column is an item of the board's row of columns, dragged by its heading
  (scope "column", lib/drag); its cards are items of the list inside it (no
  scope). While cards are on their way the list shows a gap where they would
  land. The cards themselves stay in the list, hidden, and so does the column
  while a pointer drags it: a finger keeps sending its touch events to the
  element it landed on.

  A long column draws a window of its cards (column-window.ts) and a blank
  for the rest. Each card tells a screen reader where it is among all
  (`aria-posinset`), and the drag where it is among those that stay
  (`data-drag-at`). The page's own search finds only the cards drawn.

  The width is a matter of the pane, in classes: as wide as the pane less a
  sliver of the neighbours where only one column fits, a fixed width where
  two do.
-->
<script lang="ts">
  import PlusIcon from '@lucide/svelte/icons/plus';
  import { tick, untrack } from 'svelte';

  import CardFace from '$lib/components/card-face/CardFace.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { isComposing } from '$lib/keys.ts';
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BoardGroup } from '$lib/logic/group.ts';
  import type { Card, PropertyTemplate } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';
  import { Input } from '$lib/ui/input/index.js';

  import { badgesOf, changeOption, type FaceContext } from './board-data.ts';
  import ColumnMenu from './ColumnMenu.svelte';
  import {
    CardHeights,
    cardsBetween,
    contentHeight,
    drawnRuns,
    guessHeight,
    piecesOf,
    topsOf,
    WINDOW_FROM,
  } from './column-window.ts';
  import type { Activation } from '../selection.svelte.ts';

  import { columnIdOf, type MoveTarget } from './kanban.ts';
  import KanbanCard from './KanbanCard.svelte';

  interface Props {
    board: BoardStore;
    /** The view showing this column: another view may reuse the same column with other card shapes. */
    viewId: string;
    groupBy: PropertyTemplate;
    group: BoardGroup;
    /** The name the column shows: of the option, or of the person. */
    name: string;
    /** The id of the column's box, which its tab names. */
    panelId: string;
    /** Where the column currently stands in the row, including while the keyboard carries it. */
    drawnAt: number;
    face: FaceContext;
    /** The reader may change the board. */
    editable: boolean;
    /** Cards can be made in, and moved into, this column: its value can be set on a card. */
    settable: boolean;
    /** The columns are options of a select property: they have names, colours and an order. */
    isOption: boolean;
    /** The column can go a place to the left, to the right. Undefined: columns cannot be reordered. */
    canStep: { left: boolean; right: boolean } | undefined;
    noValueName: string;
    /** The other columns, for the menu of a card. */
    columns: readonly MoveTarget[];
    cardHref: (cardId: string) => string;
    /** The cards on their way, wherever they come from. */
    draggedIds: readonly string[];
    /** Where the gap is, counted among the cards without the dragged ones; -1 for no gap. */
    gapAt: number;
    gapHeight: number;
    /** Drawn in the gap instead of an outline: the card the keyboard carries. */
    carried: Card | undefined;
    carriedCount: number;
    selectedIds: readonly string[];
    /** A pointer drags this column: it stays in the document, out of sight. */
    hidden: boolean;
    /** The keyboard carries this column: its heading is the stand-in that holds the focus. */
    lifted: boolean;
    /** The heading is being renamed. */
    renaming: boolean;
    cardHelpId: string;
    columnHelpId: string;
    onActivate: (cardId: string, how: Activation) => void;
    onSelect: ((cardId: string) => void) | undefined;
    onMove: (cardIds: readonly string[], optionId: string) => void;
    onAddCard: (optionId: string) => void;
    onHide: (optionId: string) => void;
    onStep: (optionId: string, step: -1 | 1) => void;
    onRenaming: (optionId: string | undefined) => void;
  }

  let {
    board,
    viewId,
    groupBy,
    group,
    name,
    panelId,
    drawnAt,
    face,
    editable,
    settable,
    isOption,
    canStep,
    noValueName,
    columns,
    cardHref,
    draggedIds,
    gapAt,
    gapHeight,
    carried,
    carriedCount,
    selectedIds,
    hidden,
    lifted,
    renaming,
    cardHelpId,
    columnHelpId,
    onActivate,
    onSelect,
    onMove,
    onAddCard,
    onHide,
    onStep,
    onRenaming,
  }: Props = $props();

  const option = $derived(group.option);
  /** A column that is an option: the column of cards without a value is not one. */
  const named = $derived(isOption && option.id !== '');
  const orderable = $derived(canStep !== undefined);

  const GAP_KEY = ' gap';
  const NO_COLUMNS: readonly MoveTarget[] = [];

  // Read through these, the list below is put together again only when the
  // gap of this column or the dragged cards change, not on every move of a
  // gap in another column.
  const gap = $derived(gapAt);
  const dragged = $derived(draggedIds);

  // ------------------------------------------------------------- window

  // A long column draws a window of its cards (column-window.ts): those near
  // what is in sight, near the focus, the gap and the card opened last.
  const windowed = $derived(group.cards.length > WINDOW_FROM);
  let list = $state<HTMLElement | null>(null);
  let scrollTop = $state(0);
  let viewHeight = $state(0);
  /** The card that has, or last had, the focus in the list. */
  let focusId = $state<string | undefined>(undefined);
  const cardHeights = new CardHeights(
    (cardId) => guesses?.get(cardId),
    (next) => {
      scrollTop = next;
    },
  );

  const templates = $derived(face.templates);
  const shownProperties = $derived(face.shownProperties);
  const heightKey = $derived(`${viewId}\u0000${shownProperties.join('\u0000')}`);
  let lastHeightKey = untrack(() => heightKey);
  $effect(() => {
    const next = heightKey;
    if (next === lastHeightKey) return;
    lastHeightKey = next;
    untrack(() => {
      cardHeights.clear();
    });
  });

  /** The cards that are not on their way: what the drag counts and the list stacks. */
  const staying = $derived(
    dragged.length === 0 ? group.cards : group.cards.filter((card) => !dragged.includes(card.id)),
  );
  const guesses = $derived(
    windowed
      ? new Map(
          group.cards.map((card) => [card.id, guessHeight(card, { templates, shownProperties })]),
        )
      : undefined,
  );
  const heights = $derived(
    guesses ? staying.map((card) => cardHeights.get(card.id) ?? guesses.get(card.id) ?? 0) : [],
  );
  const tops = $derived(topsOf(heights));
  const atOf = $derived(windowed ? new Map(staying.map((card, at) => [card.id, at])) : undefined);
  // The card opened last stays drawn once it is closed, even if the column crossed `WINDOW_FROM` meanwhile.
  let openedLast: string | undefined;
  $effect(() => {
    const cardId = board.openCard?.cardId;
    if (cardId !== undefined) openedLast = cardId;
  });
  const opened = $derived(board.openCard?.cardId ?? openedLast);

  const pieces = $derived.by(() => {
    if (!atOf) return piecesOf(group.cards, dragged, gap, undefined, heights);
    // A screen above and one below what is in sight, so that a fling finds cards drawn.
    const screen = viewHeight || 800;
    const inSight = cardsBetween(tops, scrollTop - screen, scrollTop + 2 * screen, gap, gapHeight);
    const pin = (cardId: string | undefined): number =>
      cardId === undefined ? -1 : (atOf.get(cardId) ?? -1);
    const runs = drawnRuns(staying.length, inSight, [pin(focusId), gap, pin(opened)]);
    return piecesOf(group.cards, dragged, gap, runs, heights);
  });

  // A column moved among the others (including while carried by the keyboard or
  // put back on cancel), or shown again once a pointer has dragged it, may come
  // back scrolled to its top without a scroll event: put it back where its window is.
  $effect(() => {
    if (drawnAt < 0 || panelId === '' || hidden || !list) return;
    const wanted = untrack(() => scrollTop);
    if (list.scrollTop !== wanted) list.scrollTop = wanted;
  });

  /** The card of whatever took the focus in the list: the card, its menu, the stand-in it is carried as. */
  function onFocusIn(event: FocusEvent): void {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const cardId =
      target
        .closest('[role="listitem"]')
        ?.querySelector('[data-card]')
        ?.getAttribute('data-card') ??
      target.closest('[data-drag-carried]')?.getAttribute('data-drag-carried');
    if (cardId) focusId = cardId;
  }

  const targetsOf = (cardId: string): readonly string[] =>
    selectedIds.includes(cardId) ? selectedIds : [cardId];
  /** The other columns, and this one marked as where the card is. */
  const moveTargets = $derived(
    columns.map((column) => ({ ...column, current: column.optionId === option.id })),
  );

  // ------------------------------------------------------------- renaming

  let draft = $state('');
  let field = $state<HTMLInputElement | null>(null);
  let heading = $state<HTMLElement | null>(null);

  $effect(() => {
    if (!renaming) return;
    draft = option.label;
    void tick().then(() => {
      field?.focus();
      field?.select();
    });
  });

  async function endRename(save: boolean): Promise<void> {
    if (!renaming) return;
    const text = draft.trim();
    onRenaming(undefined);
    if (save && text !== '' && text !== option.label) {
      void changeOption(board, groupBy.id, option.id, { label: text });
    }
    await tick();
    heading?.focus();
  }
</script>

{#snippet label()}
  <h3 id="{panelId}-name" class="flex min-w-0 items-center text-sm font-medium">
    {#if named}
      <OptionChip name={option.label} color={option.color} />
    {:else}
      <span class="truncate" title={name}>{name}</span>
    {/if}
  </h3>
  <span class="shrink-0 text-xs text-muted-foreground" data-column-count>
    <span aria-hidden="true">{group.cards.length}</span>
    <span class="sr-only">{m.kanban_card_count({ count: group.cards.length })}</span>
  </span>
{/snippet}

<section
  id={panelId}
  data-panel
  data-drag-item={orderable ? columnIdOf(option.id) : undefined}
  data-drag-scope={orderable ? 'column' : undefined}
  data-column={option.id}
  aria-labelledby="{panelId}-name"
  class={[
    'flex max-h-full w-full max-w-sm shrink-0 snap-center flex-col rounded-lg bg-muted @columns/pane:w-64 @columns/pane:max-w-none @columns/pane:snap-align-none @header/pane:w-68',
    hidden && 'hidden',
    lifted && 'ring-2 ring-ring',
  ]}
>
  <header class="flex min-h-10 shrink-0 items-center gap-0.5 pt-1 pr-1 pl-1 roomy:min-h-12">
    {#if renaming}
      <Input
        bind:ref={field}
        bind:value={draft}
        aria-label={m.kanban_group_name()}
        autocomplete="off"
        enterkeyhint="done"
        class="mx-1 h-8 min-w-0 flex-1 bg-background roomy:h-10 roomy:text-base"
        onblur={() => void endRename(true)}
        onkeydown={(event: KeyboardEvent) => {
          if (isComposing(event)) return;
          if (event.key === 'Enter') {
            event.preventDefault();
            void endRename(true);
          } else if (event.key === 'Escape') {
            event.preventDefault();
            event.stopPropagation();
            void endRename(false);
          }
        }}
      />
    {:else if orderable}
      <!--
        The heading is the handle of the column where columns can be moved:
        a mouse drags it at once, a finger after resting on it, and Space
        picks the column up from the keyboard.
      -->
      <div
        bind:this={heading}
        data-drag-handle
        data-drag-scope="column"
        data-drag-carried={lifted ? columnIdOf(option.id) : undefined}
        data-column-heading={option.id}
        role="button"
        tabindex="0"
        aria-label={name}
        aria-describedby={columnHelpId}
        class="flex min-h-8 min-w-0 flex-1 cursor-grab items-center gap-2 self-stretch rounded-md pl-2 outline-none select-none [-webkit-touch-callout:none] focus-visible:ring-2 focus-visible:ring-ring"
      >
        {@render label()}
      </div>
    {:else}
      <div
        data-column-heading={option.id}
        class="flex min-h-8 min-w-0 flex-1 items-center gap-2 self-stretch pl-2"
      >
        {@render label()}
      </div>
    {/if}
    {#if editable}
      {#if settable}
        <IconButton
          label={m.kanban_new_card_in({ name })}
          data-column-add={option.id}
          class="shrink-0 text-muted-foreground"
          onclick={() => {
            onAddCard(option.id);
          }}
        >
          <PlusIcon aria-hidden="true" />
        </IconButton>
      {/if}
      <ColumnMenu
        {board}
        {groupBy}
        {option}
        {name}
        isOption={named}
        {canStep}
        {noValueName}
        onHide={() => {
          onHide(option.id);
        }}
        onStep={(step: -1 | 1) => {
          onStep(option.id, step);
        }}
        onRename={() => {
          onRenaming(option.id);
        }}
      />
    {/if}
  </header>

  <div
    bind:this={list}
    role="list"
    aria-labelledby="{panelId}-name"
    data-drag-list={columnIdOf(option.id)}
    data-drag-scroll="y"
    data-column-cards={option.id}
    class={[
      'relative flex min-h-12 flex-col gap-2 overflow-y-auto overscroll-y-contain p-2 pb-[max(0.5rem,calc(var(--fab-room,0px)+var(--bulk-room,0px)))]',
      windowed && '[overflow-anchor:none]',
    ]}
    bind:clientHeight={viewHeight}
    onscroll={(event) => {
      scrollTop = event.currentTarget.scrollTop;
    }}
    onfocusin={onFocusIn}
    {@attach cardHeights.watch}
  >
    {#if windowed}
      <!--
        As high as the whole column: what scrolls keeps its height while the
        window moves, though the cards that leave it go before the blank that
        stands for them grows, and a browser keeps the scroll where it is.
      -->
      <div
        aria-hidden="true"
        class="pointer-events-none absolute top-0 left-0 w-px"
        style:height="{contentHeight(tops, gap, gapHeight)}px"
      ></div>
    {/if}
    {#each pieces as entry (entry.kind === 'card' ? entry.card.id : entry.kind === 'gap' ? GAP_KEY : ` blank ${entry.key}`)}
      {#if entry.kind === 'card'}
        <KanbanCard
          {board}
          card={entry.card}
          {face}
          {cardHref}
          at={entry.at}
          position={entry.position}
          count={group.cards.length}
          measure={windowed ? cardHeights.measure : undefined}
          dragged={draggedIds.includes(entry.card.id)}
          selected={selectedIds.includes(entry.card.id)}
          {targetsOf}
          columns={settable ? moveTargets : NO_COLUMNS}
          describedBy={editable ? cardHelpId : undefined}
          {onActivate}
          {onSelect}
          {onMove}
        />
      {:else if entry.kind === 'blank'}
        <!-- An empty blank at an end takes back the room between it and the cards. -->
        <div
          data-column-blank
          aria-hidden="true"
          class={[
            'shrink-0',
            entry.cards === 0 && entry.key === 'start' && '-mb-2',
            entry.cards === 0 && entry.key === 'end' && '-mt-2',
          ]}
          style:height="{entry.height}px"
        ></div>
      {:else if carried}
        {@const badges = badgesOf(carried)}
        <div
          role="listitem"
          data-drag-carried={carried.id}
          tabindex="-1"
          aria-describedby={cardHelpId}
          class="shrink-0 rounded-md outline-none select-none"
        >
          <CardFace
            card={carried}
            templates={face.templates}
            shownProperties={face.shownProperties}
            hasBody={badges.hasBody}
            tasksTotal={badges.tasksTotal}
            tasksDone={badges.tasksDone}
            locale={face.locale}
            regional={face.regional}
            users={face.users}
            lifted
            count={carriedCount}
          />
        </div>
      {:else}
        <div
          data-drag-gap
          aria-hidden="true"
          class="shrink-0 rounded-md border-2 border-dashed border-input-border"
          style:height="{gapHeight}px"
        ></div>
      {/if}
    {/each}
  </div>
  {#if editable && settable}
    <button
      type="button"
      data-column-new={option.id}
      class="mx-2 mb-2 flex min-h-8 shrink-0 items-center gap-1.5 rounded-md px-2 text-left text-sm text-muted-foreground hover:bg-background/60 roomy:min-h-touch"
      onclick={() => {
        onAddCard(option.id);
      }}
    >
      {m['BoardComponent.new']()}
      <span class="sr-only">: {name}</span>
    </button>
  {/if}
</section>
