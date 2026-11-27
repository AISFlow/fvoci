<!--
  One diary, open on a day of the year (`/w/<workspace>/diaries/<diary>/<MM-DD>`):
  that day of every year, this year's on top and the years before it below,
  nearest first. Which diary and day comes from the address; this keeps the
  store and the address in step.

  A diary's address without a day opens today's, the address put in place
  (no entry of the history). A day that is no day of the year, a diary not
  there for the reader, and a board's identifier at a diary's address show
  the same "not found" as the rest of the app.

  The years go back to the one the diary was made in and to every year
  written on that day, and one more each time "Show <year>" is pressed (or a
  day of an earlier year is picked). One year is open at a time: as the page
  opens, this year's day if it has come (the reader's today, on their device);
  pressing another opens it instead. Midnight changes what today is, never
  which day is open: what is being written stays on its day.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import BodyVersions from '$lib/features/card/BodyVersions.svelte';
  import { entryVersions, VersionsReading } from '$lib/features/card/body-versions.svelte.ts';
  import EntryBody from '$lib/features/card/EntryBody.svelte';
  import DayHeader from '$lib/features/diary/DayHeader.svelte';
  import DiaryNameDialog from '$lib/features/diary/DiaryNameDialog.svelte';
  import { DeviceToday } from '$lib/features/diary/today.svelte.ts';
  import YearCell from '$lib/features/diary/YearCell.svelte';
  import { holdSaveKey } from '$lib/editor/save-key.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { page } from '$lib/layout/room.svelte.ts';
  import { nameWindow } from '$lib/layout/window-title.svelte.ts';
  import { dateOfInstant, type CalendarDate } from '$lib/logic/dates.ts';
  import {
    dateIn,
    inLeapYear,
    monthDayOf,
    pageDays,
    parseMonthDay,
    stepMonthDay,
    writable,
    yearOf,
  } from '$lib/logic/diary-days.ts';
  import { displayDay, displayYear } from '$lib/logic/format.ts';
  import {
    DiaryStore,
    entryTrash,
    whileHere,
    type EntryStore,
    type Trash,
    type Workspace,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import NotFound from './NotFound.svelte';
  import DiaryTrash from './DiaryTrash.svelte';
  import type { Route, Router } from './router.svelte.ts';
  import { takeWriteToday } from './write-today.ts';

  let { workspace, router }: { workspace: Workspace; router: Router } = $props();

  const diaryId = $derived(router.route.kind === 'diary' ? router.route.diaryId : undefined);
  const asked = $derived(router.route.kind === 'diary' ? router.route.monthDay : undefined);
  const monthDay = $derived(asked === undefined ? undefined : parseMonthDay(asked));
  const askedYear = $derived(router.route.kind === 'diary' ? router.route.year : undefined);

  /** The diary of the address, made anew for another one. */
  const diary = $derived(diaryId ? new DiaryStore(workspace, diaryId) : undefined);
  const today = new DeviceToday(() => workspace.regional.timeZone);
  const locale = $derived(workspace.locale);

  // Shown while it is, closed when it goes. The board shown before lets go.
  $effect(() => {
    const store = diary;
    if (!store) return;
    let deactivate: (() => void) | undefined;
    untrack(() => {
      deactivate = workspace.activateContainer(store);
    });
    return () => {
      store.close();
      deactivate?.();
    };
  });

  // Without a day in the address: today's, in place of the diary's address.
  $effect(() => {
    if (diaryId && asked === undefined) {
      router.openDay(
        untrack(() => monthDayOf(today.date)),
        { replace: true },
      );
    }
  });

  // The day of the address is the one read.
  $effect(() => {
    const store = diary;
    const day = monthDay;
    if (store && day) untrack(() => void store.show(day));
  });

  /** The earliest year asked for, before the diary's first. */
  let reach = $state(Infinity);
  /** The year the diary was made in, on the reader's calendar. */
  const made = $derived(
    diary?.diary
      ? yearOf(dateOfInstant(diary.diary.createdAt, workspace.regional.timeZone))
      : yearOf(today.date),
  );
  const from = $derived(Math.min(made, reach));
  const days = $derived(
    monthDay
      ? pageDays(
          monthDay,
          today.date,
          from,
          (diary?.entries ?? []).map((entry) => entry.date),
        )
      : [],
  );
  /**
   * The day of the year before the earliest shown, for "Show <year>": a leap
   * year's for `02-29`, also where no year shown has one.
   */
  const before = $derived.by(() => {
    if (!monthDay) return undefined;
    const below = Math.min(from, ...days.map(yearOf)) - 1;
    for (let year = below; year >= below - 8; year -= 1) {
      const date = dateIn(year, monthDay);
      if (date) return date;
    }
    return undefined;
  });

  /** The year named in the address (`?year=YYYY`), when that day is among the page's days. */
  const target = $derived.by(() => {
    const day = monthDay;
    const year = askedYear;
    if (!day || !year) return undefined;
    const date = dateIn(Number(year), day);
    if (!date) return undefined;
    if ((diary?.entries ?? []).some((entry) => entry.date === date)) return date;
    // Checked against today as the page opened: midnight never opens another day.
    const shown = untrack(() => pageDays(day, today.date, from, []));
    return shown.includes(date) && untrack(() => writable(date, today.date)) ? date : undefined;
  });

  /** This year's day, as the page opens on a day of the year: open if it has come. */
  const opening = $derived.by(() => {
    const day = monthDay;
    if (!day) return undefined;
    return untrack(() => {
      const date = dateIn(yearOf(today.date), day);
      return date && writable(date, today.date) ? date : undefined;
    });
  });
  /** The year the reader opened on this day of the year; resets whenever the route changes. */
  let chosen = $derived<{ route: Route; date: CalendarDate | undefined }>({
    route: router.route,
    date: undefined,
  });
  /** The day the reader opened by pressing it: the caret goes into it. */
  let pressed = $state.raw<CalendarDate | undefined>(undefined);

  // Asked by the hub of today to write today: the caret goes into today, where a keyboard is likely.
  $effect(() => {
    const id = diaryId;
    if (id && untrack(() => takeWriteToday(id)) && page.stage !== 'compact') pressed = today.date;
  });
  const open = $derived(chosen.date ?? target ?? opening);

  function openDate(date: CalendarDate): void {
    // Reading `chosen` first syncs its route dependency before overriding `date` for this route.
    chosen = { route: chosen.route, date };
  }

  function step(by: 1 | -1): void {
    if (monthDay) router.openDay(stepMonthDay(monthDay, by));
  }

  /** A day picked from the month: its day of the year, its year shown and open. */
  function pick(date: CalendarDate): void {
    reach = Math.min(reach, yearOf(date));
    router.openDay(monthDayOf(date));
    openDate(date);
  }

  function toToday(): void {
    router.openDay(monthDayOf(today.date));
    openDate(today.date);
  }

  let renaming = $state(false);
  let trash = $state.raw<Trash | undefined>(undefined);
  let trashOpen = $state(false);
  let deleting = $state(false);
  const openEntryId = $derived(
    open ? diary?.entries.find((one) => one.date === open)?.id : undefined,
  );
  /** The versions of the open entry, read anew each time their dialog opens; resets when the open entry changes. */
  let versions = $derived<{
    entryId: string | undefined;
    reading: VersionsReading | undefined;
    open: boolean;
  }>({
    entryId: openEntryId,
    reading: undefined,
    open: false,
  });
  const title = $derived(diary?.diary?.title || m.diary_untitled());
  nameWindow(() => {
    return [diary?.diary && title, monthDay && displayDay(inLeapYear(monthDay), locale)];
  });

  async function rename(store: DiaryStore, name: string): Promise<void> {
    try {
      workspace.receiveContainer(
        await workspace.backend.patchContainer(store.diaryId, { title: name }),
      );
    } catch (error) {
      workspace.report('save', error);
    }
  }

  /** Deletes the diary; the page goes on to the first board, wiki or diary of the sidebar. */
  async function deleteDiary(store: DiaryStore): Promise<void> {
    const done = await whileHere(workspace, async () => {
      await workspace.deleteContainer(store.diaryId);
      return true;
    });
    if (!done || workspace.containers.some((left) => left.id === store.diaryId)) return;
    if (router.route.kind !== 'diary' || router.route.diaryId !== store.diaryId) return;
    const next = workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (next) router.openContainer(next);
    else router.go({ workspaceId: workspace.info?.id }, { replace: true });
  }
</script>

<!-- An open day shown, the keys that keep a version of its body save no page anywhere on it. -->
<svelte:window
  onkeydown={(event: KeyboardEvent) => {
    if (open) holdSaveKey(event);
  }}
  onpagehide={(event: PageTransitionEvent) => {
    diary?.leave(event.persisted);
  }}
/>

{#snippet body(entry: EntryStore, label: string)}
  <EntryBody {entry} {label} focus={pressed === entry.date} />
{/snippet}

{#if (asked !== undefined && !monthDay) || diary?.missing}
  <NotFound {router} />
{:else if !diary || !monthDay}
  <div class="grid h-full place-items-center p-6">
    <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
  </div>
{:else}
  {@const store = diary}
  {@const day = monthDay}
  <div class="h-full overflow-y-auto" data-testid="diary-page" data-month-day={day}>
    <div class="mx-auto flex max-w-3xl flex-col gap-2 px-4 pt-4 pb-16 roomy:px-3">
      <DayHeader
        {title}
        calendar={store.calendar}
        monthDay={day}
        today={today.date}
        {locale}
        weekStart={workspace.regional.weekStart}
        onStep={step}
        onToday={toToday}
        onPick={pick}
        onRename={() => {
          renaming = true;
        }}
        onTrash={() => {
          trash = entryTrash(store);
          void trash.read();
          trashOpen = true;
        }}
        onDelete={() => {
          deleting = true;
        }}
      />
      {#if day === '02-29'}
        <p class="text-sm text-muted-foreground">{m.diary_leap_only()}</p>
      {/if}
      <!-- The head stays as the days go by: what has the focus there keeps it. -->
      {#if store.status === 'failed'}
        <p class="py-6 text-center text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      {:else if store.status !== 'ready'}
        <p class="py-6 text-center text-muted-foreground" role="status">{m.app_loading()}</p>
      {:else}
        <div
          aria-label={m.diary_years({ day: displayDay(`2000-${day}` as CalendarDate, locale) })}
          role="list"
          class="flex flex-col"
        >
          {#each days as date (date)}
            <div role="listitem" class="border-t first:border-t-0">
              <YearCell
                diary={store}
                {date}
                today={today.date}
                open={open === date}
                {locale}
                onOpen={() => {
                  openDate(date);
                  pressed = date;
                }}
                onVersions={() => {
                  const dayStore = store.day(date);
                  if (!dayStore.entry) return;
                  versions = {
                    entryId: versions.entryId ?? dayStore.entry.id,
                    reading: new VersionsReading(entryVersions(dayStore)),
                    open: true,
                  };
                }}
                {body}
              />
            </div>
          {/each}
        </div>
        {#if before}
          {@const earlier = before}
          <Button
            variant="ghost"
            class="self-start text-muted-foreground roomy:min-h-touch"
            data-testid="diary-earlier"
            onclick={() => {
              reach = Math.min(reach, yearOf(earlier));
            }}
          >
            {m.diary_earlier_year({ year: displayYear(earlier, locale) })}
          </Button>
        {/if}
      {/if}
    </div>
  </div>
  <!-- Another day or entry shown, the dialog of the one before goes with it. -->
  {#if versions.reading}
    <BodyVersions
      bind:open={
        () => versions.open,
        (next: boolean) => {
          versions = { ...versions, open: next };
        }
      }
      reading={versions.reading}
      container={{ workspace, canEdit: true }}
      of="entry"
    />
  {/if}
  <DiaryTrash bind:open={trashOpen} {trash} {workspace} />
  <DiaryNameDialog
    bind:open={renaming}
    initial={store.diary?.title ?? ''}
    onSubmit={(name: string) => void rename(store, name)}
  />
  <ConfirmDialog
    bind:open={deleting}
    title={m.shell_delete_diary_title()}
    description={m.shell_delete_diary_info({ title })}
    confirmLabel={m['DeleteBoardDialog.confirm-delete']()}
    destructive
    onConfirm={() => void deleteDiary(store)}
  />
{/if}
