<!--
  What names a card or a page of a wiki: the cards and pages whose body, or a
  comment on which, has a reference to it, as far as the reader may see them,
  the oldest first. What names it from where the reader does not read is no
  part of the answer, and nothing says how much of that there is.

  Read when it is opened, a page at a time. While nothing names it this takes
  no room at all: no heading, no empty list. A card that is renamed meanwhile
  shows its new title, and one that goes leaves the list (`referenceTitles`).
-->
<script lang="ts">
  import { onMount } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { referenceTitles, type LinkedBlock, type Workspace } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    workspace: Workspace;
    /** The card, the page of a wiki, or the unit of a course that is named. */
    blockId: string;
    /** Which it is, for the heading. */
    of?: 'card' | 'page' | 'unit';
    /** The level of the heading: under a card's title, or under a page's or unit's. */
    heading?: 'h2' | 'h3';
  }

  let { workspace, blockId, of = 'card', heading = 'h3' }: Props = $props();

  /** How many are read at a time (the server's own page). */
  const AT_ONCE = 50;

  const uid = $props.id();
  const titles = $derived(referenceTitles(workspace));

  let read = $state.raw<LinkedBlock[]>([]);
  /** The last page was full: there may be more. */
  let more = $state(false);
  let reading = $state(false);
  let pages = 0;
  let here = true;

  async function readOn(): Promise<void> {
    if (reading) return;
    reading = true;
    try {
      const found = await titles.to(blockId, { page: pages, perPage: AT_ONCE });
      if (!here) return;
      pages += 1;
      const known = new Set(read.map((linked) => linked.id));
      read = [...read, ...found.filter((linked) => !known.has(linked.id))];
      more = found.length === AT_ONCE;
    } catch {
      // The card is shown without what names it; opening it again asks again.
      more = false;
    } finally {
      reading = false;
    }
  }

  onMount(() => {
    void readOn();
    return () => {
      here = false;
    };
  });

  /** What is listed, as it is called now; what the reader may see no longer is left out. */
  const naming = $derived(
    read.flatMap((linked) => {
      const now = titles.of(linked.id);
      return now === null ? [] : [now ?? linked];
    }),
  );

  function open(event: MouseEvent, linked: LinkedBlock): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    titles.places?.open(linked);
  }
</script>

{#if naming.length > 0}
  <section
    aria-labelledby="{uid}-heading"
    class="flex flex-col gap-2 border-t pt-5"
    data-testid="card-backlinks"
  >
    <svelte:element this={heading} id="{uid}-heading" class="font-semibold">
      {of === 'page' ? m.page_backlinks() : of === 'unit' ? m.unit_backlinks() : m.card_backlinks()}
      <span class="font-normal text-muted-foreground">{naming.length}{more ? '+' : ''}</span>
    </svelte:element>
    <ul class="flex flex-col gap-1">
      {#each naming as linked (linked.id)}
        {@const href = titles.places?.href(linked)}
        {@const name = linked.title || m.editor_reference_untitled()}
        <li class="min-w-0" data-backlink={linked.id}>
          {#if href}
            <a
              {href}
              onclick={(event) => {
                open(event, linked);
              }}
              class="block truncate rounded-md px-2 py-1 underline-offset-2 hover:bg-accent hover:underline roomy:py-2"
            >
              {#if linked.icon}<span aria-hidden="true">{linked.icon} </span>{/if}{name}
            </a>
          {:else}
            <span class="block truncate px-2 py-1 roomy:py-2">
              {#if linked.icon}<span aria-hidden="true">{linked.icon} </span>{/if}{name}
            </span>
          {/if}
        </li>
      {/each}
    </ul>
    {#if more}
      <Button
        variant="ghost"
        class="self-start roomy:min-h-touch"
        aria-disabled={reading}
        onclick={() => void readOn()}
      >
        {m.card_backlinks_more()}
      </Button>
    {/if}
  </section>
{/if}
