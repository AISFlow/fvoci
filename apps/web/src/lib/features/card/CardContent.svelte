<!--
  What is inside the frame of an open card, top to bottom: the top row, then
  one box that scrolls (title, properties, body, attachments, comments), and
  the field for a new comment as the last row, always in sight.

  The component is made anew for another card and for nothing else: not for
  a change of the frame, and not for anything that arrives from the board.

  The title, icon and properties come from the board's list of cards and are
  there at once; the body, attachments and comments are read when the card is
  opened, while the body's room and the editor are asked for. A card that is
  not there (the address is wrong, or somebody deleted it, also while it was
  open) says so and can be closed.
-->
<script lang="ts">
  import { untrack, type Snippet } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CardFrame } from '$lib/layout/presentation.ts';
  import { ancestorsOf } from '$lib/logic/card-hierarchy.ts';
  import type { BoardView } from '$lib/logic/wire.ts';
  import {
    whileHere,
    type BoardStore,
    type CardStore,
    type CommentVersion,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { bodyReferences } from './body-references.ts';
  import BodyVersions from './BodyVersions.svelte';
  import { cardVersions, keepVersion, VersionsReading } from './body-versions.svelte.ts';
  import CardAttachments from './CardAttachments.svelte';
  import CardBacklinks from './CardBacklinks.svelte';
  import CardBody from './CardBody.svelte';
  import CardComments from './CardComments.svelte';
  import CardHeader from './CardHeader.svelte';
  import CardParentBar from './CardParentBar.svelte';
  import CardProperties from './CardProperties.svelte';
  import CardTitle from './CardTitle.svelte';
  import CommentComposer from './CommentComposer.svelte';
  import SubcardsSection from './SubcardsSection.svelte';
  import { editorChunk } from './editor-chunk.svelte.ts';
  import { saveStateOf } from './save-state.ts';
  import { keepScroll } from './scroll-keeper.ts';

  interface Props {
    board: BoardStore;
    view: BoardView | undefined;
    card: CardStore;
    frame: CardFrame;
    /** The address of a card of the board: its link to copy. */
    cardHref: (cardId: string) => string;
    onClose: () => void;
    onOpenCard?: (cardId: string) => void;
    /** Extra actions in the card header (e.g. personal timer), supplied by the app shell. */
    headerActions?: Snippet<[CardStore]> | undefined;
  }

  let { board, view, card, frame, cardHref, onClose, onOpenCard, headerActions }: Props = $props();

  const references = $derived(bodyReferences(board, card.cardId));

  // The body's room is asked for with the card's reading (`CardStore.load`): the
  // editor that shows its document is asked for with them, not once the card is read.
  $effect(() => {
    if (card.room) editorChunk.load();
  });

  const summary = $derived(card.summary);
  const hasParent = $derived(
    summary !== undefined &&
      !summary.fields.isTemplate &&
      ancestorsOf(summary.id, board.cardsById).length > 0,
  );
  const ready = $derived(card.status === 'ready');
  /** A card that starts without a title and a body was just made: the caret goes to its title. */
  const blank = untrack(() => card.summary?.title === '' && !card.summary.hasBody);

  let scroller = $state<HTMLElement>();
  /** The reader is deleting the card: it closes as soon as it is gone. */
  let deleting = $state(false);
  /** The versions of the body, read anew each time their dialog opens. */
  let versions = $state.raw<VersionsReading>();
  let versionsOpen = $state(false);

  function showVersions(): void {
    versions = new VersionsReading(cardVersions(board, card));
    versionsOpen = true;
  }

  // What these two need is taken before they wait: by the time a write has
  // answered, this component may be gone (the card was deleted, or changed
  // for another), and its arguments with it.
  async function duplicate(): Promise<void> {
    const [store, cardId, open] = [board, card.cardId, onOpenCard];
    // What was typed goes to the room first. The copy is made from the card's body as stored,
    // which the room writes a little later: the last seconds of typing may be missing from it
    // (a gap of the server, recorded in e2e-live/body.spec.ts).
    await card.settleBody();
    const copy = await whileHere(store, () => store.duplicateCard(cardId));
    if (copy) open?.(copy.id);
  }

  async function remove(): Promise<void> {
    const [store, cardId, close] = [board, card.cardId, onClose];
    deleting = true;
    // What was typed goes to the room first. Whether the card that undo brings back has
    // its last seconds depends on the server writing the room's body first, as for a copy.
    await card.settleBody();
    await store.deleteCards([cardId]);
    // Refused: the card is still there, and stays open.
    if (store.card(cardId)) deleting = false;
    else close();
  }

  /** What a comment of the card said before, with its writers known by name. */
  async function readVersions(commentId: string): Promise<CommentVersion[]> {
    const workspace = board.workspace;
    const list = await workspace.backend.commentVersions(board.boardId, commentId, card.cardId);
    await workspace.learnUsers(list.map((version) => version.writtenBy).filter((id) => id !== ''));
    return list;
  }

  function showNewest(): void {
    requestAnimationFrame(() => {
      scroller?.scrollTo({ top: scroller.scrollHeight });
    });
  }
</script>

<CardHeader
  {board}
  {view}
  {card}
  {frame}
  {cardHref}
  saveState={saveStateOf(card)}
  {onClose}
  onDuplicate={() => void duplicate()}
  onVersions={showVersions}
  onDelete={() => void remove()}
  extraActions={headerActions}
/>
<BodyVersions bind:open={versionsOpen} reading={versions} container={board} />

{#if deleting}
  <div class="flex-1"></div>
{:else if card.missing || (card.status === 'failed' && !summary)}
  <div class="grid flex-1 place-items-center p-6 text-center" data-testid="card-missing">
    <div class="flex max-w-sm flex-col items-center gap-2">
      <p class="text-lg font-medium" role="alert">
        {card.missing ? m['CardDialog.nocard']() : m.app_problem_load()}
      </p>
      {#if card.missing}
        <p class="text-muted-foreground">{m.card_missing_hint()}</p>
      {/if}
      <Button class="mt-2 roomy:min-h-touch" onclick={onClose}>{m.ui_close()}</Button>
    </div>
  </div>
{:else if !summary}
  <div class="grid flex-1 place-items-center p-6">
    <p class="text-muted-foreground" role="status">{m.card_loading()}</p>
  </div>
{:else}
  <div
    bind:this={scroller}
    class="min-h-0 flex-1 overflow-y-auto overscroll-contain webkit:[overflow-anchor:none]"
    data-testid="card-scroll"
    {@attach keepScroll}
  >
    <article
      class="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 pt-2 pb-8 @card-roomy/card:px-8"
    >
      {#if summary.fields.isTemplate}
        <p class="rounded-md bg-muted px-3 py-2 text-muted-foreground">
          {m['CardDialog.editing-template']()}
        </p>
      {:else if hasParent}
        <CardParentBar {board} card={summary} {onOpenCard} />
      {/if}
      <CardTitle {board} card={summary} autofocus={blank}>
        {#snippet meta()}
          {#if !summary.fields.isTemplate && !hasParent}
            <CardParentBar {board} card={summary} {onOpenCard} />
          {/if}
        {/snippet}
      </CardTitle>
      <CardProperties {board} {view} card={summary} {onOpenCard} />
      {#if ready}
        <CardBody
          container={board}
          {card}
          {references}
          keep={() => keepVersion(cardVersions(board, card))}
        />
        {#if !summary.fields.isTemplate}
          <SubcardsSection {board} card={summary} {onOpenCard} />
        {/if}
        <CardBacklinks workspace={board.workspace} blockId={card.cardId} />
        <CardAttachments {board} {card} />
        <CardComments {board} {card} {readVersions} />
      {:else if card.status === 'failed'}
        <p class="border-t pt-5 text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      {:else}
        <p class="border-t pt-5 text-muted-foreground" role="status">{m.card_loading()}</p>
      {/if}
    </article>
  </div>
  {#if ready && board.canComment}
    <CommentComposer {board} {card} draftKey={card.cardId} onSent={showNewest} />
  {/if}
{/if}
