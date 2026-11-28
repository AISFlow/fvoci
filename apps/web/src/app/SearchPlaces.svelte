<!--
  A section of places at the head of the search, as "바로 이동" is
  (`SearchJumps`): the reader's stars, or what they opened last on this
  browser. Each is a link (a middle click or ⌘/Ctrl and a click open it in a
  new tab; a plain click goes there in this one), not an option of the list of
  what was found: Tab reaches them from the field.
-->
<script lang="ts">
  import FavoriteIcon from '$lib/features/shell/FavoriteIcon.svelte';
  import { favoriteTitle } from '$lib/features/shell/shown-title.ts';
  import type { Favorite } from '$lib/state/index.ts';

  interface Props {
    heading: string;
    testId: string;
    places: readonly Favorite[];
    locale: string;
    hrefOf: (place: Favorite) => string;
    onGo: (place: Favorite) => void;
  }

  let { heading, testId, places, locale, hrefOf, onGo }: Props = $props();

  const uid = $props.id();

  function follow(event: MouseEvent, place: Favorite): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onGo(place);
  }
</script>

<section class="grid gap-0.5 pb-2" aria-labelledby="{uid}-title" data-testid={testId}>
  <h3 id="{uid}-title" class="px-2 pb-0.5 text-xs font-medium text-muted-foreground">
    {heading}
  </h3>
  <ul class="grid gap-0.5">
    {#each places as place (place.targetId)}
      {@const title = favoriteTitle(place)}
      <li>
        <a
          href={hrefOf(place)}
          class="flex min-h-9 items-center gap-2.5 rounded-md px-2 text-sm outline-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground focus-visible:ring-3 focus-visible:ring-ring/50 roomy:min-h-touch"
          data-search-place={place.targetId}
          onclick={(event) => {
            follow(event, place);
          }}
        >
          <FavoriteIcon
            kind={place.kind}
            icon={place.icon}
            {title}
            containerKind={place.containerKind}
            {locale}
          />
          <span class="min-w-0 truncate">{title}</span>
        </a>
      </li>
    {/each}
  </ul>
</section>
