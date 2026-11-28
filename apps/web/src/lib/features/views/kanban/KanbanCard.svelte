<!--
  One card in a column: the face, what makes it a thing to open, select and
  drag, and its menu laid over the corner.

  The card is a link to its own address: a press opens it. With Ctrl or ⌘
  the press takes the card into or out of the selection, with Shift it
  selects the range up to it. Enter does the same from the keyboard, Space
  picks the card up (lib/drag). What a plain press means while cards are
  being selected is the board's to say (`onActivate`).

  The card stays in the document while it is dragged, hidden: a finger that
  drags it keeps sending its touch events to this element. The menu is a
  sibling of the link, not inside it: a button in a link is no button to a
  screen reader.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import CardPresence from '../CardPresence.svelte';
  import CardFace from '$lib/components/card-face/CardFace.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Card } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  import { badgesOf, type FaceContext } from './board-data.ts';
  import CardMenu from './CardMenu.svelte';
  import { activationOf, type Activation } from '../selection.svelte.ts';

  import { CARD_MENU_BUTTON, type MoveTarget } from './kanban.ts';

  interface Props {
    board: BoardStore;
    card: Card;
    face: FaceContext;
    cardHref: (cardId: string) => string;
    /** Its place among the cards of the column not on their way, as the drag counts; -1 on its way. */
    at: number;
    /** Its place in the column and how many cards the column has: for a screen reader, which may not be told of all. */
    position: number;
    count: number;
    /** Is told of the card's box while it is drawn, to know how high it is (a long column, KanbanColumn). */
    measure: ((element: HTMLElement, cardId: string) => () => void) | undefined;
    /** On its way with a drag: hidden where it was. */
    dragged: boolean;
    selected: boolean;
    /** The cards the menu's "move" and "delete" act on. */
    targetsOf: (cardId: string) => readonly string[];
    /** The columns the card can be moved to; empty when it cannot. */
    columns: readonly MoveTarget[];
    /** The id of the text that says how to drag with the keyboard. */
    describedBy: string | undefined;
    onActivate: (cardId: string, how: Activation) => void;
    /** Takes the card into or out of the selection. Undefined: cards cannot be selected. */
    onSelect: ((cardId: string) => void) | undefined;
    onMove: (cardIds: readonly string[], optionId: string) => void;
  }

  let {
    board,
    card,
    face,
    cardHref,
    at,
    position,
    count,
    measure,
    dragged,
    selected,
    targetsOf,
    columns,
    describedBy,
    onActivate,
    onSelect,
    onMove,
  }: Props = $props();

  const uid = $props.id();
  const badges = $derived(badgesOf(card));
  const subcards = $derived(face.hierarchy?.progressOf(card.id));
  const parentHint = $derived(face.hierarchy?.parentOf(card));
  const menuLabel = $derived(
    m.kanban_card_menu({ title: card.title || m['KanbanCard.untitled']() }),
  );

  // The menu is mounted only when asked for: scrolling a long column would otherwise build two menus per card.
  let menuMade = $state(false);
  let menuOpen = $state(false);

  async function openMenu(event: Event): Promise<void> {
    const holder = (event.currentTarget as HTMLElement | null)?.parentElement;
    menuMade = true;
    await tick();
    holder?.querySelector<HTMLElement>('[data-card-menu]')?.focus({ preventScroll: true });
    menuOpen = true;
  }

  function activate(event: MouseEvent | KeyboardEvent): void {
    const how = activationOf(event);
    if (how) onActivate(card.id, how);
  }

  function onClick(event: MouseEvent): void {
    const target = event.target;
    // The link of a URL property and its buttons keep their own presses.
    if (target instanceof Element && target.closest('button, input, a[href]')) return;
    activate(event);
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key !== 'Enter' || event.target !== event.currentTarget) return;
    event.preventDefault();
    activate(event);
  }
</script>

<div
  role="listitem"
  aria-posinset={position + 1}
  aria-setsize={count}
  class={['group/card relative shrink-0', dragged && 'hidden']}
  {@attach (element: HTMLElement) => measure?.(element, card.id)}
>
  <div
    role="link"
    data-drag-item={card.id}
    data-drag-at={at >= 0 ? at : undefined}
    data-card={card.id}
    data-selected={selected ? '' : undefined}
    tabindex="0"
    aria-labelledby="{uid}-title"
    aria-describedby={describedBy}
    class="cursor-pointer rounded-md outline-none select-none [-webkit-touch-callout:none] focus-visible:ring-2 focus-visible:ring-ring"
    onclick={onClick}
    onkeydown={onKeyDown}
  >
    <CardFace
      {card}
      templates={face.templates}
      shownProperties={face.shownProperties}
      hasBody={badges.hasBody}
      tasksTotal={badges.tasksTotal}
      tasksDone={badges.tasksDone}
      subcardsTotal={subcards?.total ?? 0}
      subcardsDone={subcards?.done ?? 0}
      {parentHint}
      titleId="{uid}-title"
      locale={face.locale}
      regional={face.regional}
      users={face.users}
      {selected}
      menuRoom
    >
      {#snippet readers()}<CardPresence {board} cardId={card.id} class="mt-2" />{/snippet}
    </CardFace>
    {#if selected}
      <span class="sr-only">{m.kanban_selected()}</span>
    {/if}
  </div>
  <div data-slot="card-menu" class="absolute top-1 right-1 roomy:top-0 roomy:right-0">
    {#if menuMade}
      {@const targets = targetsOf(card.id)}
      <CardMenu
        bind:open={menuOpen}
        {board}
        {card}
        href={cardHref(card.id)}
        targetIds={targets}
        {columns}
        {selected}
        onSelect={onSelect &&
          (() => {
            onSelect(card.id);
          })}
        onMove={(optionId: string) => {
          onMove(targets, optionId);
        }}
      />
    {:else}
      <button
        type="button"
        aria-label={menuLabel}
        title={menuLabel}
        aria-haspopup="menu"
        aria-expanded="false"
        data-card-menu={card.id}
        class={CARD_MENU_BUTTON}
        onclick={(event) => void openMenu(event)}
        onkeydown={(event) => {
          if (event.key !== 'ArrowDown') return;
          event.preventDefault();
          void openMenu(event);
        }}
      >
        <svg class="size-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
          <circle cx="3" cy="8" r="1.25" /><circle cx="8" cy="8" r="1.25" />
          <circle cx="13" cy="8" r="1.25" />
        </svg>
      </button>
    {/if}
  </div>
</div>
