<!--
  The diaries of the reader's own space on the hub of today, a line each: a
  way to write today's day while it is not written, else the days in a row.
-->
<script lang="ts">
  import NotebookPenIcon from '@lucide/svelte/icons/notebook-pen';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { TodayDiary } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    diaries: readonly TodayDiary[];
    /** Opens the diary on today, to write it. */
    onWrite: (diary: TodayDiary) => void;
    onOpen: (diary: TodayDiary) => void;
  }

  let { diaries, onWrite, onOpen }: Props = $props();
</script>

<section class="grid gap-2" data-testid="today-hub-diaries">
  <h3 class="text-sm font-semibold text-muted-foreground">{m.today_hub_diaries()}</h3>
  <ul class="grid gap-1.5">
    {#each diaries as diary (diary.diaryId)}
      <li
        class="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 rounded-lg border px-3 py-2"
        data-testid="today-hub-diary"
      >
        <span class="min-w-0 flex-1 truncate font-medium">{diary.title || m.diary_untitled()}</span>
        {#if diary.writtenToday}
          <span class="text-sm text-muted-foreground" data-testid="today-hub-diary-streak">
            {m.today_hub_diary_streak({ days: diary.streak })}
          </span>
          <Button
            variant="outline"
            size="sm"
            class="roomy:min-h-touch"
            onclick={() => {
              onOpen(diary);
            }}
          >
            {m.today_hub_diary_open()}
          </Button>
        {:else}
          <Button
            size="sm"
            class="roomy:min-h-touch"
            data-testid="today-hub-diary-write"
            onclick={() => {
              onWrite(diary);
            }}
          >
            <NotebookPenIcon aria-hidden="true" />
            {m.today_hub_diary_write()}
          </Button>
        {/if}
      </li>
    {/each}
  </ul>
</section>
