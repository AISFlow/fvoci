/**
 * The backend in memory keeps the server's rules where a diary is reached by
 * a way other than its own routes: an archive imported, a diary brought back
 * from the trash, search, and the routes of boards and wikis (members,
 * sharing, a board's blocks), which have no diary. And the page of a diary
 * keeps its days true to what the diary has after what it missed (a resync),
 * and when a day left blank is opened again before its entry is deleted.
 */
import { describe, expect, it, vi } from 'vitest';

import { dateOfUtc } from '$lib/logic/dates.ts';
import { monthDayOf } from '$lib/logic/diary-days.ts';
import { BackendError, DiaryStore, firstDiary, type Change } from '$lib/state/index.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';
import { DIARIES_PER_SPACE, ENTRIES_PER_DIARY } from '$lib/state/memory/diaries.ts';
import { checkDiariesComing } from '$lib/state/memory/diary-places.ts';
// Loaded before a room is opened, as the page has it by then.
import '$lib/state/memory/rooms.ts';

import { settle, setup, text } from './support.ts';

async function statusOf(promise: Promise<unknown>): Promise<number | 'ok'> {
  try {
    await promise;
    return 'ok';
  } catch (error) {
    if (error instanceof BackendError) return error.status;
    throw error;
  }
}

async function spaces(backend: MemoryBackend): Promise<{ ownId: string; sharedId: string }> {
  const all = await backend.workspaces();
  const own = all.find((workspace) => workspace.kind === 'personal');
  const shared = all.find((workspace) => workspace.kind !== 'personal');
  if (!own || !shared) throw new Error('no own space, or no shared one');
  return { ownId: own.id, sharedId: shared.id };
}

/** A diary with an entry, and its archive as `exportContainer` makes it. */
async function anArchive() {
  const made = setup();
  const { ownId, sharedId } = await spaces(made.backend);
  const diaries = made.backend.diary;
  const diary = await diaries.createDiary(ownId, { title: 'Days' });
  const today = dateOfUtc(made.world.now());
  const entry = await diaries.createEntry(diary.id, today);
  const saved = await made.backend.exportContainer(diary.id);
  const lines = (await saved.bytes.text()).split('\n').filter((line) => line !== '');
  return { ...made, ownId, sharedId, diaries, diary, entry, today, lines, name: saved.name };
}

/** The archive of `lines`, each changed as `change` has it. */
function archiveOf(lines: readonly string[], name: string, more: readonly string[] = []): File {
  return new File([[...lines, ...more].join('\n')], name);
}

/** The line of the archive that holds the entry, its data changed by `change`. */
function entryLine(
  lines: readonly string[],
  change: (data: Record<string, unknown>) => void,
): string {
  const line = lines.find((one) => one.includes('"entry"'));
  if (!line) throw new Error('the archive holds no entry');
  const parsed = JSON.parse(line) as { data: Record<string, unknown> };
  change(parsed.data);
  return JSON.stringify(parsed);
}

describe('a diary reached by another way than its own routes, in memory', () => {
  it('is imported into the own space alone, as many as it may hold', async () => {
    const made = await anArchive();
    const into = (workspaceId: string) =>
      statusOf(made.backend.importArchive(workspaceId, archiveOf(made.lines, made.name)));
    // Server's `not_of_its_kind`: 400 on `/kind`, not a custom top-level reason.
    await expect(
      made.backend.importArchive(made.sharedId, archiveOf(made.lines, made.name)),
    ).rejects.toMatchObject({
      status: 400,
      reason: undefined,
      errors: [expect.objectContaining({ pointer: '/kind', reason: 'invalid' })],
    });
    expect(await into(made.ownId)).toBe('ok');
    expect(await made.diaries.diaries(made.ownId)).toHaveLength(2);
    for (let left = DIARIES_PER_SPACE - 2; left > 0; left -= 1) {
      await made.diaries.createDiary(made.ownId, {});
    }
    expect(await into(made.ownId)).toBe(400);
    expect(await made.diaries.diaries(made.ownId)).toHaveLength(DIARIES_PER_SPACE);
  });

  it('is not imported with a day twice, one far ahead, missing fields.date, nested or foreign blocks, or past the entry limit', async () => {
    const made = await anArchive();
    const twice = entryLine(made.lines, (data) => {
      data.id = 'another-entry';
    });
    const ahead = entryLine(made.lines, (data) => {
      data.title = '2999-01-01';
      data.fields = { ...(data.fields as object), date: '2999-01-01' };
    });
    const noDateField = entryLine(made.lines, (data) => {
      data.title = made.today;
      data.fields = { mood: null, weather: '', place: '' };
    });
    const nested = entryLine(made.lines, (data) => {
      data.parentId = 'some-parent';
    });
    const nonEntryInDiary = entryLine(made.lines, (data) => {
      data.id = 'foreign-card';
      data.type = 'card';
    });
    const [board] = await made.backend.boards(made.sharedId);
    if (!board) throw new Error('no board');
    const exportedBoard = await made.backend.exportContainer(board.id);
    const boardLines = (await exportedBoard.bytes.text()).split('\n').filter((line) => line !== '');
    const entryInBoard = entryLine(made.lines, (data) => {
      data.id = 'stray-entry';
      data.containerId = board.id;
    });
    const asIs = made.lines.filter((line) => !line.includes('"entry"'));
    for (const [archive, pointer] of [
      [archiveOf(made.lines, made.name, [twice]), undefined],
      [archiveOf(asIs, made.name, [ahead]), undefined],
      [archiveOf(asIs, made.name, [noDateField]), '/fields'],
      [archiveOf(asIs, made.name, [nested]), '/parentId'],
      [archiveOf(made.lines, made.name, [nonEntryInDiary]), '/type'],
      [archiveOf(boardLines, exportedBoard.name, [entryInBoard]), '/type'],
    ] as const) {
      const result = made.backend.importArchive(made.ownId, archive);
      if (pointer) {
        await expect(result).rejects.toMatchObject({
          status: 400,
          errors: [expect.objectContaining({ pointer })],
        });
      } else {
        expect(await statusOf(result)).toBe(400);
      }
    }
    // Over ENTRIES_PER_DIARY entries in one diary is refused with `too_many_entries`.
    const sample = made.world.blocks.get(made.entry.id);
    if (!sample) throw new Error('no entry');
    const tooMany = Array.from({ length: ENTRIES_PER_DIARY + 1 }, (_, i) => ({
      ...sample,
      id: `e-${i}`,
    }));
    expect(() => {
      checkDiariesComing(made.world, made.backend.userId, made.ownId, {
        containers: [made.diary],
        blocks: tooMany,
      });
    }).toThrow(expect.objectContaining({ status: 400, reason: 'too_many_entries' }));
    // Refused before anything was stored.
    expect(await made.diaries.diaries(made.ownId)).toHaveLength(1);
  });

  it('comes back from the trash only where there is room for it', async () => {
    const made = await anArchive();
    await made.backend.deleteContainer(made.diary.id);
    for (let left = DIARIES_PER_SPACE; left > 0; left -= 1) {
      await made.diaries.createDiary(made.ownId, {});
    }
    expect(await statusOf(made.backend.restoreContainer(made.diary.id))).toBe(400);
  });

  it('is searched: its entries are found as entries of a diary', async () => {
    const made = await anArchive();
    await made.diaries.patchEntry(made.diary.id, made.entry.id, {
      body: text('바다에 간 날'),
      readAt: made.entry.updatedAt,
    });
    const hits = await made.backend.search.find(made.ownId, { query: '바다에' });
    expect(hits).toEqual([
      expect.objectContaining({ id: made.entry.id, kind: 'entry', containerKind: 'diary' }),
    ]);
  });

  it('has no members, no sharing, no card properties and no blocks of a board, and its entries are below no block', async () => {
    const made = await anArchive();
    const { backend, diary, entry, ownId } = made;
    const wiki = await backend.wiki.createWiki(ownId, { title: 'Notes', visibility: 'private' });
    const reader = backend.userId;
    for (const asked of [
      backend.members(diary.id),
      backend.addMember(diary.id, reader, 'editor'),
      backend.setMemberRole(diary.id, reader, 'editor'),
      backend.removeMember(diary.id, reader),
      backend.publicLink(diary.id),
      backend.setPublicLink(diary.id, { enabled: true }),
      backend.reorderCardProperties(diary.id, []),
      backend.reorderCardProperties(wiki.id, []),
      backend.deleteBlock(diary.id, entry.id),
    ]) {
      expect(await statusOf(asked)).toBe(404);
    }
    expect(made.world.blocks.get(entry.id)?.parentId).toBe('');
  });
});

/** The reader's diary page on today, with what it hears passed through `heard`. */
async function aPage(heard: (change: Change) => boolean = () => true) {
  const made = setup();
  const { ownId } = await spaces(made.backend);
  await made.workspace.load(ownId);
  const diary = await firstDiary(made.workspace);
  if (!diary) throw new Error('no diary');
  let tell: (change: Change) => void = () => undefined;
  const watch = made.backend.watchContainer.bind(made.backend);
  vi.spyOn(made.backend, 'watchContainer').mockImplementation((id, listener) => {
    tell = listener;
    return watch(id, (change) => {
      if (heard(change)) listener(change);
    });
  });
  const store = new DiaryStore(made.workspace, diary.id);
  const today = dateOfUtc(made.world.now());
  await store.show(monthDayOf(today));
  return {
    ...made,
    diary,
    store,
    today,
    tell: (change: Change) => {
      tell(change);
    },
  };
}

describe('a diary’s page after what it missed', () => {
  it('lets a day go whose entry was deleted unheard, once what was missed is read', async () => {
    // Deletions are not heard: the page is offline as the entry goes.
    const { store, backend, diary, today, tell } = await aPage(
      (change) => change.kind !== 'blocksDeleted',
    );
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    await vi.waitFor(() => {
      expect(entry.entry?.id).toBe(there.id);
    });
    await backend.as(backend.userId).diary.deleteEntry(diary.id, there.id);
    await settle();
    expect(entry.entry?.id).toBe(there.id);
    tell({ kind: 'resync' });
    await vi.waitFor(() => {
      expect(entry.entry).toBeUndefined();
    });
    expect(store.entries).toEqual([]);
    store.close();
  });

  it('opens the room of the entry the day has now, made elsewhere after one deleted unheard', async () => {
    const { store, backend, diary, today } = await aPage(
      (change) => change.kind !== 'blocksDeleted',
    );
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    entry.openBody();
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    const opening = vi.spyOn(backend, 'openDoc');
    const elsewhere = backend.as(backend.userId).diary;
    await elsewhere.deleteEntry(diary.id, there.id);
    const again = await elsewhere.createEntry(diary.id, today);
    await vi.waitFor(() => {
      expect(entry.entry?.id).toBe(again.id);
    });
    expect(opening).toHaveBeenCalledWith(expect.anything(), diary.id, again.id, expect.anything());
    store.close();
  });

  it('keeps an entry left blank and opened again before it was deleted, and still deletes it when folded again', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.mark({ mood: 'bad' });
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    entry.mark({ mood: null });
    entry.noteBlank(true);
    entry.closeBody();
    // Opened again while what the room sends is settled.
    entry.openBody();
    await settle();
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toHaveLength(1);
    // Folded a second time while still blank: `#changedHere` was restored, so it is deleted now.
    entry.noteBlank(true);
    entry.closeBody();
    await vi.waitFor(async () => {
      expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toEqual([]);
    });
    store.close();
  });

  it('still deletes a blank entry on the next fold after a deletion failure', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.mark({ mood: 'bad' });
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    vi.spyOn(backend.diary, 'deleteEntry').mockRejectedValueOnce(
      new BackendError(500, { detail: 'down' }),
    );
    entry.mark({ mood: null });
    entry.noteBlank(true);
    entry.closeBody();
    await settle();
    expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toHaveLength(1);
    // Opened and folded again without editing: still remembers it was emptied here.
    entry.openBody();
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    entry.noteBlank(true);
    entry.closeBody();
    await vi.waitFor(async () => {
      expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toEqual([]);
    });
    store.close();
  });

  it('does not resurrect a deleted entry when an earlier read of the day answers later', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    let release = (): void => undefined;
    const read = backend.diary.entriesOn.bind(backend.diary);
    vi.spyOn(backend.diary, 'entriesOn').mockImplementationOnce(async (...args) => {
      const answer = await read(...args);
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return answer;
    });
    const reading = store.show(monthDayOf(today));
    await settle();
    await backend.as(backend.userId).diary.deleteEntry(diary.id, there.id);
    await vi.waitFor(() => {
      expect(store.entries).toEqual([]);
    });
    release();
    await reading;
    expect(store.entries).toEqual([]);
    expect(store.day(today).entry).toBeUndefined();
    store.close();
  });

  it('drops a stale #gone check after deletion or newer update, and tells the calendar on 404', async () => {
    // Deletions are not heard: `#gone` discovers a missing entry on a read of the day.
    const { store, backend, diary, today, world } = await aPage(
      (change) => change.kind !== 'blocksDeleted',
    );
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    await store.calendar.showMonth(today);
    await vi.waitFor(() => {
      expect(store.calendar.days).toHaveLength(1);
      expect(store.calendar.runs).toEqual([{ from: today, to: today, days: 1 }]);
    });
    const day = store.day(today);
    await vi.waitFor(() => {
      expect(day.entry?.id).toBe(there.id);
    });
    // Deleted unheard: calling `show` (not `resync`) triggers `#gone` without `calendar.resync()`,
    // so the calendar clears only because `#gone`'s 404 branch calls `calendar.heardDeleted`.
    const heardDeleted = vi.spyOn(store.calendar, 'heardDeleted');
    await backend.as(backend.userId).diary.deleteEntry(diary.id, there.id);
    await store.show(monthDayOf(today));
    await vi.waitFor(() => {
      expect(day.entry).toBeUndefined();
      expect(store.calendar.days).toEqual([]);
      expect(store.calendar.runs).toEqual([]);
    });
    expect(heardDeleted).toHaveBeenCalledWith([there.id]);

    // `#gone` check skips a stale answer when a newer update arrived while it was in flight.
    const fresh = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    await vi.waitFor(() => {
      expect(day.entry?.id).toBe(fresh.id);
    });
    let releaseStale = (): void => undefined;
    const getEntry = backend.diary.entry.bind(backend.diary);
    vi.spyOn(backend.diary, 'entry').mockImplementationOnce(async (...args) => {
      const answer = await getEntry(...args);
      await new Promise<void>((resolve) => {
        releaseStale = resolve;
      });
      return answer;
    });
    vi.spyOn(backend.diary, 'entriesOn').mockResolvedValueOnce([]);
    await store.show(monthDayOf(today));
    const made = world.now();
    world.useClock(() => made + 5000);
    const newer = await backend.diary.patchEntry(diary.id, fresh.id, { weather: '맑음' });
    store.received(newer);
    releaseStale();
    await settle();
    expect(store.entries[0]?.weather).toBe('맑음');

    // `#gone` check drops its answer when the entry is deleted while `#gone` is in flight.
    let releaseDeleted = (): void => undefined;
    vi.spyOn(backend.diary, 'entry').mockImplementationOnce(async (...args) => {
      const answer = await getEntry(...args);
      await new Promise<void>((resolve) => {
        releaseDeleted = resolve;
      });
      return answer;
    });
    vi.spyOn(backend.diary, 'entriesOn').mockResolvedValueOnce([]);
    await store.show(monthDayOf(today));
    store.removed(fresh.id);
    day.lose();
    expect(store.entries).toEqual([]);
    releaseDeleted();
    await settle();
    expect(store.entries).toEqual([]);
    expect(day.entry).toBeUndefined();
    store.close();
  });
});
