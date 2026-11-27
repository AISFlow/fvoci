<!--
  "지운 일기" from the diary's menu: the entries of the diary in the trash
  (`entryTrash`, made and read by who opens it), each named by its day and
  its mood, and brought back to its day. A day that has an entry since takes none back (`day_taken`), which the
  reader is told; deleting the day's entry makes room.
-->
<script lang="ts">
  import { MOOD_FACES } from '$lib/features/diary/moods.ts';
  import TrashDialog from '$lib/features/shell/TrashDialog.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { parseCalendarDate } from '$lib/logic/dates.ts';
  import { displayDayWithYear } from '$lib/logic/format.ts';
  import type { DeletedItem, Trash, Workspace } from '$lib/state/index.ts';

  interface Props {
    open: boolean;
    trash: Trash | undefined;
    workspace: Workspace;
  }

  let { open = $bindable(), trash, workspace }: Props = $props();

  function nameOf(item: DeletedItem): string {
    const day = parseCalendarDate(item.title);
    const date = day ? displayDayWithYear(day, workspace.locale) : item.title;
    return item.mood ? `${date} · ${MOOD_FACES[item.mood].name()}` : date;
  }
</script>

<TrashDialog
  bind:open
  {trash}
  {workspace}
  title={m.diary_trash()}
  description={m.diary_trash_retention({ days: workspace.trashRetentionDays })}
  untitled={m.diary_untitled()}
  nameItem={nameOf}
  testId="entry-trash"
/>
