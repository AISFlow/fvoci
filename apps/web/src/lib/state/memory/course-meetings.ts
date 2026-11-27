/**
 * In-memory implementation of course study meetings and shared attendance
 * matching `crates/model/src/course_meeting.rs`, `crates/store/src/course_meetings.rs`,
 * and `crates/server/src/app/course_events.rs`.
 *
 * Invariants:
 * - Meetings sorted by `startsAt ASC, id ASC`; attendance sorted by `userId ASC`.
 * - `presenterId`, `hostId`, and `attendance` preserve tombstones on user erasure.
 */
import { isGone } from '../../logic/users.ts';
import {
  compareCourseMeetings,
  COURSE_MEETING_LAST_MS,
  COURSE_MEETING_LOCATION_MAX_CHARS,
  COURSE_MEETING_MAX_UNITS,
  COURSE_MEETING_NOTES_MAX_CHARS,
  COURSE_MEETING_TITLE_MAX_CHARS,
  isAttendanceStatus,
  type CourseMeeting,
  type CourseMeetingPatch,
  type CourseMeetingsApi,
  type MeetingAttendance,
  type MeetingAttendanceUpdate,
  type NewCourseMeeting,
} from '../course-meeting-types.ts';
import { hasControlChars } from '../course-types.ts';

import { roleOn } from './comments.ts';
import { memberRole } from './container-access.ts';
import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import type { WikiHost } from './wikis.ts';
import type { World } from './world.ts';

function hasDisallowedMultilineControlChars(raw: string): boolean {
  for (const ch of raw) {
    const code = ch.codePointAt(0) ?? 0;
    if (ch === '\n' || ch === '\r' || ch === '\t') continue;
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

export function activeMemberIds(world: World, courseId: string): Set<string> {
  const active = new Set<string>();
  for (const member of world.members.get(courseId) ?? []) {
    const user = world.users.get(member.userId);
    if (user && !isGone(user) && user.deletedAt === undefined) active.add(member.userId);
  }
  return active;
}

export function isEnrolledActive(world: World, userId: string, courseId: string): boolean {
  return (
    roleOn(world.members.get(courseId) ?? [], userId) !== '' &&
    activeMemberIds(world, courseId).has(userId)
  );
}

export function mayManageCourseEvents(world: World, userId: string, courseId: string): void {
  const role = memberRole(world, userId, courseId, 'course');
  if (role !== 'admin' && role !== 'editor') fail(403, 'access denied to the course');
}

export function checkTitle(raw: string, maxChars: number): string {
  const trimmed = raw.trim();
  if (trimmed === '') fail(400, 'title is required', { field: 'title', reason: 'required' });
  if (Array.from(trimmed).length > maxChars) {
    fail(400, `at most ${maxChars} characters`, {
      field: 'title',
      reason: 'too_long',
      limit: maxChars,
    });
  }
  if (hasControlChars(trimmed)) {
    fail(400, 'control characters are not allowed', {
      field: 'title',
      reason: 'invalid_characters',
    });
  }
  return trimmed;
}

function checkSingleLine(raw: string, field: string, maxChars: number): string {
  const trimmed = raw.trim();
  if (Array.from(trimmed).length > maxChars) {
    fail(400, `at most ${maxChars} characters`, { field, reason: 'too_long', limit: maxChars });
  }
  if (hasControlChars(trimmed)) {
    fail(400, 'control characters are not allowed', { field, reason: 'invalid_characters' });
  }
  return trimmed;
}

export function checkMultiline(raw: string, field: string, maxChars: number): string {
  if (Array.from(raw).length > maxChars) {
    fail(400, `at most ${maxChars} characters`, { field, reason: 'too_long', limit: maxChars });
  }
  if (hasDisallowedMultilineControlChars(raw)) {
    fail(400, 'control characters are not allowed', { field, reason: 'invalid_characters' });
  }
  return raw;
}

function checkStartsAt(value: number): number {
  if (!Number.isInteger(value) || value < 0 || value > COURSE_MEETING_LAST_MS) {
    fail(400, 'startsAt must be milliseconds from 0 to the end of 9999', {
      field: 'startsAt',
      reason: 'invalid',
    });
  }
  return value;
}

function checkUnitIds(raw: readonly string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const id of raw) {
    if (typeof id !== 'string' || id.trim() === '') {
      fail(400, 'must be a valid identifier', { field: 'unitIds', reason: 'invalid' });
    }
    if (!seen.has(id)) {
      seen.add(id);
      out.push(id);
    }
  }
  if (out.length > COURSE_MEETING_MAX_UNITS) {
    fail(400, `at most ${COURSE_MEETING_MAX_UNITS} unit identifiers`, {
      field: 'unitIds',
      reason: 'too_long',
      limit: COURSE_MEETING_MAX_UNITS,
    });
  }
  return out;
}

function checkOptionalMember(
  world: World,
  courseId: string,
  userId: string | null | undefined,
  field: 'presenterId' | 'hostId',
): string | null {
  if (userId === undefined || userId === null) return null;
  if (userId.trim() === '') fail(400, 'must be a valid identifier', { field, reason: 'invalid' });
  if (!isEnrolledActive(world, userId, courseId)) {
    fail(404, 'member is not enrolled in this course');
  }
  return userId;
}

function cloneMeeting(meeting: CourseMeeting): CourseMeeting {
  return {
    ...copy(meeting),
    attendance: [...meeting.attendance]
      .sort((a, b) => (a.userId < b.userId ? -1 : a.userId > b.userId ? 1 : 0))
      .map(copy),
  };
}

function findMeeting(world: World, courseId: string, meetingId: string): CourseMeeting {
  const list = world.courseMeetings.get(courseId) ?? [];
  const found = list.find((item) => item.id === meetingId);
  if (!found) fail(404, 'no such meeting');
  return found;
}

export function memoryListCourseMeetings(
  world: World,
  userId: string,
  courseId: string,
): CourseMeeting[] {
  memberRole(world, userId, courseId, 'course');
  return [...(world.courseMeetings.get(courseId) ?? [])]
    .sort(compareCourseMeetings)
    .map(cloneMeeting);
}

export function memoryCreateCourseMeeting(
  world: World,
  userId: string,
  courseId: string,
  draft: NewCourseMeeting,
): CourseMeeting {
  mayManageCourseEvents(world, userId, courseId);
  const title = checkTitle(draft.title, COURSE_MEETING_TITLE_MAX_CHARS);
  const startsAt = checkStartsAt(draft.startsAt);
  const location = checkSingleLine(
    draft.location ?? '',
    'location',
    COURSE_MEETING_LOCATION_MAX_CHARS,
  );
  const unitIds = checkUnitIds(draft.unitIds ?? []);
  const presenterId = checkOptionalMember(world, courseId, draft.presenterId, 'presenterId');
  const hostId = checkOptionalMember(world, courseId, draft.hostId, 'hostId');
  const notes = checkMultiline(draft.notes ?? '', 'notes', COURSE_MEETING_NOTES_MAX_CHARS);
  const now = world.now();
  const meeting: CourseMeeting = {
    id: world.newId(),
    containerId: courseId,
    title,
    startsAt,
    location,
    unitIds,
    presenterId,
    hostId,
    notes,
    notifiedAt: null,
    createdAt: now,
    updatedAt: now,
    attendance: [],
  };
  const existing = world.courseMeetings.get(courseId) ?? [];
  existing.push(meeting);
  world.courseMeetings.set(courseId, existing);
  return cloneMeeting(meeting);
}

export function memoryPatchCourseMeeting(
  world: World,
  userId: string,
  courseId: string,
  meetingId: string,
  patch: CourseMeetingPatch,
): CourseMeeting {
  mayManageCourseEvents(world, userId, courseId);
  const meeting = findMeeting(world, courseId, meetingId);
  // Validate all fields before mutating so a rejected PATCH leaves stored state untouched.
  const nextTitle =
    patch.title === undefined ? undefined : checkTitle(patch.title, COURSE_MEETING_TITLE_MAX_CHARS);
  const nextStartsAt = patch.startsAt === undefined ? undefined : checkStartsAt(patch.startsAt);
  const nextLocation =
    patch.location === undefined
      ? undefined
      : checkSingleLine(patch.location, 'location', COURSE_MEETING_LOCATION_MAX_CHARS);
  const nextUnitIds = patch.unitIds === undefined ? undefined : checkUnitIds(patch.unitIds);
  const hasPresenter = 'presenterId' in patch && patch.presenterId !== undefined;
  const nextPresenterId = hasPresenter
    ? checkOptionalMember(world, courseId, patch.presenterId, 'presenterId')
    : undefined;
  const hasHost = 'hostId' in patch && patch.hostId !== undefined;
  const nextHostId = hasHost
    ? checkOptionalMember(world, courseId, patch.hostId, 'hostId')
    : undefined;
  const nextNotes =
    patch.notes === undefined
      ? undefined
      : checkMultiline(patch.notes, 'notes', COURSE_MEETING_NOTES_MAX_CHARS);

  if (nextTitle !== undefined) meeting.title = nextTitle;
  if (nextStartsAt !== undefined) meeting.startsAt = nextStartsAt;
  if (nextLocation !== undefined) meeting.location = nextLocation;
  if (nextUnitIds !== undefined) meeting.unitIds = nextUnitIds;
  if (hasPresenter) meeting.presenterId = nextPresenterId ?? null;
  if (hasHost) meeting.hostId = nextHostId ?? null;
  if (nextNotes !== undefined) meeting.notes = nextNotes;
  meeting.updatedAt = world.now();
  return cloneMeeting(meeting);
}

export function memoryDeleteCourseMeeting(
  world: World,
  userId: string,
  courseId: string,
  meetingId: string,
): void {
  mayManageCourseEvents(world, userId, courseId);
  const list = world.courseMeetings.get(courseId) ?? [];
  const index = list.findIndex((item) => item.id === meetingId);
  if (index === -1) fail(404, 'no such meeting');
  list.splice(index, 1);
}

export function memoryPutMeetingAttendance(
  world: World,
  callerId: string,
  courseId: string,
  meetingId: string,
  targetUserId: string,
  update: MeetingAttendanceUpdate,
): MeetingAttendance {
  const role = memberRole(world, callerId, courseId, 'course');
  if (!isEnrolledActive(world, callerId, courseId)) {
    fail(403, 'join the course before recording attendance');
  }
  if (targetUserId !== callerId && role !== 'admin' && role !== 'editor') {
    fail(403, 'only editors can update another member’s attendance');
  }
  if (!isAttendanceStatus(update.status)) {
    fail(400, 'invalid attendance status', { field: 'status', reason: 'invalid' });
  }
  const meeting = findMeeting(world, courseId, meetingId);
  if (!isEnrolledActive(world, targetUserId, courseId)) {
    fail(404, 'member is not enrolled in this course');
  }
  const row: MeetingAttendance = {
    userId: targetUserId,
    status: update.status,
    updatedAt: world.now(),
  };
  const index = meeting.attendance.findIndex((item) => item.userId === targetUserId);
  if (index === -1) meeting.attendance.push(row);
  else meeting.attendance[index] = row;
  meeting.attendance.sort((a, b) => (a.userId < b.userId ? -1 : a.userId > b.userId ? 1 : 0));
  return copy(row);
}

export function memoryCourseMeetings(
  host: WikiHost,
  enter: () => Promise<void>,
): CourseMeetingsApi {
  return {
    async list(courseId) {
      await enter();
      return memoryListCourseMeetings(host.world, host.userId, courseId);
    },
    async create(courseId, draft) {
      await enter();
      return memoryCreateCourseMeeting(host.world, host.userId, courseId, draft);
    },
    async patch(courseId, meetingId, patch) {
      await enter();
      return memoryPatchCourseMeeting(host.world, host.userId, courseId, meetingId, patch);
    },
    async delete(courseId, meetingId) {
      await enter();
      memoryDeleteCourseMeeting(host.world, host.userId, courseId, meetingId);
    },
    async putAttendance(courseId, meetingId, userId, update) {
      await enter();
      return memoryPutMeetingAttendance(
        host.world,
        host.userId,
        courseId,
        meetingId,
        userId,
        update,
      );
    },
  };
}
