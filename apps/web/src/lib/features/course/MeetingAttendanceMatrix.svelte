<!--
  Shared attendance matrix for one course study meeting.
  - Editors/admins (and personal-space owners) may mark attendance for any active
    enrolled course member.
  - Enrolled non-editors (viewer/commenter/guest) may mark only their own row.
  - Unjoined open-course readers see read-only attendance.
  - Erased/departed users retained in `meeting.attendance` render read-only with
    the localized deleted-user tombstone label.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    ATTENDANCE_STATUSES,
    type AttendanceStatus,
    type CourseMeeting,
    type CourseStore,
  } from '$lib/state/index.ts';

  import { attendanceLabel, attendanceTone, memberDisplayLabel } from './course-meeting-labels.ts';

  interface Props {
    course: CourseStore;
    meeting: CourseMeeting;
  }

  let { course, meeting }: Props = $props();

  interface RosterRow {
    userId: string;
    name: string;
    isMe: boolean;
    activeMember: boolean;
    status: AttendanceStatus | undefined;
  }

  const meId = $derived(course.workspace.me?.id);

  const attendanceByUser = $derived(
    new Map(meeting.attendance.map((row) => [row.userId, row.status])),
  );

  const counts = $derived.by(() => {
    const out: Record<AttendanceStatus, number> = { present: 0, late: 0, absent: 0, excused: 0 };
    for (const row of meeting.attendance) out[row.status] += 1;
    return out;
  });

  const rows = $derived.by<RosterRow[]>(() => {
    const activeIds = new Set(course.orderedMembers.map((member) => member.userId));
    const list: RosterRow[] = course.orderedMembers.map((member) => ({
      userId: member.userId,
      name: memberDisplayLabel(course, member.userId),
      isMe: member.userId === meId,
      activeMember: true,
      status: attendanceByUser.get(member.userId),
    }));
    for (const record of meeting.attendance) {
      if (activeIds.has(record.userId)) continue;
      list.push({
        userId: record.userId,
        name: memberDisplayLabel(course, record.userId),
        isMe: record.userId === meId,
        activeMember: false,
        status: record.status,
      });
    }
    return list;
  });

  function canEditRow(row: RosterRow): boolean {
    if (!row.activeMember || !course.canTrackProgress) return false;
    return course.canEdit || row.isMe;
  }
</script>

<section
  class="flex flex-col gap-2.5 border-t pt-3"
  aria-label={m.course_attendance_title()}
  data-testid="meeting-attendance-matrix"
>
  <div class="flex flex-wrap items-center justify-between gap-2">
    <h4 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
      {m.course_attendance_title()}
    </h4>
    <div class="flex flex-wrap items-center gap-1.5" data-testid="attendance-summary">
      {#each ATTENDANCE_STATUSES as status (status)}
        <span
          class={[
            'inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-xs tabular-nums',
            counts[status] > 0
              ? attendanceTone(status)
              : 'border-border bg-muted/60 text-muted-foreground',
          ]}
          data-testid="attendance-summary-{status}"
        >
          <span>{attendanceLabel(status)}</span>
          <span class="font-semibold">{counts[status]}</span>
        </span>
      {/each}
    </div>
  </div>

  {#if course.canTrackProgress && !course.canEdit}
    <p class="text-xs text-muted-foreground" data-testid="attendance-self-hint">
      {m.course_attendance_self_hint()}
    </p>
  {/if}

  {#if rows.length === 0}
    <p class="text-xs text-muted-foreground">{m.course_attendance_empty()}</p>
  {:else}
    <ul class="divide-y divide-border rounded-md border bg-background">
      {#each rows as row (row.userId)}
        {@const editable = canEditRow(row)}
        {@const saving = course.events.isAttendanceBusy(meeting.id, row.userId)}
        <li
          class="flex flex-col gap-2 px-3 py-2 sm:flex-row sm:items-center sm:justify-between"
          data-testid="attendance-row"
          data-user-id={row.userId}
        >
          <div class="flex min-w-0 items-center gap-1.5">
            <span
              class={['truncate text-sm', !row.activeMember && 'text-muted-foreground italic']}
              data-testid="attendance-member-name"
            >
              {row.name}
            </span>
            {#if row.isMe}
              <span
                class="shrink-0 rounded-full bg-muted px-1.5 py-0.5 text-[0.6875rem] font-medium text-muted-foreground"
              >
                {m.course_attendance_you()}
              </span>
            {/if}
          </div>

          {#if editable}
            <div
              class="flex flex-wrap items-center gap-1"
              role="group"
              aria-label="{row.name} — {m.course_attendance_status()}"
            >
              {#each ATTENDANCE_STATUSES as status (status)}
                {@const active = row.status === status}
                <button
                  type="button"
                  disabled={saving}
                  aria-pressed={active}
                  data-testid="attendance-btn-{status}"
                  class={[
                    'rounded-md border px-2 py-1 text-xs font-medium transition-colors roomy:min-h-touch',
                    active
                      ? attendanceTone(status)
                      : 'border-border bg-background text-muted-foreground hover:bg-muted hover:text-foreground',
                    saving && 'opacity-60',
                  ]}
                  onclick={() => {
                    void course.events.setAttendance(meeting.id, row.userId, status);
                  }}
                >
                  {attendanceLabel(status)}
                </button>
              {/each}
            </div>
          {:else}
            <span
              class={[
                'inline-flex w-fit items-center rounded-md border px-2 py-0.5 text-xs font-medium',
                row.status
                  ? attendanceTone(row.status)
                  : 'border-border bg-muted/60 text-muted-foreground',
              ]}
              data-testid="attendance-readonly-status"
            >
              {row.status ? attendanceLabel(row.status) : m.course_attendance_unmarked()}
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>
