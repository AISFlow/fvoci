<!--
  The card that is open over a board.

  One dialog in three frames: the whole screen on a compact page, a panel at
  the right on a medium one, a dialog in the middle on an expanded one
  (`cardFrame`, lib/layout/presentation.ts). Only the classes of the dialog
  differ (frame.ts). Nothing inside is put together again when the window
  crosses a stage, so the text being typed, the caret, the focus and the
  scroll position stay; and when the address names another card the dialog
  stays too and only what it shows is replaced.

  The card is an address: closing it is the router's business (`onClose`),
  and so is the back button. This component keeps the board store's open
  card in step with the address, and sends what was written to a card before
  letting go of it.
-->
<script lang="ts">
  import { onMount, untrack, type Snippet } from 'svelte';

  import { display } from '$lib/display.svelte.ts';
  import { holdSaveKey } from '$lib/editor/save-key.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { cardFrame } from '$lib/layout/presentation.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import type { BoardView } from '$lib/logic/wire.ts';
  import { whileHere, type BoardStore, type CardStore } from '$lib/state/index.ts';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import CardContent from './CardContent.svelte';
  import { FRAME_CLASS } from './frame.ts';

  interface Props {
    board: BoardStore;
    /** The view the card was opened from. */
    view: BoardView | undefined;
    /** The card to show. Undefined: no card is open. */
    cardId: string | undefined;
    /** The address of a card of the board: its link to copy. */
    cardHref: (cardId: string) => string;
    onClose: () => void;
    /** Shows another card in place of this one: the copy "Duplicate" made, a card a relation names. */
    onOpenCard?: (cardId: string) => void;
    /** Extra actions in the card header (e.g. personal timer), supplied by the app shell. */
    headerActions?: Snippet<[CardStore]> | undefined;
  }

  let { board, view, cardId, cardHref, onClose, onOpenCard, headerActions }: Props = $props();

  // The store's open card follows the address. A card that is left sends
  // what was written in its body as it leaves the body's room.
  $effect(() => {
    const store = board;
    const id = cardId;
    untrack(() => {
      if (id) void store.showCard(id);
      else store.closeCard();
    });
  });

  // The tab is closed or put away with something not sent: send it while there is time.
  onMount(() => {
    const save = (): void => {
      void board.openCard?.settleBody();
    };
    const whenHidden = (): void => {
      if (document.visibilityState === 'hidden') save();
    };
    window.addEventListener('pagehide', save);
    document.addEventListener('visibilitychange', whenHidden);
    return () => {
      window.removeEventListener('pagehide', save);
      document.removeEventListener('visibilitychange', whenHidden);
    };
  });

  const card = $derived(board.openCard?.cardId === cardId ? board.openCard : undefined);
  const frame = $derived(cardFrame(page.stage, display.resolved.cardOpen));
  const title = $derived(card?.summary?.title || m['KanbanCard.untitled']());

  let content = $state<HTMLElement | null>(null);
  /** The card that was shown last, to give the focus back to where it was opened from. */
  let shown: string | undefined;
  $effect(() => {
    if (cardId !== undefined) shown = cardId;
  });

  /** The card whose closing was held back once because what was written in its body had not gone. */
  let held: unknown;

  /**
   * Whether the card may be left, for another card or for the board. What
   * the reader wrote in the body is sent first. When it does not reach the
   * server (no connection, not in the room), or a file is still going up
   * for the body (shown in its place), the card stays open once, so that
   * nothing is lost by a careless tap. Leaving again leaves.
   */
  function canLeaveNow(): boolean {
    const open = card;
    if (!open || held === open) return true;
    return !(open.bodyPending || open.filesGoingUp);
  }

  async function mayLeave(): Promise<boolean> {
    const open = card;
    // Asked before and after the sending: a function, so that the second answer is not taken for the first.
    const unsaved = (): boolean => (open?.bodyPending ?? false) || (open?.filesGoingUp ?? false);
    if (open && unsaved() && held !== open) {
      await open.settleBody();
      if (unsaved() && board.openCard === open) {
        held = open;
        return false;
      }
    }
    return true;
  }

  function close(): void {
    if (canLeaveNow()) {
      onClose();
      return;
    }
    void mayLeave().then((ok) => {
      if (ok) onClose();
    });
  }

  /** Shows another card in place of this one, when this one may be left. */
  async function openOther(otherId: string): Promise<void> {
    if (await whileHere(board, mayLeave)) onOpenCard?.(otherId);
  }

  /**
   * The dialog itself takes the focus, not its first field: a focused field
   * brings up the on-screen keyboard before the card has been read. A card
   * that starts without a title asks for the title itself (CardTitle).
   */
  function focusOnOpen(event: Event): void {
    event.preventDefault();
    if (content && !content.contains(document.activeElement)) content.focus();
  }

  /** Back to the card on the board: the one this was opened from, or the one shown last. */
  function focusOnClose(event: Event): void {
    if (shown === undefined) return;
    const viaTitle = document.querySelector<HTMLElement>(
      `[data-card-return="${CSS.escape(shown)}"]`,
    );
    if (viaTitle) {
      event.preventDefault();
      viaTitle.focus({ preventScroll: true });
      return;
    }
    const onBoard = document.querySelector<HTMLElement>(`[data-card="${CSS.escape(shown)}"]`);
    if (!onBoard || onBoard.tabIndex < 0) return;
    event.preventDefault();
    onBoard.focus();
  }
</script>

<!-- Open, the keys that keep a version of the body save no page anywhere in it. -->
<svelte:window
  onkeydown={(event: KeyboardEvent) => {
    if (cardId !== undefined) holdSaveKey(event);
  }}
/>

<Dialog.Root
  bind:open={
    () => cardId !== undefined,
    (open) => {
      if (!open) close();
    }
  }
>
  <Dialog.Content
    bind:ref={content}
    showCloseButton={false}
    data-testid="card-dialog"
    data-card-dialog
    data-frame={frame}
    class={['@container/card flex flex-col gap-0 overflow-hidden p-0', FRAME_CLASS[frame]]}
    onOpenAutoFocus={focusOnOpen}
    onCloseAutoFocus={focusOnClose}
  >
    <Dialog.Title class="sr-only">{title}</Dialog.Title>
    <Dialog.Description class="sr-only">
      {m.card_location({ board: board.board?.title ?? '', view: view?.title ?? '' })}
    </Dialog.Description>
    {#if card}
      {#key card}
        <CardContent
          {board}
          {view}
          {card}
          {frame}
          {cardHref}
          onClose={close}
          onOpenCard={(otherId: string) => void openOther(otherId)}
          {headerActions}
        />
      {/key}
    {/if}
  </Dialog.Content>
</Dialog.Root>
