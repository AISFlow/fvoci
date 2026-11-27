<!--
  The button the outline of a page is behind when it is not beside the page,
  for a bar that stays at the head of the page and covers none of it.

  Where the page's column has room for the outline (the reader folded it
  away), the button puts it back beside the page, and the focus with it.
  Where it has not, the button opens the outline as a list below it, over the
  page: choosing a heading closes it, the focus going to the heading (or back
  to the button where it did not: an editor under a finger); so do Escape, a
  press or the focus elsewhere, and "back".
-->
<script lang="ts">
  import TableOfContentsIcon from '@lucide/svelte/icons/table-of-contents';
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { Button } from '$lib/ui/button/index.js';

  import OutlineNav from './OutlineNav.svelte';
  import type { PageOutline } from './page-outline.svelte.ts';

  interface Props {
    outline: PageOutline;
    /**
     * The identifier of the outline beside the page, which takes the focus when it is put
     * back; the button's is it with `-button`, and the list's with `-list`.
     */
    id: string;
  }

  let { outline, id }: Props = $props();

  /** Read only by handlers: nothing is drawn from it. */
  let box: HTMLElement | undefined;
  // State, as the button's `ref` it is bound to is.
  let opener = $state<HTMLElement | null>(null);

  closeOnBack(
    () => outline.open,
    () => {
      outline.open = false;
    },
  );

  async function press(): Promise<void> {
    if (outline.room) {
      outline.setFolded(false);
      await tick();
      document.getElementById(id)?.querySelector<HTMLElement>('button')?.focus();
      return;
    }
    outline.open = !outline.open;
    if (!outline.open) return;
    await tick();
    const list = box?.querySelector('[data-testid="page-outline"]');
    (
      list?.querySelector<HTMLElement>('[aria-current]') ??
      list?.querySelector<HTMLElement>('button')
    )?.focus();
  }

  function close(): void {
    outline.open = false;
    opener?.focus();
  }
</script>

<svelte:document
  onpointerdown={(event: PointerEvent) => {
    if (outline.open && event.target instanceof Node && !box?.contains(event.target)) {
      outline.open = false;
    }
  }}
/>

<div
  bind:this={box}
  class="relative"
  onkeydown={(event: KeyboardEvent) => {
    if (event.key === 'Escape' && outline.open) {
      event.preventDefault();
      close();
    }
  }}
  onfocusout={(event: FocusEvent) => {
    const next = event.relatedTarget;
    if (outline.open && next instanceof Node && !box?.contains(next)) outline.open = false;
  }}
  role="presentation"
>
  <Button
    bind:ref={opener}
    id="{id}-button"
    variant="ghost"
    size="sm"
    class="text-muted-foreground roomy:min-h-touch"
    data-testid="open-page-outline"
    aria-label={outline.room ? m.wiki_outline_unfold() : undefined}
    aria-expanded={outline.room ? false : outline.open}
    aria-controls={outline.open ? `${id}-list` : undefined}
    onclick={() => void press()}
  >
    <TableOfContentsIcon aria-hidden="true" />
    {m.wiki_outline()}
  </Button>
  {#if outline.open}
    <OutlineNav
      {outline}
      id="{id}-list"
      class="absolute top-full right-0 z-30 mt-1 max-h-[min(60dvh,32rem)] w-[min(20rem,calc(100vw-1rem))] rounded-lg bg-popover p-1 text-popover-foreground shadow-md ring-1 ring-foreground/10"
      onChose={(focused: boolean) => {
        if (!focused) opener?.focus();
      }}
    />
  {/if}
</div>
