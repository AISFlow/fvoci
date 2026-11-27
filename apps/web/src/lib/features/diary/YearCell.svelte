<!--
  One year of a diary's page: the day of that year and what was written on it.

  Only the cell that is open has an editor, and with it the entry's room: the
  others show the first lines of their entry, its images small, as it was
  read (`BodyView`), and open when pressed. One open at a time keeps the page
  to one room, however many years it has.

  A day nobody wrote on says so ("Not written") and opens to be written; the
  day of this year that is still to come says that, and opens to nothing.
  An open day has its mood, weather and place to mark above its body
  (`DayNotes`); a folded one shows them under its year.
  The open day's body, editor and all, is the page's (`body`): a feature
  does not reach into another's.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  import HistoryIcon from '@lucide/svelte/icons/history';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import BodyView from '$lib/editor/BodyView.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import { writable } from '$lib/logic/diary-days.ts';
  import { displayDayWithYear, displayWeekday, displayYear } from '$lib/logic/format.ts';
  import type { DiaryStore, EntryStore } from '$lib/state/index.ts';

  import DayNotes from './DayNotes.svelte';
  import NotesShown from './NotesShown.svelte';

  interface Props {
    diary: DiaryStore;
    date: CalendarDate;
    today: CalendarDate;
    open: boolean;
    locale: string;
    onOpen: () => void;
    /** Opens the versions kept of the open entry's body. */
    onVersions?: () => void;
    /** The open day's body, with its editor; `label` is the day written whole. */
    body: Snippet<[entry: EntryStore, label: string]>;
  }

  let { diary, date, today, open, locale, onOpen, onVersions, body }: Props = $props();

  const uid = $props.id();
  const entry = $derived(diary.entries.find((one) => one.date === date));
  const toCome = $derived(!writable(date, today));
  const whole = $derived(displayDayWithYear(date, locale));
</script>

<section
  aria-labelledby="{uid}-year"
  class="flex flex-col gap-2 py-4"
  data-testid="diary-year"
  data-date={date}
  data-open={open && (!toCome || entry) ? '' : undefined}
  data-written={entry ? '' : undefined}
>
  <div class="flex items-center justify-between gap-2">
    <h2 id="{uid}-year" class="flex items-baseline gap-2">
      <span class="text-base font-semibold">{displayYear(date, locale)}</span>
      <span class="text-sm text-muted-foreground">{displayWeekday(date, locale)}</span>
      <span class="sr-only">{whole}</span>
    </h2>
    {#if open && entry && onVersions}
      <IconButton label={m.card_versions()} data-testid="entry-versions" onclick={onVersions}>
        <HistoryIcon aria-hidden="true" />
      </IconButton>
    {/if}
  </div>
  {#if entry && !open}
    <NotesShown notes={entry} />
  {/if}
  <!-- A day still to come here may be written already, on a device where it has come. -->
  {#if toCome && !entry}
    <p class="text-sm text-muted-foreground" data-testid="entry-to-come">{m.diary_not_yet()}</p>
  {:else if open}
    {@const day = diary.day(date)}
    <DayNotes entry={day} label={whole} />
    {@render body(day, whole)}
  {:else if entry}
    <div class="relative">
      <div
        class="max-h-28 overflow-hidden mask-b-from-60% [&_img]:max-h-20 [&_img]:w-auto"
        data-testid="entry-preview"
      >
        <BodyView doc={entry.body} zoomImages={false} class="text-base">
          {#snippet empty()}
            <p class="text-muted-foreground">{m.diary_entry_empty()}</p>
          {/snippet}
          {#snippet invalid()}
            <p class="text-muted-foreground">{m.diary_body_invalid()}</p>
          {/snippet}
        </BodyView>
      </div>
      <button
        type="button"
        class="absolute inset-0 rounded-md outline-none hover:bg-accent/30 focus-visible:ring-[3px] focus-visible:ring-ring/50"
        aria-label={m.diary_open_entry({ date: whole })}
        data-testid="entry-open"
        onclick={onOpen}
      ></button>
    </div>
  {:else}
    <button
      type="button"
      class="flex min-h-8 items-center self-start rounded-md px-2 text-sm text-muted-foreground hover:bg-accent roomy:min-h-touch"
      aria-label={m.diary_write_day({ date: whole })}
      data-testid="entry-blank"
      onclick={onOpen}
    >
      {m.diary_not_written()}
    </button>
  {/if}
</section>
