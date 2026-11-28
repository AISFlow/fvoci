<!--
  "즐겨찾기" at the head of the sidebar: the reader's stars in the workspace,
  in their order (`WorkspaceFavorites`), a link to each. Folded and unfolded
  by hand, as the other folds are; not there while the reader has none, nor
  where none are kept. A star is dragged to another place among them, with a
  pointer or the keyboard, as the boards of a category are (`Sortable`, of its
  own scope inside the sidebar's).
-->
<script lang="ts">
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import { untrack } from 'svelte';

  import { Sortable } from '$lib/drag/sortable.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { favoriteOrderAfter } from '$lib/logic/favorite-order.ts';
  import { workspaceFavorites, type Favorite, type Workspace } from '$lib/state/index.ts';

  import { containerDragMessages } from './drag-messages.ts';
  import FavoriteIcon from './FavoriteIcon.svelte';
  import type { FavoritePlaces } from './favorite-places.ts';
  import { favoriteTitle } from './shown-title.ts';

  interface Props {
    workspace: Workspace;
    /** The container shown. */
    containerId: string | undefined;
    places: FavoritePlaces;
    /** The words that say how to drag with the keyboard. */
    describedBy: string;
  }

  let { workspace, containerId, places, describedBy }: Props = $props();

  const LIST = 'favorites';
  const SCOPE = 'favorites';

  const favorites = $derived(workspaceFavorites(workspace));
  let folded = $state(false);

  // Read for the reader and workspace of now, and again as they change (from none, or another).
  $effect(() => {
    const shown = favorites;
    if (shown.scope === undefined) return;
    untrack(() => void shown.read());
  });

  /** A container's title as the sidebar knows it now, else as the stars were read. */
  function titleOf(favorite: Favorite): string {
    const container =
      favorite.kind === 'container'
        ? workspace.containers.find((one) => one.id === favorite.targetId)
        : undefined;
    return favoriteTitle(container ? { ...favorite, title: container.title } : favorite);
  }

  const ids = $derived(favorites.list.map((favorite) => favorite.targetId));

  const sortable = new Sortable({
    scope: SCOPE,
    order: () => (folded ? [] : [{ id: LIST, items: ids }]),
    onDrop: ({ itemId, to }) => {
      const order = favoriteOrderAfter(ids, itemId, to);
      if (order) void favorites.reorder(order);
    },
    messages: containerDragMessages(),
    itemLabel: (id) => {
      const favorite = favorites.list.find((one) => one.targetId === id);
      return favorite ? titleOf(favorite) : id;
    },
    listLabel: () => m.favorites_section(),
  });

  const drag = $derived(sortable.active);
  const lifted = $derived(
    drag ? favorites.list.find((favorite) => favorite.targetId === drag.itemId) : undefined,
  );

  type Row = { kind: 'favorite'; favorite: Favorite } | { kind: 'gap' };

  const rows = $derived.by((): Row[] => {
    const gapAt = drag?.gap?.listId === LIST ? drag.gap.index : -1;
    const shown: Row[] = [];
    let staying = 0;
    for (const favorite of favorites.list) {
      if (!drag?.itemIds.includes(favorite.targetId)) {
        if (staying === gapAt) shown.push({ kind: 'gap' });
        staying += 1;
      }
      shown.push({ kind: 'favorite', favorite });
    }
    if (staying === gapAt) shown.push({ kind: 'gap' });
    return shown;
  });

  function follow(event: MouseEvent, favorite: Favorite): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    places.open(favorite);
  }
</script>

{#if favorites.list.length > 0}
  <div class="pt-2" data-testid="sidebar-favorites" {@attach sortable.attach}>
    <button
      type="button"
      aria-expanded={!folded}
      class="flex min-h-8 w-full items-center gap-1 rounded-md py-1 pl-1 text-left text-xs font-medium text-muted-foreground hover:bg-sidebar-accent roomy:min-h-touch"
      onclick={() => {
        folded = !folded;
      }}
    >
      <ChevronRightIcon
        class={['size-4 shrink-0 transition-transform', !folded && 'rotate-90']}
        aria-hidden="true"
      />
      <span class="min-w-0 flex-1 truncate">{m.favorites_section()}</span>
    </button>
    {#if !folded}
      <ul data-drag-list={LIST} data-drag-scope={SCOPE} aria-label={m.favorites_section()}>
        {#each rows as row (row.kind === 'gap' ? ' gap' : row.favorite.targetId)}
          {#if row.kind === 'favorite'}
            {@const favorite = row.favorite}
            {@const title = titleOf(favorite)}
            <li class={[drag?.itemIds.includes(favorite.targetId) && 'hidden']}>
              <a
                href={places.href(favorite)}
                draggable="false"
                data-drag-item={favorite.targetId}
                data-drag-scope={SCOPE}
                data-favorite={favorite.targetId}
                aria-current={favorite.kind === 'container' && favorite.targetId === containerId
                  ? 'page'
                  : undefined}
                aria-describedby={describedBy}
                class="flex min-h-8 items-center gap-2 rounded-md py-1 pr-2 pl-2 select-none [-webkit-touch-callout:none] hover:bg-sidebar-accent aria-[current=page]:bg-sidebar-accent aria-[current=page]:font-medium roomy:min-h-touch"
                onclick={(event: MouseEvent) => {
                  follow(event, favorite);
                }}
              >
                <FavoriteIcon
                  kind={favorite.kind}
                  icon={favorite.icon}
                  {title}
                  containerKind={favorite.containerKind}
                  locale={workspace.locale}
                />
                <span class="min-w-0 flex-1 truncate">{title}</span>
              </a>
            </li>
          {:else if drag?.mode === 'keyboard' && lifted}
            <li>
              <div
                data-drag-carried={lifted.targetId}
                data-drag-scope={SCOPE}
                role="button"
                tabindex="0"
                aria-describedby={describedBy}
                class="flex min-h-8 items-center gap-2 rounded-md bg-sidebar-accent py-1 pr-2 pl-2 font-medium ring-2 ring-ring outline-none roomy:min-h-touch"
              >
                <span class="min-w-0 flex-1 truncate">{titleOf(lifted)}</span>
              </div>
            </li>
          {:else}
            <li
              data-drag-gap
              aria-hidden="true"
              class="rounded-md border-2 border-dashed border-input-border"
              style:height="{drag?.height ?? 0}px"
            ></li>
          {/if}
        {/each}
      </ul>
    {/if}
    <div class="sr-only" aria-live="assertive" aria-atomic="true">{sortable.announcement}</div>
  </div>
{/if}

<!-- The star a pointer drags: it follows the pointer, over everything, and takes no presses. -->
{#if drag?.mode === 'pointer' && sortable.overlay && lifted}
  <div
    aria-hidden="true"
    class="pointer-events-none fixed top-0 left-0 z-[70] flex items-center gap-2 rounded-md bg-sidebar-accent py-1 pr-2 pl-2 font-medium text-sidebar-accent-foreground shadow-lg ring-2 ring-ring will-change-transform"
    style:width="{drag.width}px"
    style:height="{drag.height}px"
    style:translate="{sortable.overlay.x}px {sortable.overlay.y}px"
  >
    <span class="min-w-0 flex-1 truncate">{titleOf(lifted)}</span>
  </div>
{/if}
