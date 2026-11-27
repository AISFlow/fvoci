<!--
  The cards of a view as a grid.

  How many cards stand side by side follows the width of the box the gallery
  is in: as many as fit at the least width of a card (`MIN_CARD`, in the
  class below), and one where not even that fits. Nothing else changes with
  the width.

  A card shows the same face as on a board, larger: icon and title, the
  first picture of its body and the beginning of its text, the properties
  the view turned on, how its tasks stand. The previews are read only while
  a gallery is shown (`CardPreviews`), and a picture is fetched once its
  card comes near the screen.

  The order is the view's. Cards are not dragged here: the grid runs along
  two axes and lib/drag sorts along one. A sort of the view orders them.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';

  import CardPresence from '../CardPresence.svelte';
  import CardFace from '$lib/components/card-face/CardFace.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { propertiesThatMeetFilterGroup } from '$lib/logic/filter.ts';

  import { badgesOf, faceContextOf } from '../kanban/board-data.ts';
  import CardMenu from '../kanban/CardMenu.svelte';
  import { CARD_MENU_BUTTON } from '../kanban/kanban.ts';
  import type { ViewProps } from '../props.ts';
  import { whileHere } from '$lib/state/index.ts';

  let { board, view, cards, cardHref, onOpenCard }: ViewProps = $props();

  const uid = $props.id();
  const face = $derived(faceContextOf(board, view));
  const previews = $derived(board.previews);

  // Each card's menu is mounted only when asked for, so a large gallery builds plain buttons.
  const menusMade = new SvelteSet<string>();
  let openMenuId = $state<string | undefined>(undefined);

  let shownViewId = untrack(() => view.id);
  $effect(() => {
    if (view.id === shownViewId) return;
    shownViewId = view.id;
    untrack(() => {
      openMenuId = undefined;
      menusMade.clear();
    });
  });

  async function openMenu(cardId: string, event: Event): Promise<void> {
    const holder = (event.currentTarget as HTMLElement | null)?.parentElement;
    menusMade.add(cardId);
    await tick();
    holder?.querySelector<HTMLElement>('[data-card-menu]')?.focus({ preventScroll: true });
    openMenuId = cardId;
  }

  /** The address of a picture of the board; none where the backend has no bytes for it. */
  function imageOf(fileId: string | undefined): string | undefined {
    return fileId ? board.workspace.backend.fileUrl(board.boardId, fileId) : undefined;
  }

  /** A new card that the view will show: it gets the values the view's filter asks for. */
  async function addCard(): Promise<void> {
    if (!board.board) return;
    const properties = propertiesThatMeetFilterGroup(
      view.fields.filter,
      board.board.cardProperties,
    );
    const card = await whileHere(board, () => board.createCard({ properties }));
    if (card) onOpenCard(card.id);
  }

  function onClick(event: MouseEvent, cardId: string): void {
    const target = event.target;
    // The link of a URL property and its buttons keep their own presses.
    if (target instanceof Element && target.closest('button, input, a[href]')) return;
    onOpenCard(cardId);
  }
</script>

<!-- The previews are read once per open board visit and reused across view switches (§1.3.6). -->
<div
  class="h-full overflow-y-auto pb-(--fab-room,0px)"
  data-testid="gallery"
  {@attach () => previews.retain()}
>
  <!-- MIN_CARD is the 14rem below: a card is never narrower, unless the box itself is. -->
  <ul
    aria-label={m.gallery_cards()}
    data-testid="gallery-grid"
    class="grid grid-cols-[repeat(auto-fill,minmax(min(100%,14rem),1fr))] gap-3 p-3 pb-[calc(0.75rem+var(--safe-bottom))] @header/pane:gap-4 @header/pane:p-4 @header/pane:pb-[calc(1rem+var(--safe-bottom))]"
  >
    {#each cards as card (card.id)}
      {@const badges = badgesOf(card)}
      {@const subcards = face.hierarchy?.progressOf(card.id)}
      {@const parentHint = face.hierarchy?.parentOf(card)}
      {@const preview = previews.of(card.id)}
      <li class="group/card relative min-w-0">
        <div
          role="link"
          data-card={card.id}
          tabindex="0"
          aria-labelledby="{uid}-{card.id}"
          class="h-full cursor-pointer rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onclick={(event) => {
            onClick(event, card.id);
          }}
          onkeydown={(event) => {
            if (event.key !== 'Enter' || event.target !== event.currentTarget) return;
            event.preventDefault();
            onOpenCard(card.id);
          }}
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
            size="large"
            previewImage={imageOf(preview?.imageFileId)}
            previewText={preview?.text}
            titleId="{uid}-{card.id}"
            locale={face.locale}
            regional={face.regional}
            users={face.users}
            menuRoom
            class="h-full"
          >
            {#snippet readers()}<CardPresence {board} cardId={card.id} class="mt-2" />{/snippet}
          </CardFace>
        </div>
        <div data-slot="card-menu" class="absolute top-2 right-2 roomy:top-1 roomy:right-1">
          {#if menusMade.has(card.id)}
            <CardMenu
              bind:open={
                () => openMenuId === card.id,
                (next: boolean) => {
                  if (next) openMenuId = card.id;
                  else if (openMenuId === card.id) openMenuId = undefined;
                }
              }
              {board}
              {card}
              href={cardHref(card.id)}
            />
          {:else}
            {@const label = m.kanban_card_menu({ title: card.title || m['KanbanCard.untitled']() })}
            <button
              type="button"
              aria-label={label}
              title={label}
              aria-haspopup="menu"
              aria-expanded="false"
              data-card-menu={card.id}
              class={CARD_MENU_BUTTON}
              onclick={(event) => void openMenu(card.id, event)}
              onkeydown={(event) => {
                if (event.key !== 'ArrowDown') return;
                event.preventDefault();
                void openMenu(card.id, event);
              }}
            >
              <svg class="size-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                <circle cx="3" cy="8" r="1.25" /><circle cx="8" cy="8" r="1.25" />
                <circle cx="13" cy="8" r="1.25" />
              </svg>
            </button>
          {/if}
        </div>
      </li>
    {/each}
    {#if board.canEdit}
      <li class="min-w-0">
        <button
          type="button"
          data-testid="gallery-new"
          class="flex h-full min-h-14 w-full items-center justify-center rounded-md border border-dashed border-input-border px-4 text-sm text-muted-foreground hover:bg-muted roomy:min-h-touch"
          onclick={() => void addCard()}
        >
          {m['TableComponent.plus-new']()}
        </button>
      </li>
    {/if}
  </ul>
</div>
