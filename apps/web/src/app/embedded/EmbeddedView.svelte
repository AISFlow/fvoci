<!--
  A view of a board shown in a body: the board view of a card's, a page's or
  a unit's body (board-view.ts draws its place; BoardViewHost.svelte mounts
  this into it).

  Nothing is read before it comes into sight (or near it): then what the view
  is is asked as a reference's title is (`referenceTitles`), for the reader
  and as things are now. A view the reader may not see, one deleted, one of
  another workspace, and one emptied for a reader by link all show the same
  one line, which tells none of them from the others. A view the reader sees
  is drawn from its board's store (EmbeddedBoardView.svelte), which is held
  while it is shown.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BoardView } from '$lib/logic/wire.ts';
  import { referenceTitles, type LinkedBlock, type Workspace } from '$lib/state/index.ts';

  import EmbeddedBoardView from './EmbeddedBoardView.svelte';
  import './embedded-view.css';
  import type { Router } from '../router.svelte.ts';

  /** Given once, when the view is drawn into its place: another view is drawn anew. */
  interface Props {
    workspace: Workspace;
    router: Router;
    viewId: string;
    /** Opens a card of the view's board over a page that is not the board's. */
    onOpenCard: (boardId: string, view: BoardView, cardId: string) => void;
  }

  let { workspace, router, viewId, onOpenCard }: Props = $props();

  // Taken here, not where it is read: what is made while a derived runs is not followed by it.
  const titles = untrack(() => referenceTitles(workspace));

  /** Whether it has come near enough to be seen: nothing is read before. */
  let near = $state(false);

  /** Watches for it to come within a screen of being seen, once. */
  function watchSight(element: HTMLElement): (() => void) | undefined {
    if (typeof IntersectionObserver === 'undefined') {
      near = true;
      return undefined;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        near = true;
        observer.disconnect();
      },
      { rootMargin: '100% 0px' },
    );
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }

  /** The view as the reader is told of it: `undefined` while it is asked, `null` if it is none they see. */
  const linked = $derived.by<LinkedBlock | null | undefined>(() => {
    if (!near) return undefined;
    const told = titles.of(viewId);
    if (told === undefined) return undefined;
    return told?.kind === 'view' ? told : null;
  });
</script>

<section class="embedded-view" data-embedded-view={viewId} {@attach watchSight}>
  {#if linked === null}
    <p class="doc-board-view-unseen" data-testid="board-view-unseen">{m.board_view_unseen()}</p>
  {:else if linked === undefined}
    <p class="embedded-view-said" role="status">{m.board_view_loading()}</p>
  {:else}
    <!-- Another board is another store to hold. -->
    {#key linked.containerId}
      <EmbeddedBoardView {workspace} {router} {linked} {onOpenCard} />
    {/key}
  {/if}
</section>
