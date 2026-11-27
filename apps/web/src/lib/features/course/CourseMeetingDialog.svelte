<!--
  Modal dialog for creating or editing a course study meeting (editors/admins
  and personal-space owners). Uses reader's IANA time zone for date+time inputs
  via `@internationalized/date` helpers (`course-meeting-labels.ts`).
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import CalendarPlusIcon from '@lucide/svelte/icons/calendar-plus';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import { outlineUnits } from '$lib/logic/course-schedule.ts';
  import {
    COURSE_MEETING_LOCATION_MAX_CHARS,
    COURSE_MEETING_MAX_UNITS,
    COURSE_MEETING_NOTES_MAX_CHARS,
    COURSE_MEETING_TITLE_MAX_CHARS,
    hasControlChars,
    type CourseMeeting,
    type CourseMeetingPatch,
    type CourseStore,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  import {
    defaultMeetingInputs,
    meetingErrorsFromBackend,
    memberDisplayLabel,
    parseMeetingInstant,
    splitInstantToInputs,
    type MeetingFormErrors,
  } from './course-meeting-labels.ts';

  interface Props {
    open: boolean;
    course: CourseStore;
    meeting?: CourseMeeting | undefined;
  }

  let { open = $bindable(), course, meeting }: Props = $props();

  const uid = $props.id();
  const compact = $derived(page.stage === 'compact');
  const editing = $derived(meeting !== undefined);
  const busy = $derived(course.events.busy);
  const timeZone = $derived(course.workspace.regional.timeZone);
  const liveUnits = $derived(outlineUnits(course.units));

  let title = $state('');
  let dateText = $state('');
  let timeText = $state('');
  let location = $state('');
  let presenterId = $state('');
  let hostId = $state('');
  let selectedUnitIds = $state.raw<string[]>([]);
  let notes = $state('');
  let errors = $state.raw<MeetingFormErrors>({});
  let snapshotKey = '';

  const activeMemberIds = $derived(new Set(course.orderedMembers.map((member) => member.userId)));
  const preservedPresenter = $derived(
    meeting?.presenterId && !activeMemberIds.has(meeting.presenterId)
      ? meeting.presenterId
      : undefined,
  );
  const preservedHost = $derived(
    meeting?.hostId && !activeMemberIds.has(meeting.hostId) ? meeting.hostId : undefined,
  );

  $effect(() => {
    const key = open ? `${course.courseId}:${meeting?.id ?? 'new'}` : '';
    if (key === snapshotKey) return;
    snapshotKey = key;
    if (!open) return;
    untrack(() => {
      const zone = course.workspace.regional.timeZone;
      if (meeting) {
        const split = splitInstantToInputs(meeting.startsAt, zone);
        title = meeting.title;
        dateText = split.date;
        timeText = split.time;
        location = meeting.location;
        presenterId = meeting.presenterId ?? '';
        hostId = meeting.hostId ?? '';
        const liveIds = new Set(liveUnits.map((unit) => unit.id));
        selectedUnitIds = meeting.unitIds.filter((id) => liveIds.has(id));
        notes = meeting.notes;
      } else {
        const defaults = defaultMeetingInputs(Date.now(), zone);
        title = '';
        dateText = defaults.date;
        timeText = defaults.time;
        location = '';
        presenterId = '';
        hostId = '';
        selectedUnitIds = [];
        notes = '';
      }
      errors = {};
    });
  });

  closeOnBack(
    () => open,
    () => {
      open = false;
    },
  );

  function toggleUnit(unitId: string): void {
    if (selectedUnitIds.includes(unitId)) {
      selectedUnitIds = selectedUnitIds.filter((id) => id !== unitId);
    } else if (selectedUnitIds.length < COURSE_MEETING_MAX_UNITS) {
      selectedUnitIds = [...selectedUnitIds, unitId];
    }
  }

  function validate():
    { startsAt: number; trimmedTitle: string; trimmedLocation: string } | undefined {
    const next: MeetingFormErrors = {};
    const trimmedTitle = title.trim();
    if (
      trimmedTitle === '' ||
      Array.from(trimmedTitle).length > COURSE_MEETING_TITLE_MAX_CHARS ||
      hasControlChars(trimmedTitle)
    ) {
      next.title = m.course_meetings_error_title();
    }
    const startsAt = parseMeetingInstant(dateText, timeText, timeZone);
    if (startsAt === undefined) next.startsAt = m.course_meetings_error_datetime();
    const trimmedLocation = location.trim();
    if (
      Array.from(trimmedLocation).length > COURSE_MEETING_LOCATION_MAX_CHARS ||
      hasControlChars(trimmedLocation)
    ) {
      next.location = m.course_meetings_error_location();
    }
    if (selectedUnitIds.length > COURSE_MEETING_MAX_UNITS) {
      next.unitIds = m.course_meetings_error_units({ max: COURSE_MEETING_MAX_UNITS });
    }
    if (Array.from(notes).length > COURSE_MEETING_NOTES_MAX_CHARS) {
      next.notes = m.course_meetings_error_notes();
    }
    if (Object.keys(next).length > 0 || startsAt === undefined) {
      errors = next;
      return undefined;
    }
    errors = {};
    return { startsAt, trimmedTitle, trimmedLocation };
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!course.canEdit || busy) return;
    const valid = validate();
    if (!valid) return;
    const nextPresenter = presenterId === '' ? null : presenterId;
    const nextHost = hostId === '' ? null : hostId;
    let assignedMember: boolean;
    let result;
    if (meeting) {
      const patch: CourseMeetingPatch = {
        title: valid.trimmedTitle,
        startsAt: valid.startsAt,
        location: valid.trimmedLocation,
        unitIds: selectedUnitIds,
        notes,
        // Omit unchanged presenter/host so preserved tombstones or departed members are kept safely.
        ...(presenterId === (meeting.presenterId ?? '') ? {} : { presenterId: nextPresenter }),
        ...(hostId === (meeting.hostId ?? '') ? {} : { hostId: nextHost }),
      };
      assignedMember = Boolean(patch.presenterId || patch.hostId);
      result = await course.events.patchMeeting(meeting.id, patch);
    } else {
      assignedMember = Boolean(nextPresenter || nextHost);
      result = await course.events.createMeeting({
        title: valid.trimmedTitle,
        startsAt: valid.startsAt,
        location: valid.trimmedLocation,
        unitIds: selectedUnitIds,
        presenterId: nextPresenter,
        hostId: nextHost,
        notes,
      });
    }
    if (result.ok) {
      open = false;
    } else if (result.error) {
      errors = meetingErrorsFromBackend(result.error, assignedMember);
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    showCloseButton={false}
    data-testid="course-meeting-dialog"
    class={compact
      ? 'top-0 left-0 flex h-dvh max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none p-0 pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
      : 'flex max-h-[min(44rem,calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard)))] flex-col gap-0 p-0 sm:max-w-lg'}
  >
    <Dialog.Header class="shrink-0 flex-row items-center gap-2 border-b px-4 py-3">
      <CalendarPlusIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      <Dialog.Title class="min-w-0 flex-1 text-base font-semibold">
        {editing ? m.course_meetings_edit() : m.course_meetings_new()}
      </Dialog.Title>
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()} data-testid="close-meeting-dialog">
            <XIcon />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>

    <form novalidate class="flex min-h-0 flex-1 flex-col" onsubmit={(e) => void submit(e)}>
      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 py-4">
        <div class="flex flex-col gap-1">
          <Label for="{uid}-title">{m.course_meetings_field_title()}</Label>
          <Input
            id="{uid}-title"
            bind:value={title}
            maxlength={COURSE_MEETING_TITLE_MAX_CHARS}
            required
            disabled={busy}
            data-testid="meeting-title-input"
            class="roomy:min-h-touch"
          />
          {#if errors.title}
            <p class="text-xs text-destructive" role="alert">{errors.title}</p>
          {/if}
        </div>

        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div class="flex flex-col gap-1">
            <Label for="{uid}-date">{m.course_meetings_field_date()}</Label>
            <Input
              id="{uid}-date"
              type="date"
              bind:value={dateText}
              required
              disabled={busy}
              data-testid="meeting-date-input"
              class="roomy:min-h-touch"
            />
          </div>
          <div class="flex flex-col gap-1">
            <Label for="{uid}-time">{m.course_meetings_field_time()}</Label>
            <Input
              id="{uid}-time"
              type="time"
              bind:value={timeText}
              required
              disabled={busy}
              data-testid="meeting-time-input"
              class="roomy:min-h-touch"
            />
          </div>
        </div>
        {#if errors.startsAt}
          <p class="-mt-2 text-xs text-destructive" role="alert">{errors.startsAt}</p>
        {/if}

        <div class="flex flex-col gap-1">
          <Label for="{uid}-location">{m.course_meetings_field_location()}</Label>
          <Input
            id="{uid}-location"
            bind:value={location}
            maxlength={COURSE_MEETING_LOCATION_MAX_CHARS}
            placeholder={m.course_meetings_field_location_placeholder()}
            disabled={busy}
            data-testid="meeting-location-input"
            class="roomy:min-h-touch"
          />
          {#if errors.location}
            <p class="text-xs text-destructive" role="alert">{errors.location}</p>
          {/if}
        </div>

        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div class="flex flex-col gap-1">
            <Label for="{uid}-presenter">{m.course_meetings_field_presenter()}</Label>
            <select
              id="{uid}-presenter"
              bind:value={presenterId}
              disabled={busy}
              data-testid="meeting-presenter-select"
              class="h-9 rounded-md border border-input-border bg-background px-2.5 text-base text-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none md:text-sm roomy:min-h-touch"
            >
              <option value="">{m.course_meetings_field_none()}</option>
              {#if preservedPresenter}
                <option value={preservedPresenter}
                  >{memberDisplayLabel(course, preservedPresenter)}</option
                >
              {/if}
              {#each course.orderedMembers as member (member.userId)}
                <option value={member.userId}>{memberDisplayLabel(course, member.userId)}</option>
              {/each}
            </select>
          </div>
          <div class="flex flex-col gap-1">
            <Label for="{uid}-host">{m.course_meetings_field_host()}</Label>
            <select
              id="{uid}-host"
              bind:value={hostId}
              disabled={busy}
              data-testid="meeting-host-select"
              class="h-9 rounded-md border border-input-border bg-background px-2.5 text-base text-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none md:text-sm roomy:min-h-touch"
            >
              <option value="">{m.course_meetings_field_none()}</option>
              {#if preservedHost}
                <option value={preservedHost}>{memberDisplayLabel(course, preservedHost)}</option>
              {/if}
              {#each course.orderedMembers as member (member.userId)}
                <option value={member.userId}>{memberDisplayLabel(course, member.userId)}</option>
              {/each}
            </select>
          </div>
        </div>
        {#if errors.member}
          <p class="-mt-2 text-xs text-destructive" role="alert">{errors.member}</p>
        {/if}

        <fieldset class="flex flex-col gap-1.5">
          <legend class="text-sm font-medium">{m.course_meetings_field_units()}</legend>
          {#if liveUnits.length === 0}
            <p class="text-xs text-muted-foreground">{m.course_meetings_field_units_empty()}</p>
          {:else}
            <div class="flex max-h-36 flex-col gap-1 overflow-y-auto rounded-md border p-2">
              {#each liveUnits as unit (unit.id)}
                {@const checked = selectedUnitIds.includes(unit.id)}
                <label
                  class="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 text-sm hover:bg-muted"
                >
                  <input
                    type="checkbox"
                    {checked}
                    disabled={busy ||
                      (!checked && selectedUnitIds.length >= COURSE_MEETING_MAX_UNITS)}
                    data-testid="meeting-unit-option-{unit.id}"
                    onchange={() => {
                      toggleUnit(unit.id);
                    }}
                  />
                  <span class="truncate">{unit.title || m.course_unit_untitled()}</span>
                </label>
              {/each}
            </div>
          {/if}
          {#if errors.unitIds}
            <p class="text-xs text-destructive" role="alert">{errors.unitIds}</p>
          {/if}
        </fieldset>

        <div class="flex flex-col gap-1">
          <Label for="{uid}-notes">{m.course_meetings_field_notes()}</Label>
          <Textarea
            id="{uid}-notes"
            bind:value={notes}
            rows={3}
            maxlength={COURSE_MEETING_NOTES_MAX_CHARS}
            disabled={busy}
            data-testid="meeting-notes-input"
          />
          {#if errors.notes}
            <p class="text-xs text-destructive" role="alert">{errors.notes}</p>
          {/if}
        </div>

        {#if errors.general}
          <p class="text-xs text-destructive" role="alert">{errors.general}</p>
        {/if}
      </div>

      <Dialog.Footer class="shrink-0 border-t px-4 py-3">
        <Button
          type="button"
          variant="outline"
          class="roomy:min-h-touch"
          disabled={busy}
          onclick={() => {
            open = false;
          }}
        >
          {m.shared_cancel()}
        </Button>
        <Button
          type="submit"
          class="roomy:min-h-touch"
          disabled={busy}
          data-testid="meeting-submit"
        >
          {editing ? m.course_meetings_save() : m.course_meetings_create()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
