<!--
  The ★ of a board, a card, a page or a unit, as the reader's stars answer it:
  not starred, starred, and a press that switches it; nothing until the stars
  are read, and the titles alone.
-->
<script lang="ts">
  import FavoriteToggle from '$lib/components/favorite-toggle/FavoriteToggle.svelte';
  import type { ReaderStars } from '$lib/components/favorite-toggle/reader-stars.ts';
  import { SvelteSet } from 'svelte/reactivity';

  let { unread = false }: { unread?: boolean } = $props();

  const starred = new SvelteSet<string>(['starred']);

  const stars: ReaderStars = {
    scope: 'story/reader',
    has: (targetId) => (unread ? undefined : starred.has(targetId)),
    read: () => Promise.resolve(),
    toggle: (targetId) => {
      if (starred.has(targetId)) starred.delete(targetId);
      else starred.add(targetId);
      return Promise.resolve();
    },
  };
</script>

<!-- Each ★ beside the title it stands at, as at the top bar: with none, the title alone. -->
<div class="flex flex-col gap-2" data-testid="favorite-samples">
  <div class="flex items-center gap-1.5">
    <span class="text-base font-semibold">Road map · 로드맵</span>
    <FavoriteToggle {stars} targetId="plain" />
  </div>
  <div class="flex items-center gap-1.5">
    <span class="text-base font-semibold">Release checklist · 출시 점검</span>
    <FavoriteToggle {stars} targetId="starred" />
  </div>
</div>
