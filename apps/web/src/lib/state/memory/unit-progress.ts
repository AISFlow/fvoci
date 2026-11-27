/**
 * Personal unit progress and course progress summary in the in-memory backend,
 * matching the server rules (`crates/server/src/api/unit_progress.rs`,
 * `crates/store/src/unit_progress.rs`):
 * - Per-user isolation: each learner reads and writes only their own rows.
 * - Idempotent `completed: true` preserves original `completedAt`; `completed: false` clears it.
 * - `completed: null` is refused (400); omitting `completed` or `lastPosition` preserves it.
 * - `lastPosition` rejects Unicode control chars and > 256 Unicode scalar values after trim (400 `/lastPosition`).
 * - Chapters, trashed units, non-course containers, and unreadable private courses return 404.
 * - Unjoined open-course readers receive `[]` on GET progress, 403 on PUT, and 200 on summary.
 * - Trashed units are omitted from GET progress and summary while rows survive until purge.
 * - Zero WebSocket broadcasts on progress writes.
 */
import {
  hasControlChars,
  UNIT_PROGRESS_POSITION_MAX_CHARS,
  type CourseProgressSummary,
  type UnitProgress,
  type UnitProgressUpdate,
} from '../course-types.ts';

import { roleOn } from './comments.ts';
import { memberRole } from './container-access.ts';
import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import { workersOf } from './spaces.ts';
import { memorySuggestedIntervalForUnit } from './unit-reviews.ts';
import type { World } from './world.ts';

type PositionChange = { kind: 'keep' } | { kind: 'clear' } | { kind: 'set'; value: string };

function checkPosition(raw: string | null | undefined): PositionChange {
  if (raw === undefined) return { kind: 'keep' };
  if (raw === null) return { kind: 'clear' };
  if (hasControlChars(raw)) {
    fail(400, 'control characters are not allowed', {
      field: 'lastPosition',
      reason: 'invalid_characters',
    });
  }
  const trimmed = raw.trim();
  if (Array.from(trimmed).length > UNIT_PROGRESS_POSITION_MAX_CHARS) {
    fail(400, `at most ${UNIT_PROGRESS_POSITION_MAX_CHARS} characters`, {
      field: 'lastPosition',
      reason: 'too_long',
    });
  }
  return trimmed === '' ? { kind: 'clear' } : { kind: 'set', value: trimmed };
}

function hasMembership(world: World, userId: string, courseId: string): boolean {
  return roleOn(world.members.get(courseId) ?? [], userId) !== '';
}

export function memoryMyProgress(world: World, userId: string, courseId: string): UnitProgress[] {
  memberRole(world, userId, courseId, 'course');
  if (!hasMembership(world, userId, courseId)) return [];
  const byUnit = world.unitProgress.get(userId);
  if (!byUnit) return [];
  const rows: UnitProgress[] = [];
  for (const [unitId, row] of byUnit) {
    const block = world.blocks.get(unitId);
    if (block?.containerId === courseId && block.type === 'unit') {
      const cloned = copy(row);
      if (cloned.completedAt !== null) {
        cloned.suggestedNextIntervalDays = memorySuggestedIntervalForUnit(
          world,
          userId,
          courseId,
          unitId,
        );
      }
      rows.push(cloned);
    }
  }
  return rows.sort((a, b) => (a.unitId < b.unitId ? -1 : a.unitId > b.unitId ? 1 : 0));
}

export function memoryPutUnitProgress(
  world: World,
  userId: string,
  courseId: string,
  unitId: string,
  update: UnitProgressUpdate,
): UnitProgress {
  memberRole(world, userId, courseId, 'course');
  if (!hasMembership(world, userId, courseId)) {
    fail(403, 'join the course before recording progress');
  }
  if (
    'completed' in update &&
    update.completed !== undefined &&
    typeof update.completed !== 'boolean'
  ) {
    fail(400, 'completed must be a boolean', { field: 'completed' });
  }
  const positionChange = checkPosition(update.lastPosition);
  const block = world.blocks.get(unitId);
  if (block?.containerId !== courseId || block.type !== 'unit') {
    fail(404, 'no such unit');
  }
  let byUnit = world.unitProgress.get(userId);
  if (!byUnit) {
    byUnit = new Map();
    world.unitProgress.set(userId, byUnit);
  }
  const existing = byUnit.get(unitId);
  const now = world.now();
  const completedAt =
    update.completed === true
      ? (existing?.completedAt ?? now)
      : update.completed === false
        ? null
        : (existing?.completedAt ?? null);
  const lastPosition =
    positionChange.kind === 'set'
      ? positionChange.value
      : positionChange.kind === 'clear'
        ? null
        : (existing?.lastPosition ?? null);
  const next: UnitProgress = {
    unitId,
    completedAt,
    plannedDate: existing?.plannedDate ?? null,
    lastPosition,
    updatedAt: now,
  };
  byUnit.set(unitId, next);
  const result = copy(next);
  if (result.completedAt !== null) {
    result.suggestedNextIntervalDays = memorySuggestedIntervalForUnit(
      world,
      userId,
      courseId,
      unitId,
    );
  }
  return result;
}

export function memoryProgressSummary(
  world: World,
  userId: string,
  courseId: string,
): CourseProgressSummary {
  memberRole(world, userId, courseId, 'course');
  const container = world.containers.get(courseId);
  if (!container) fail(404, 'no such course');
  const liveUnitIds = new Set(
    world
      .blocksOf(courseId)
      .filter((block) => block.type === 'unit')
      .map((block) => block.id),
  );
  const totalUnits = liveUnitIds.size;
  const workers = workersOf(world, container.workspaceId);
  const memberIds = new Set<string>();
  for (const member of world.members.get(courseId) ?? []) {
    if (roleOn([member], member.userId) === '') continue;
    if (!workers.has(member.userId)) continue;
    const user = world.users.get(member.userId);
    if (!user || user.deletedAt !== undefined) continue;
    memberIds.add(member.userId);
  }
  const sortedIds = [...memberIds].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  const members = sortedIds.map((memberId) => {
    const byUnit = world.unitProgress.get(memberId);
    let completed = 0;
    if (byUnit) {
      for (const [unitId, row] of byUnit) {
        if (liveUnitIds.has(unitId) && row.completedAt !== null) completed += 1;
      }
    }
    return { userId: memberId, completedUnits: Math.min(completed, totalUnits) };
  });
  return { totalUnits, members };
}
