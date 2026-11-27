/**
 * Personal study notes (`unit_notes`) in the in-memory backend, matching the
 * server rules (`crates/server/src/api/unit_reviews.rs`,
 * `crates/store/src/unit_reviews/notes.rs`).
 */
import {
  UNIT_NOTE_CONTENT_MAX_CHARS,
  UNIT_NOTE_KINDS,
  validateUnitNoteContent,
  type CreateUnitNote,
  type PatchUnitNote,
  type UnitNote,
  type UnitNoteKind,
} from '../unit-review-types.ts';

import { roleOn } from './comments.ts';
import { memberRole } from './container-access.ts';
import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import type { World } from './world.ts';

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function hasMembership(world: World, userId: string, courseId: string): boolean {
  return roleOn(world.members.get(courseId) ?? [], userId) !== '';
}

function requireEnrolled(world: World, userId: string, courseId: string, detail: string): void {
  if (!hasMembership(world, userId, courseId)) fail(403, detail);
}

function requireLiveUnit(world: World, courseId: string, unitId: string): void {
  const block = world.blocks.get(unitId);
  if (block?.containerId !== courseId || block.type !== 'unit') fail(404, 'no such unit');
}

function checkNoteKind(raw: unknown, field = 'kind'): UnitNoteKind {
  if (typeof raw !== 'string' || !(UNIT_NOTE_KINDS as readonly string[]).includes(raw)) {
    fail(400, 'kind must be one of question, mistake, or review', { field, reason: 'invalid' });
  }
  return raw as UnitNoteKind;
}

function checkNoteContent(raw: unknown): string {
  if (typeof raw !== 'string') {
    fail(400, 'content must be a string', { field: 'content', reason: 'invalid' });
  }
  const error = validateUnitNoteContent(raw);
  if (error === 'invalid_characters') {
    fail(400, 'control characters other than line breaks and tabs are not allowed', {
      field: 'content',
      reason: 'invalid_characters',
    });
  }
  if (error === 'empty') {
    fail(400, 'must not be empty', { field: 'content', reason: 'required' });
  }
  if (error === 'too_long') {
    fail(400, `at most ${UNIT_NOTE_CONTENT_MAX_CHARS} characters`, {
      field: 'content',
      reason: 'too_long',
    });
  }
  return raw.trim();
}

function compareNotesDesc(a: UnitNote, b: UnitNote): number {
  if (a.createdAt !== b.createdAt) return b.createdAt - a.createdAt;
  return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
}

export function memoryListMyUnitNotes(
  world: World,
  userId: string,
  courseId: string,
  unitId: string,
  kind?: UnitNoteKind,
): UnitNote[] {
  memberRole(world, userId, courseId, 'course');
  const filterKind = kind === undefined ? undefined : checkNoteKind(kind);
  requireLiveUnit(world, courseId, unitId);
  if (!hasMembership(world, userId, courseId)) return [];
  const rows = (world.unitNotes.get(userId) ?? []).filter(
    (note) =>
      note.containerId === courseId &&
      note.unitId === unitId &&
      (filterKind === undefined || note.kind === filterKind),
  );
  return rows.sort(compareNotesDesc).map(copy);
}

export function memoryCreateUnitNote(
  world: World,
  userId: string,
  courseId: string,
  unitId: string,
  request: CreateUnitNote,
): UnitNote {
  memberRole(world, userId, courseId, 'course');
  requireEnrolled(world, userId, courseId, 'join the course before recording notes');
  const kind = checkNoteKind(request.kind);
  const content = checkNoteContent(request.content);
  requireLiveUnit(world, courseId, unitId);
  const now = world.now();
  const created: UnitNote = {
    id: world.newId(),
    unitId,
    containerId: courseId,
    kind,
    content,
    createdAt: now,
    updatedAt: now,
  };
  const theirs = world.unitNotes.get(userId) ?? [];
  theirs.push(created);
  world.unitNotes.set(userId, theirs);
  return copy(created);
}

export function memoryPatchUnitNote(
  world: World,
  userId: string,
  courseId: string,
  noteId: string,
  patch: PatchUnitNote,
): UnitNote {
  memberRole(world, userId, courseId, 'course');
  requireEnrolled(world, userId, courseId, 'join the course before updating notes');
  const hasKind = 'kind' in patch && patch.kind !== undefined;
  const hasContent = 'content' in patch && patch.content !== undefined;
  if (!hasKind && !hasContent) {
    fail(400, 'at least one of kind or content is required', {
      field: 'content',
      reason: 'required',
    });
  }
  const kind = hasKind ? checkNoteKind(patch.kind) : undefined;
  const content = hasContent ? checkNoteContent(patch.content) : undefined;
  if (!UUID_RE.test(noteId)) fail(404, 'no such unit note');
  const theirs = world.unitNotes.get(userId) ?? [];
  const note = theirs.find((item) => item.id === noteId && item.containerId === courseId);
  if (!note) fail(404, 'no such unit note');
  requireLiveUnit(world, courseId, note.unitId);
  if (kind !== undefined) note.kind = kind;
  if (content !== undefined) note.content = content;
  note.updatedAt = world.now();
  return copy(note);
}

export function memoryDeleteUnitNote(
  world: World,
  userId: string,
  courseId: string,
  noteId: string,
): void {
  memberRole(world, userId, courseId, 'course');
  requireEnrolled(world, userId, courseId, 'join the course before deleting notes');
  if (!UUID_RE.test(noteId)) fail(404, 'no such unit note');
  const theirs = world.unitNotes.get(userId) ?? [];
  const index = theirs.findIndex((item) => item.id === noteId && item.containerId === courseId);
  if (index === -1) fail(404, 'no such unit note');
  theirs.splice(index, 1);
}
