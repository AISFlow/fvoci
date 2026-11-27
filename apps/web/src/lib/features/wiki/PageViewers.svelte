<!--
  Who else has a page of the wiki open now: their faces, two at most and how
  many more, with their names for a screen reader. Nothing while nobody else
  has it. Small on a row of the tree, larger at the head of the page.
-->
<script lang="ts">
  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import type { PresenceSource } from '$lib/state/index.ts';

  import { nameOf, viewersLabel, viewersOf } from './page-viewers.ts';

  let {
    wiki,
    pageId,
    of = 'page',
    large = false,
    id,
    class: className,
  }: {
    wiki: PresenceSource;
    pageId: string;
    of?: 'page' | 'unit';
    large?: boolean;
    id?: string;
    class?: string;
  } = $props();

  const people = $derived(viewersOf(wiki, pageId));
  const label = $derived(viewersLabel(wiki, people, of));
</script>

{#if people.length > 0}
  <span
    data-testid="page-viewers"
    {id}
    role="img"
    aria-label={label}
    title={label}
    class={['flex shrink-0 items-center gap-1 text-xs text-muted-foreground', className]}
  >
    <span class="flex">
      {#each people.slice(0, 2) as userId, index (userId)}
        <Avatar
          name={nameOf(wiki, userId)}
          colorKey={userId}
          locale={wiki.workspace.locale}
          decorative
          class={[large ? 'size-7' : 'size-4', 'ring-1 ring-background', index > 0 && '-ms-1']}
        />
      {/each}
    </span>
    {#if people.length > 2}<span aria-hidden="true">+{people.length - 2}</span>{/if}
  </span>
{/if}
