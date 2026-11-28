<!--
  The ★ of a board, a wiki, a course, a card, a page or a unit: pressed, it is
  put among the reader's stars, at the head of the sidebar and of the search,
  or taken off (`WorkspaceFavorites`). Not shown until the stars are read, nor
  where none are kept (a public link). A side of 44 pixels, for a finger as
  for a mouse.
-->
<script lang="ts">
  import StarIcon from '@lucide/svelte/icons/star';
  import { untrack } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';

  import type { ReaderStars } from './reader-stars.ts';

  interface Props {
    stars: ReaderStars;
    targetId: string;
  }

  let { stars, targetId }: Props = $props();

  // Read for the reader and workspace of now, and again as they change (from none, or another).
  $effect(() => {
    const shown = stars;
    if (shown.scope === undefined) return;
    untrack(() => void shown.read());
  });

  const starred = $derived(stars.has(targetId));
</script>

{#if starred !== undefined}
  <IconButton
    label={m.favorite_toggle()}
    aria-pressed={starred}
    data-testid="favorite-toggle"
    class={['size-touch', starred ? 'text-primary' : 'text-muted-foreground']}
    onclick={() => void stars.toggle(targetId)}
  >
    <StarIcon aria-hidden="true" fill={starred ? 'currentColor' : 'none'} />
  </IconButton>
{/if}
