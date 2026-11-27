<!--
  Study meetings tab of a course: chronological meeting list, covered unit
  chips, presenter & host badges (with deleted-user tombstone support), shared
  notes, editor CRUD dialog, and inline shared attendance matrix.

  When `focusMeetingId` is provided (e.g. from clicking a `meeting_upcoming`
  notification), the matching meeting card is highlighted, scrolled into view,
  and focused. Never mounted on public share links.
-->
<script lang="ts">
  import type { Attachment } from 'svelte/attachments';

  import BookOpenIcon from '@lucide/svelte/icons/book-open';
  import CalendarIcon from '@lucide/svelte/icons/calendar';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import MapPinIcon from '@lucide/svelte/icons/map-pin';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import UserIcon from '@lucide/svelte/icons/user';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { displayDateTime } from '$lib/logic/format.ts';
  import type { CourseMeeting, CourseStore, CourseUnit } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import CourseMeetingDialog from './CourseMeetingDialog.svelte';
  import MeetingAttendanceMatrix from './MeetingAttendanceMatrix.svelte';
  import { memberDisplayLabel, safeHttpUrl } from './course-meeting-labels.ts';

  interface Props {
    course: CourseStore;
    focusMeetingId?: string | undefined;
    onOpenUnit: (unitId: string) => void;
  }

  let { course, focusMeetingId, onOpenUnit }: Props = $props();

  let dialogOpen = $state(false);
  let editingMeeting = $state.raw<CourseMeeting | undefined>(undefined);
  let deletingMeeting = $state.raw<CourseMeeting | undefined>(undefined);
  let deleteOpen = $state(false);
  let lastFocusedKey = '';

  const meetings = $derived(course.events.meetings);
  const displayContext = $derived({
    locale: course.workspace.locale,
    timeZone: course.workspace.regional.timeZone,
    hourCycle: course.workspace.regional.hourCycle,
  });

  function focusTarget(meetingId: string): Attachment<HTMLElement> {
    return (element) => {
      const targetId = focusMeetingId;
      if (!targetId) {
        lastFocusedKey = '';
        return;
      }
      if (targetId !== meetingId) return;
      const key = `${course.courseId}:${meetingId}`;
      if (key === lastFocusedKey) return;
      lastFocusedKey = key;
      element.scrollIntoView({ block: 'nearest' });
      element.focus();
    };
  }

  function liveLinkedUnits(unitIds: readonly string[]): CourseUnit[] {
    return unitIds
      .map((id) => course.unit(id))
      .filter((unit): unit is CourseUnit => unit?.kind === 'unit');
  }

  function openCreate(): void {
    editingMeeting = undefined;
    dialogOpen = true;
  }

  function openEdit(meeting: CourseMeeting): void {
    editingMeeting = meeting;
    dialogOpen = true;
  }

  function askDelete(meeting: CourseMeeting): void {
    deletingMeeting = meeting;
    deleteOpen = true;
  }
</script>

<div
  class="mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6"
  data-testid="course-meetings-tab"
>
  <header class="flex flex-wrap items-start justify-between gap-3">
    <div class="min-w-0 flex-1">
      <h2 class="text-xl font-semibold">{m.course_meetings_title()}</h2>
      <p class="mt-0.5 text-sm text-muted-foreground">{m.course_meetings_subtitle()}</p>
    </div>
    {#if course.canEdit}
      <Button class="shrink-0 roomy:min-h-touch" data-testid="new-meeting" onclick={openCreate}>
        <PlusIcon aria-hidden="true" />
        {m.course_meetings_new()}
      </Button>
    {/if}
  </header>

  {#if course.events.meetingsFailed && !course.events.meetingsLoaded}
    <div class="flex flex-col items-center gap-3 py-8 text-center">
      <p class="text-sm text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        onclick={() => void course.events.refreshMeetings()}
      >
        {m.app_retry()}
      </Button>
    </div>
  {:else if !course.events.meetingsLoaded}
    <p class="py-8 text-center text-sm text-muted-foreground" role="status">{m.app_loading()}</p>
  {:else if meetings.length === 0}
    <div
      class="flex flex-col items-center gap-3 rounded-lg border border-dashed px-6 py-10 text-center"
      data-testid="meetings-empty"
    >
      <p class="font-medium">{m.course_meetings_empty()}</p>
      <p class="max-w-md text-sm text-muted-foreground">{m.course_meetings_empty_hint()}</p>
      {#if course.canEdit}
        <Button class="roomy:min-h-touch" onclick={openCreate}>
          <PlusIcon aria-hidden="true" />
          {m.course_meetings_new()}
        </Button>
      {/if}
    </div>
  {:else}
    <div class="flex flex-col gap-4">
      {#each meetings as meeting (meeting.id)}
        {@const focused = meeting.id === focusMeetingId}
        {@const locationUrl = safeHttpUrl(meeting.location)}
        {@const linkedUnits = liveLinkedUnits(meeting.unitIds)}
        <article
          tabindex="-1"
          data-testid="meeting-card"
          data-meeting-id={meeting.id}
          data-focused={focused ? 'true' : undefined}
          {@attach focusTarget(meeting.id)}
          class={[
            'flex flex-col gap-3 rounded-lg border bg-card p-4 text-card-foreground outline-none',
            focused && 'border-ring ring-2 ring-ring',
          ]}
        >
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0 flex-1">
              <h3 class="text-base font-semibold break-words" data-testid="meeting-title">
                {meeting.title}
              </h3>
              <div
                class="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground"
              >
                <span class="inline-flex items-center gap-1" data-testid="meeting-starts-at">
                  <CalendarIcon class="size-3.5 shrink-0" aria-hidden="true" />
                  {displayDateTime(meeting.startsAt, displayContext)}
                </span>
                {#if meeting.location}
                  <span
                    class="inline-flex min-w-0 items-center gap-1"
                    data-testid="meeting-location"
                  >
                    <MapPinIcon class="size-3.5 shrink-0" aria-hidden="true" />
                    {#if locationUrl}
                      <a
                        href={locationUrl}
                        target="_blank"
                        rel="noopener noreferrer"
                        class="inline-flex items-center gap-1 truncate text-foreground underline underline-offset-2 hover:text-primary"
                      >
                        <span class="truncate">{meeting.location}</span>
                        <ExternalLinkIcon class="size-3 shrink-0" aria-hidden="true" />
                      </a>
                    {:else}
                      <span class="break-words">{meeting.location}</span>
                    {/if}
                  </span>
                {/if}
              </div>
            </div>

            {#if course.canEdit}
              <div class="flex shrink-0 items-center gap-1">
                <IconButton
                  label={m.course_meetings_edit()}
                  data-testid="edit-meeting"
                  class="text-muted-foreground"
                  onclick={() => {
                    openEdit(meeting);
                  }}
                >
                  <PencilIcon />
                </IconButton>
                <IconButton
                  label={m.course_meetings_delete()}
                  data-testid="delete-meeting"
                  class="text-muted-foreground hover:text-destructive"
                  onclick={() => {
                    askDelete(meeting);
                  }}
                >
                  <Trash2Icon />
                </IconButton>
              </div>
            {/if}
          </div>

          {#if meeting.presenterId || meeting.hostId}
            <div class="flex flex-wrap items-center gap-2 text-xs">
              {#if meeting.presenterId}
                <span
                  class="inline-flex items-center gap-1 rounded-md border bg-muted/60 px-2 py-0.5"
                  data-testid="meeting-presenter"
                >
                  <UserIcon class="size-3 text-muted-foreground" aria-hidden="true" />
                  <span class="text-muted-foreground">{m.course_meetings_presenter()}:</span>
                  <span class="font-medium">{memberDisplayLabel(course, meeting.presenterId)}</span>
                </span>
              {/if}
              {#if meeting.hostId}
                <span
                  class="inline-flex items-center gap-1 rounded-md border bg-muted/60 px-2 py-0.5"
                  data-testid="meeting-host"
                >
                  <UserIcon class="size-3 text-muted-foreground" aria-hidden="true" />
                  <span class="text-muted-foreground">{m.course_meetings_host()}:</span>
                  <span class="font-medium">{memberDisplayLabel(course, meeting.hostId)}</span>
                </span>
              {/if}
            </div>
          {/if}

          {#if linkedUnits.length > 0}
            <div class="flex flex-wrap items-center gap-1.5" data-testid="meeting-units">
              <span class="text-xs text-muted-foreground">{m.course_meetings_units()}:</span>
              {#each linkedUnits as linked (linked.id)}
                <button
                  type="button"
                  data-testid="meeting-unit-chip"
                  class="inline-flex items-center gap-1 rounded-full border bg-secondary px-2.5 py-0.5 text-xs font-medium text-secondary-foreground hover:bg-accent roomy:min-h-touch"
                  onclick={() => {
                    onOpenUnit(linked.id);
                  }}
                >
                  <BookOpenIcon class="size-3 shrink-0" aria-hidden="true" />
                  <span class="truncate">{linked.title || m.course_unit_untitled()}</span>
                </button>
              {/each}
            </div>
          {/if}

          {#if meeting.notes}
            <p
              class="rounded-md bg-muted/50 px-3 py-2 text-sm whitespace-pre-line text-foreground"
              data-testid="meeting-notes"
            >
              {meeting.notes}
            </p>
          {/if}

          <MeetingAttendanceMatrix {course} {meeting} />
        </article>
      {/each}
    </div>
  {/if}
</div>

{#if course.canEdit}
  <CourseMeetingDialog bind:open={dialogOpen} {course} meeting={editingMeeting} />
  {#if deletingMeeting}
    {@const target = deletingMeeting}
    <ConfirmDialog
      bind:open={deleteOpen}
      title={m.course_meetings_delete_title()}
      description={m.course_meetings_delete_info({ title: target.title })}
      confirmLabel={m.shared_delete()}
      destructive
      onConfirm={() => {
        void course.events.deleteMeeting(target.id);
      }}
    />
  {/if}
{/if}
