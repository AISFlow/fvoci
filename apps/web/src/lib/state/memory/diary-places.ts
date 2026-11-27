/**
 * Where a diary may come by a way other than its own route, as the server has
 * it (`check_place`): an archive imported, a diary brought back from the
 * trash. Only into the reader's own space, no more than it may hold, and each
 * day of a diary's entries once and not far ahead. A refusal comes before
 * anything is stored.
 */
import { addDays, dateOfUtc, parseCalendarDate } from '../../logic/dates.ts';
import type { Container } from '../../logic/wire.ts';
import { DAYS_AHEAD, DIARIES_PER_SPACE, ENTRIES_PER_DIARY } from './diaries.ts';
import { fail } from './refuse.ts';
import type { StoredBlock, World } from './world.ts';

/** `more` diaries come into `workspaceId`: it is the reader's own space, with room for them. */
export function checkDiaryRoom(
  world: World,
  userId: string,
  workspaceId: string,
  more: number,
): void {
  if (more === 0) return;
  if (world.ownSpaces.get(userId)?.id !== workspaceId) {
    fail(400, 'a diary is kept in its owner’s own space alone', { field: 'kind' });
  }
  const there = [...world.containers.values()].filter(
    (container) => container.workspaceId === workspaceId && container.kind === 'diary',
  );
  if (there.length + more > DIARIES_PER_SPACE) {
    fail(400, 'the space has as many diaries as it may', {
      reason: 'too_many_diaries',
      limit: DIARIES_PER_SPACE,
    });
  }
}

/** The containers of an archive come into `workspaceId`: its diaries, and the days of their entries. */
export function checkDiariesComing(
  world: World,
  userId: string,
  workspaceId: string,
  coming: { containers: readonly Container[]; blocks: readonly StoredBlock[] },
): void {
  const diaries = coming.containers.filter((container) => container.kind === 'diary');
  checkDiaryRoom(world, userId, workspaceId, diaries.length);
  const diaryIds = new Set(diaries.map((diary) => diary.id));
  for (const block of coming.blocks) {
    if (block.type === 'entry' && !diaryIds.has(block.containerId)) {
      fail(400, 'an entry belongs on a diary alone', { field: 'type' });
    }
  }
  const latest = addDays(dateOfUtc(world.now()), DAYS_AHEAD);
  for (const diary of diaries) {
    const mine = coming.blocks.filter((block) => block.containerId === diary.id);
    for (const block of mine) {
      if (block.type !== 'entry') fail(400, 'a diary holds entries alone', { field: 'type' });
      if (block.parentId !== '') fail(400, 'an entry is below no block', { field: 'parentId' });
    }
    if (mine.length > ENTRIES_PER_DIARY) {
      fail(400, 'the diary has as many entries as it may', {
        reason: 'too_many_entries',
        limit: ENTRIES_PER_DIARY,
      });
    }
    const days = new Set<string>();
    for (const block of mine) {
      const date = block.fields.date;
      const day = typeof date === 'string' ? parseCalendarDate(date) : undefined;
      if (!day) fail(400, 'an entry’s day is no day', { field: 'fields' });
      if (day > latest) fail(400, 'an entry’s day is too far ahead');
      if (days.has(day)) fail(400, 'a diary has one entry a day');
      days.add(day);
    }
  }
}
