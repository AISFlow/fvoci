<!--
  One thing the search found, as an option of the list: what it is (its
  icon), a card's number, its title (of a diary entry, its day), the board,
  wiki, diary or course it is in, and the piece of its body around what
  matched, the matched stretches marked.

  Everything is drawn as text: the snippet is the reader's own writing, and a
  `<script>` in it is the letters of one. It is a link, so that a middle click
  or ⌘/Ctrl and a click open it in a new tab as any link does; a plain click
  goes there in this one (`onGo`). The focus stays in the field of the
  search: the row is chosen by `active`, not focused.
-->
<script lang="ts">
  import FileTextIcon from '@lucide/svelte/icons/file-text';
  import GraduationCapIcon from '@lucide/svelte/icons/graduation-cap';
  import NotebookPenIcon from '@lucide/svelte/icons/notebook-pen';
  import StickyNoteIcon from '@lucide/svelte/icons/sticky-note';

  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { parseCalendarDate } from '$lib/logic/dates.ts';
  import { displayDayWithYear } from '$lib/logic/format.ts';
  import type { SearchHit } from '$lib/state/content-search.ts';

  interface Props {
    hit: SearchHit;
    id: string;
    href: string;
    /** The name of the board or wiki it is in; empty where it is not known. */
    container: string;
    /** The reader's language, for the day an entry is of. */
    locale: string;
    active: boolean;
    onGo: () => void;
    onPoint: () => void;
  }

  let { hit, id, href, container, locale, active, onGo, onPoint }: Props = $props();

  /** An entry's title is its day (`2026-11-21`), written as the reader reads a day. */
  const day = $derived(hit.kind === 'entry' ? parseCalendarDate(hit.title) : undefined);
  const title = $derived(
    (day && displayDayWithYear(day, locale)) ||
      hit.title ||
      (hit.kind === 'page'
        ? m.search_untitled_page()
        : hit.kind === 'unit'
          ? m.search_untitled_unit()
          : m['KanbanCard.untitled']()),
  );

  function onclick(event: MouseEvent): void {
    // A click with a key that opens a new tab is the browser's.
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onGo();
  }

  /** The field keeps the focus: a click that opens a new tab leaves the search open, its keys with it. */
  function keepFocus(event: MouseEvent): void {
    event.preventDefault();
  }
</script>

<a
  {id}
  {href}
  role="option"
  tabindex="-1"
  aria-selected={active}
  data-search-hit={hit.id}
  data-kind={hit.kind}
  class={[
    'flex min-h-10 items-start gap-2.5 rounded-md px-2 py-1.5 text-sm outline-none roomy:min-h-touch',
    active && 'bg-accent text-accent-foreground',
  ]}
  {onclick}
  onmousedown={keepFocus}
  onpointermove={onPoint}
>
  {#if hit.kind === 'page'}
    <FileTextIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  {:else if hit.kind === 'entry'}
    <NotebookPenIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  {:else if hit.kind === 'unit'}
    <GraduationCapIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  {:else}
    <StickyNoteIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  {/if}
  <span class="grid min-w-0 flex-1 gap-0.5">
    <span class="flex min-w-0 items-baseline gap-2">
      <span class="sr-only"
        >{hit.kind === 'page'
          ? m.search_hit_page()
          : hit.kind === 'unit'
            ? m.search_hit_unit()
            : hit.kind === 'entry'
              ? m.search_hit_entry()
              : m.search_hit_card()}</span
      >
      <CardNumber number={hit.number} class="self-center text-xs" />
      <span class="min-w-0 truncate font-medium" data-hit-title>{title}</span>
      {#if container}
        <span class="min-w-0 shrink-[2] truncate text-xs text-muted-foreground" data-hit-container>
          {container}
        </span>
      {/if}
    </span>
    {#if hit.snippet.length > 0}
      <span class="line-clamp-2 text-xs break-words text-muted-foreground" data-hit-snippet
        >{#each hit.snippet as part, index (index)}{#if part.matched}<mark
              class="rounded-sm bg-primary/15 font-semibold text-foreground">{part.text}</mark
            >{:else}{part.text}{/if}{/each}</span
      >
    {/if}
  </span>
</a>
