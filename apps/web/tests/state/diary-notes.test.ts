/**
 * What a day of a diary carries besides its body, and its calendar.
 *
 * The backend in memory keeps the server's rules for both: a mood, weather or
 * place is set as it is sent (no version), weather and place are 100
 * characters at most, the written days of a range are a year at most, and the
 * runs of written days are the diary's alone.
 *
 * Over them the stores of the page: a mark is the day's at once, makes the
 * entry of a day that has none, holds a day from being deleted as blank, and
 * is sent once the typing pauses or the field is left, never taken back by a
 * change heard meanwhile; the calendar follows what is written in the diary.
 */
import { describe, expect, it, vi } from 'vitest';

import { addDays, dateOfUtc, type CalendarDate } from '$lib/logic/dates.ts';
import { monthDayOf } from '$lib/logic/diary-days.ts';
import { BackendError, DiaryStore, firstDiary } from '$lib/state/index.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';
// Loaded before a room is opened, as the page has it by then (`memory-diaries.test.ts` too).
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

async function ownSpace(backend: MemoryBackend): Promise<string> {
  const own = (await backend.workspaces()).find((workspace) => workspace.kind === 'personal');
  if (!own) throw new Error('no own space');
  return own.id;
}

/** The reader's own space open in the workspace, its diary's page on today. */
async function aPage() {
  const made = setup();
  const ownId = await ownSpace(made.backend);
  await made.workspace.load(ownId);
  const diary = await firstDiary(made.workspace);
  if (!diary) throw new Error('no diary');
  const store = new DiaryStore(made.workspace, diary.id);
  const today = dateOfUtc(made.world.now());
  await store.show(monthDayOf(today));
  return { ...made, ownId, diary, store, today };
}

describe('the notes and days of diaries in memory', () => {
  it('set a mood, weather and place as they are sent, each told, and refuse a long one', async () => {
    const { backend, diary, today } = await aPage();
    const made = await backend.diary.createEntry(diary.id, today);
    const told = vi.fn();
    const stop = backend.watchContainer(diary.id, told);
    const marked = await backend.diary.patchEntry(diary.id, made.id, {
      mood: 'good',
      weather: '맑음',
    });
    expect(marked).toMatchObject({ mood: 'good', weather: '맑음', place: '' });
    expect(marked.updatedAt).toBeGreaterThanOrEqual(made.updatedAt);
    await vi.waitFor(() => {
      expect(told).toHaveBeenCalledWith(
        expect.objectContaining({
          kind: 'entries',
          entries: [expect.objectContaining({ id: made.id, mood: 'good', weather: '맑음' })],
        }),
      );
    });
    const cleared = await backend.diary.patchEntry(diary.id, made.id, { mood: null });
    expect(cleared).toMatchObject({ mood: null, weather: '맑음' });
    const long = await statusOf(
      backend.diary.patchEntry(diary.id, made.id, { place: '가'.repeat(101) }),
    );
    expect(long).toBe(400);
    expect(await backend.diary.patchEntry(diary.id, made.id, { place: '가'.repeat(100) })).toEqual(
      expect.objectContaining({ place: '가'.repeat(100) }),
    );
    stop();
  });

  it('list the written days of a range of a year at most, and the runs of the diary alone', async () => {
    const { backend, diary, ownId } = await aPage();
    const other = await backend.diary.createDiary(ownId, { title: 'Runs' });
    const on = (text: string) => text as CalendarDate;
    for (const date of ['2024-02-28', '2024-02-29', '2024-03-01', '2024-03-03']) {
      await backend.diary.createEntry(diary.id, on(date));
    }
    await backend.diary.createEntry(other.id, on('2024-03-02'));
    const marked = await backend.diary.createEntry(diary.id, on('2024-01-31'));
    await backend.diary.patchEntry(diary.id, marked.id, { mood: 'bad' });
    const days = await backend.diary.days(diary.id, on('2024-01-01'), on('2024-02-29'));
    expect(days).toEqual([
      { date: '2024-01-31', entryId: marked.id, mood: 'bad' },
      expect.objectContaining({ date: '2024-02-28', mood: null }),
      expect.objectContaining({ date: '2024-02-29' }),
    ]);
    expect(await statusOf(backend.diary.days(diary.id, on('2024-01-01'), on('2025-01-01')))).toBe(
      400,
    );
    expect(await statusOf(backend.diary.days(diary.id, on('2023-01-02'), on('2024-01-01')))).toBe(
      'ok',
    );
    expect(await backend.diary.runs(diary.id)).toEqual([
      { from: '2024-01-31', to: '2024-01-31', days: 1 },
      { from: '2024-02-28', to: '2024-03-01', days: 3 },
      { from: '2024-03-03', to: '2024-03-03', days: 1 },
    ]);
    expect(await backend.diary.runs(other.id)).toEqual([
      { from: '2024-03-02', to: '2024-03-02', days: 1 },
    ]);
  });

  it('are no board: a diary is not copied, and has no trash of cards', async () => {
    const { backend, diary, ownId } = await aPage();
    expect(await statusOf(backend.duplicateContainer(diary.id, false, ownId))).toBe(404);
    expect(await statusOf(backend.deletedCards(diary.id))).toBe(404);
  });
});

describe('the notes of a day on its page', () => {
  it('are the day’s at once, and make the entry of a day that has none, once', async () => {
    const { store, backend, diary, today } = await aPage();
    const making = vi.spyOn(backend.diary, 'createEntry');
    const entry = store.day(today);
    entry.mark({ mood: 'great' });
    entry.mark({ mood: 'good' });
    expect(entry.notes.mood).toBe('good');
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.mood).toBe('good');
    });
    expect(making).toHaveBeenCalledTimes(1);
    expect(store.entries.map((one) => one.mood)).toEqual(['good']);
    store.close();
  });

  it('keep a day with a mood from being deleted as blank when it is folded before the mood is answered', async () => {
    const { store, backend, diary, today } = await aPage();
    // The mood is held on its way until the day is folded.
    const patch = backend.diary.patchEntry.bind(backend.diary);
    let release = (): void => undefined;
    vi.spyOn(backend.diary, 'patchEntry').mockImplementationOnce(
      (...args) =>
        new Promise((resolve, reject) => {
          release = () => {
            patch(...args).then(resolve, reject);
          };
        }),
    );
    const entry = store.day(today);
    entry.openBody();
    entry.mark({ mood: 'okay' });
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    entry.noteBlank(true);
    entry.closeBody();
    await settle();
    expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toHaveLength(1);
    release();
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.mood).toBe('okay');
    });
    store.close();
  });

  it('leave a day made here blank once its mood is taken away, and it is deleted as it is left', async () => {
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
    await vi.waitFor(async () => {
      expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toEqual([]);
    });
    store.close();
  });

  it('send what is typed once, when the field is left, over what another device sent meanwhile', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    const patching = vi.spyOn(backend.diary, 'patchEntry');
    for (const typed of ['비', '비 온', '비 온 뒤 맑음']) entry.mark({ weather: typed }, 'later');
    expect(patching).not.toHaveBeenCalled();
    // Another device's weather is heard while this one is typed: what is typed stays.
    await backend.as(backend.userId).diary.patchEntry(diary.id, there.id, { weather: '눈' });
    await vi.waitFor(() => {
      expect(store.entries[0]?.weather).toBe('눈');
    });
    expect(entry.notes.weather).toBe('비 온 뒤 맑음');
    entry.sendNotes();
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.weather).toBe('비 온 뒤 맑음');
    });
    expect(patching).toHaveBeenCalledTimes(1);
    expect(patching).toHaveBeenCalledWith(
      diary.id,
      there.id,
      { weather: '비 온 뒤 맑음' },
      { keepalive: true },
    );
    store.close();
  });

  it('send what is typed as the page goes, and still once it has gone', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    const patching = vi.spyOn(backend.diary, 'patchEntry');
    entry.mark({ place: '집' }, 'later');
    store.leave(false);
    expect(patching).toHaveBeenCalledWith(diary.id, there.id, { place: '집' }, { keepalive: true });
    store.close();
  });

  it('send as the page goes what was typed while another note was on its way', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    // The first note is on its way, and stays so: the page goes before it is answered.
    const patching = vi
      .spyOn(backend.diary, 'patchEntry')
      .mockImplementationOnce(() => new Promise(() => undefined));
    entry.mark({ weather: '비' }, 'later');
    entry.sendNotes();
    entry.mark({ place: '집' }, 'later');
    store.leave(false);
    expect(patching).toHaveBeenLastCalledWith(
      diary.id,
      there.id,
      { weather: '비', place: '집' },
      { keepalive: true },
    );
    store.close();
  });

  it('send what is typed as the page goes in the same write as a draft not taken yet', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('떠나기 전'));
    await vi.waitFor(() => {
      expect(entry.entry).toBeDefined();
    });
    const patching = vi.spyOn(backend.diary, 'patchEntry');
    entry.mark({ weather: '맑음' }, 'later');
    store.leave(false);
    // One write: a write of the notes alone would change the entry the draft's write read.
    expect(patching).toHaveBeenCalledTimes(1);
    expect(patching).toHaveBeenCalledWith(
      diary.id,
      entry.entry?.id,
      expect.objectContaining({ weather: '맑음', appendBody: text('떠나기 전') }),
      { keepalive: true },
    );
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.weather).toBe('맑음');
      expect(listed?.body).toEqual(text('떠나기 전'));
    });
    store.close();
  });

  it('hold the notes of a day while its draft waits for the room, and send them as it takes it', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('방이 받기 전'));
    await vi.waitFor(() => {
      expect(entry.entry).toBeDefined();
    });
    const patching = vi.spyOn(backend.diary, 'patchEntry');
    entry.mark({ mood: 'good' });
    await settle();
    // A patch landing before the draft's write would make that write find the entry changed.
    expect(patching).not.toHaveBeenCalled();
    expect(entry.notes.mood).toBe('good');
    expect(entry.takeDraft()).toEqual(text('방이 받기 전'));
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.mood).toBe('good');
    });
    expect(patching).toHaveBeenCalledWith(
      diary.id,
      entry.entry?.id,
      { mood: 'good' },
      { keepalive: true },
    );
    store.close();
  });

  it('send the notes alone as the page goes while the draft is being written', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('쓰는 중'));
    await vi.waitFor(() => {
      expect(entry.entry).toBeDefined();
    });
    // The draft's write is on its way as the day is folded, and stays so.
    const patching = vi
      .spyOn(backend.diary, 'patchEntry')
      .mockImplementationOnce(() => new Promise(() => undefined));
    entry.closeBody();
    expect(entry.flushing).toBe(true);
    entry.mark({ weather: '비' }, 'later');
    store.leave(false);
    expect(patching).toHaveBeenLastCalledWith(
      diary.id,
      entry.entry?.id,
      { weather: '비' },
      { keepalive: true },
    );
    store.close();
  });

  it('hear another device’s notes once a page kept to come back to has sent its own', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    entry.mark({ weather: '비' }, 'later');
    store.leave(true);
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.weather).toBe('비');
    });
    await settle();
    await backend.as(backend.userId).diary.patchEntry(diary.id, there.id, { weather: '눈' });
    await vi.waitFor(() => {
      expect(entry.notes.weather).toBe('눈');
    });
    store.close();
  });

  it('send nothing twice from a page kept to come back to, what was on its way answered after', async () => {
    const { store, backend, diary, today } = await aPage();
    await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    const patch = backend.diary.patchEntry.bind(backend.diary);
    let release = (): void => undefined;
    const patching = vi.spyOn(backend.diary, 'patchEntry').mockImplementationOnce(
      (...args) =>
        new Promise((resolve, reject) => {
          release = () => {
            patch(...args).then(resolve, reject);
          };
        }),
    );
    entry.mark({ weather: '비' }, 'later');
    entry.sendNotes();
    entry.mark({ place: '집' }, 'later');
    store.leave(true);
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.place).toBe('집');
    });
    release();
    await settle();
    // The note on its way, then both in one write as the page went: what that write
    // carried is answered, and is not sent again behind the first.
    expect(patching).toHaveBeenCalledTimes(2);
    expect(entry.notes).toMatchObject({ weather: '비', place: '집' });
    store.close();
  });

  it('send nothing twice when an in-flight note settles before leave’s combined write', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    const patch = backend.diary.patchEntry.bind(backend.diary);
    let releaseFirst = (): void => undefined;
    let releaseLeave = (): void => undefined;
    const patching = vi
      .spyOn(backend.diary, 'patchEntry')
      .mockImplementationOnce(
        (...args) =>
          new Promise((resolve, reject) => {
            releaseFirst = () => {
              patch(...args).then(resolve, reject);
            };
          }),
      )
      .mockImplementationOnce(
        (...args) =>
          new Promise((resolve, reject) => {
            releaseLeave = () => {
              patch(...args).then(resolve, reject);
            };
          }),
      );
    entry.mark({ weather: '비' }, 'later');
    entry.sendNotes();
    entry.mark({ place: '집' }, 'later');
    store.leave(true);
    // S1 answers BEFORE leave's combined write L: S1 must not resend `{ place }` or clear `#sending`.
    releaseFirst();
    await settle();
    expect(patching).toHaveBeenCalledTimes(2);
    releaseLeave();
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed).toMatchObject({ weather: '비', place: '집' });
    });
    await settle();
    expect(patching).toHaveBeenCalledTimes(2);
    // After L settles, `pending` is false so remote changes are heard.
    await backend.as(backend.userId).diary.patchEntry(diary.id, there.id, { weather: '눈' });
    await vi.waitFor(() => {
      expect(entry.notes.weather).toBe('눈');
    });
    store.close();
  });

  it('send what was emptied in a day left blank whose deletion is refused', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.mark({ weather: '비' });
    await vi.waitFor(async () => {
      expect(entry.room?.status).toBe('joined');
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.weather).toBe('비');
    });
    vi.spyOn(backend.diary, 'deleteEntry').mockRejectedValueOnce(
      new BackendError(500, { detail: 'down' }),
    );
    entry.mark({ weather: '' }, 'later');
    entry.noteBlank(true);
    entry.closeBody();
    // Kept, the entry has what was emptied in it, as the reader left it.
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.weather).toBe('');
    });
    store.close();
  });

  it('make no entry for notes that hold nothing, left unsent when the entry went', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, there.id, { mood: 'bad' });
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    await vi.waitFor(() => {
      expect(entry.entry?.mood).toBe('bad');
    });
    // Its mood taken away, refused as the entry is gone; then the entry is heard deleted.
    vi.spyOn(backend.diary, 'patchEntry').mockRejectedValueOnce(
      new BackendError(404, { detail: 'no such entry' }),
    );
    const making = vi.spyOn(backend.diary, 'createEntry');
    entry.mark({ mood: null });
    await settle();
    await backend.as(backend.userId).diary.deleteEntry(diary.id, there.id);
    await vi.waitFor(() => {
      expect(entry.entry).toBeUndefined();
    });
    store.close();
    await settle();
    expect(making).not.toHaveBeenCalled();
    expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toEqual([]);
  });

  it('clear nothing made elsewhere with a mood taken away on its way to an entry gone', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, there.id, { mood: 'bad' });
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    await vi.waitFor(() => {
      expect(entry.entry?.mood).toBe('bad');
    });
    // The mood taken away is on its way while the entry is deleted elsewhere and another made.
    let refuse = (): void => undefined;
    const patching = vi.spyOn(backend.diary, 'patchEntry').mockImplementationOnce(
      () =>
        new Promise((_, reject) => {
          refuse = () => {
            reject(new BackendError(404, { detail: 'no such entry' }));
          };
        }),
    );
    entry.mark({ mood: null });
    const elsewhere = backend.as(backend.userId).diary;
    await elsewhere.deleteEntry(diary.id, there.id);
    const again = await elsewhere.createEntry(diary.id, today);
    await elsewhere.patchEntry(diary.id, again.id, { mood: 'good' });
    await vi.waitFor(() => {
      expect(entry.entry?.id).toBe(again.id);
    });
    refuse();
    await settle();
    entry.mark({ weather: '비' });
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed).toMatchObject({ weather: '비', mood: 'good' });
    });
    expect(patching).toHaveBeenLastCalledWith(
      diary.id,
      again.id,
      { weather: '비' },
      {
        keepalive: true,
      },
    );
    store.close();
  });

  it('forget a mood taken away and refused for a moment, once its entry goes', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, there.id, { mood: 'bad' });
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    await vi.waitFor(() => {
      expect(entry.entry?.mood).toBe('bad');
    });
    const patching = vi
      .spyOn(backend.diary, 'patchEntry')
      .mockRejectedValueOnce(new BackendError(500, { detail: 'down' }));
    entry.mark({ mood: null });
    await settle();
    const elsewhere = backend.as(backend.userId).diary;
    await elsewhere.deleteEntry(diary.id, there.id);
    await vi.waitFor(() => {
      expect(entry.entry).toBeUndefined();
    });
    const again = await elsewhere.createEntry(diary.id, today);
    await elsewhere.patchEntry(diary.id, again.id, { mood: 'good' });
    await vi.waitFor(() => {
      expect(entry.entry?.id).toBe(again.id);
    });
    entry.mark({ weather: '비' });
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed).toMatchObject({ weather: '비', mood: 'good' });
    });
    // Only the first, refused, ever held the mood taken away.
    expect(patching.mock.calls.filter(([, , patch]) => 'mood' in patch)).toHaveLength(1);
    store.close();
  });

  it('drop only blank keys from a mixed patch on 404, and send the kept keys immediately to the next entry', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, there.id, { mood: 'bad' });
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    await vi.waitFor(() => {
      expect(entry.entry?.mood).toBe('bad');
    });
    let refuse = (): void => undefined;
    const patching = vi.spyOn(backend.diary, 'patchEntry').mockImplementationOnce(
      () =>
        new Promise((_, reject) => {
          refuse = () => {
            reject(new BackendError(404, { detail: 'no such entry' }));
          };
        }),
    );
    // Mixed patch: sets weather AND clears mood in one write.
    entry.mark({ weather: '비', mood: null });
    const elsewhere = backend.as(backend.userId).diary;
    await elsewhere.deleteEntry(diary.id, there.id);
    const again = await elsewhere.createEntry(diary.id, today);
    await elsewhere.patchEntry(diary.id, again.id, { mood: 'good' });
    await vi.waitFor(() => {
      expect(entry.entry?.id).toBe(again.id);
    });
    refuse();
    // Immediately sends `{ weather: '비' }` (without `mood: null`) to `again`, preserving `mood: 'good'`.
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed).toMatchObject({ weather: '비', mood: 'good' });
    });
    expect(patching).toHaveBeenLastCalledWith(
      diary.id,
      again.id,
      { weather: '비' },
      { keepalive: true },
    );
    store.close();
  });

  it('drop only blank keys from mixed waiting notes when an entry is replaced', async () => {
    const { store, backend, diary, today } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, there.id, { mood: 'bad' });
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    await vi.waitFor(() => {
      expect(entry.entry?.mood).toBe('bad');
    });
    vi.spyOn(backend.diary, 'patchEntry').mockRejectedValueOnce(
      new BackendError(500, { detail: 'down' }),
    );
    entry.mark({ weather: '비', mood: null });
    await settle();
    const elsewhere = backend.as(backend.userId).diary;
    await elsewhere.deleteEntry(diary.id, there.id);
    const again = await elsewhere.createEntry(diary.id, today);
    await elsewhere.patchEntry(diary.id, again.id, { mood: 'good' });
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed).toMatchObject({ id: again.id, weather: '비', mood: 'good' });
    });
    store.close();
  });

  it('do not let a permanently refused draft block notes from being sent', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('too big'));
    await vi.waitFor(() => {
      expect(entry.entry).toBeDefined();
    });
    // The draft flush is permanently refused (413), leaving `entry.draft` and `entry.unsaved = true`.
    vi.spyOn(backend.diary, 'patchEntry').mockRejectedValueOnce(
      new BackendError(413, { detail: 'body too large' }),
    );
    entry.mark({ weather: '맑음' }, 'later');
    entry.closeBody();
    await vi.waitFor(() => {
      expect(entry.unsaved).toBe(true);
    });
    // Held notes are sent on their own once the draft is refused, and later marks go right through.
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.weather).toBe('맑음');
    });
    entry.mark({ mood: 'great' });
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed).toMatchObject({ weather: '맑음', mood: 'great' });
    });
    store.close();
  });

  it('write the body after the notes of a day left before its entry was made', async () => {
    const { store, backend, diary, today } = await aPage();
    const entry = store.day(today);
    entry.mark({ weather: '흐림' }, 'later');
    entry.write({
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: '흐린 날' }] }],
    });
    store.close();
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.weather).toBe('흐림');
      expect(JSON.stringify(listed?.body)).toContain('흐린 날');
    });
  });
});

describe('the calendar of a diary', () => {
  it('marks the written days of its month by mood, and follows what is written and deleted', async () => {
    const { store, backend, diary, today } = await aPage();
    await vi.waitFor(() => {
      expect(store.calendar.counted).toBe(true);
    });
    expect(store.calendar.runs).toEqual([]);
    await store.calendar.showMonth(today);
    expect(store.calendar.days).toEqual([]);
    const device = backend.as(backend.userId).diary;
    const made = await device.createEntry(diary.id, today);
    await device.patchEntry(diary.id, made.id, { mood: 'great' });
    await vi.waitFor(() => {
      expect(store.calendar.days).toEqual([{ date: today, entryId: made.id, mood: 'great' }]);
      expect(store.calendar.runs).toEqual([{ from: today, to: today, days: 1 }]);
    });
    await device.deleteEntry(diary.id, made.id);
    await vi.waitFor(() => {
      expect(store.calendar.days).toEqual([]);
      expect(store.calendar.runs).toEqual([]);
    });
    store.close();
  });

  it('counts again what was written while it was first counted', async () => {
    const { store: first, backend, diary, today, workspace } = await aPage();
    // The page's own store reads no more: the reads counted are the new one's.
    first.close();
    const runs = backend.diary.runs.bind(backend.diary);
    let release = (): void => undefined;
    const reading = vi.spyOn(backend.diary, 'runs').mockImplementationOnce(
      (id) =>
        new Promise((resolve, reject) => {
          const answer = runs(id);
          release = () => {
            answer.then(resolve, reject);
          };
        }),
    );
    const store = new DiaryStore(workspace, diary.id);
    await store.show(monthDayOf(today));
    // Written while the first count is on its way, which read the diary before.
    await backend.as(backend.userId).diary.createEntry(diary.id, today);
    await settle();
    release();
    await vi.waitFor(() => {
      expect(store.calendar.runs).toEqual([{ from: today, to: today, days: 1 }]);
    });
    expect(reading).toHaveBeenCalledTimes(2);
    store.close();
  });

  it('counts once what is missed is read, where the first count was refused', async () => {
    const { store: first, backend, diary, today, workspace } = await aPage();
    first.close();
    await backend.diary.createEntry(diary.id, today);
    const reading = vi
      .spyOn(backend.diary, 'runs')
      .mockRejectedValueOnce(new BackendError(500, { detail: 'down' }));
    const store = new DiaryStore(workspace, diary.id);
    await store.show(monthDayOf(today));
    await settle();
    expect(store.calendar.counted).toBe(false);
    // What is heard meanwhile (every key typed in a note) does not read the runs again.
    store.calendar.heard(await backend.diary.entriesOn(diary.id, monthDayOf(today)));
    await settle();
    expect(reading).toHaveBeenCalledTimes(1);
    store.calendar.resync();
    await vi.waitFor(() => {
      expect(store.calendar.runs).toEqual([{ from: today, to: today, days: 1 }]);
    });
    store.close();
  });

  it('waits for resync when a later recount is refused, instead of asking on every edit', async () => {
    const { store: first, backend, diary, today, workspace } = await aPage();
    first.close();
    const store = new DiaryStore(workspace, diary.id);
    const reading = vi.spyOn(backend.diary, 'runs');
    await store.show(monthDayOf(today));
    await vi.waitFor(() => {
      expect(store.calendar.counted).toBe(true);
    });
    expect(reading).toHaveBeenCalledTimes(1);
    // Next recount (triggered by a new entry) is refused with 500.
    reading.mockRejectedValueOnce(new BackendError(500, { detail: 'down' }));
    const made = await backend.as(backend.userId).diary.createEntry(diary.id, today);
    await vi.waitFor(() => {
      expect(reading).toHaveBeenCalledTimes(2);
    });
    await settle();
    // Repeated edits on that day do not re-trigger `runs` while refused.
    store.calendar.heard([made]);
    store.calendar.heard([made]);
    await settle();
    expect(reading).toHaveBeenCalledTimes(2);
    // Resync retries and clears the refused state.
    store.calendar.resync();
    await vi.waitFor(() => {
      expect(store.calendar.runs).toEqual([{ from: today, to: today, days: 1 }]);
    });
    expect(reading).toHaveBeenCalledTimes(3);
    store.close();
  });

  it('still retries after a failing in-flight count when resync arrived meanwhile', async () => {
    const { store: first, backend, diary, today, workspace } = await aPage();
    first.close();
    await backend.diary.createEntry(diary.id, today);
    let rejectInFlight = (): void => undefined;
    const reading = vi.spyOn(backend.diary, 'runs').mockImplementationOnce(
      () =>
        new Promise((_, reject) => {
          rejectInFlight = () => {
            reject(new BackendError(500, { detail: 'connection dropped' }));
          };
        }),
    );
    const store = new DiaryStore(workspace, diary.id);
    await store.show(monthDayOf(today));
    expect(reading).toHaveBeenCalledTimes(1);
    // Resync arrives while the first (doomed) count is still in flight.
    store.calendar.resync();
    rejectInFlight();
    await vi.waitFor(() => {
      expect(store.calendar.runs).toEqual([{ from: today, to: today, days: 1 }]);
    });
    expect(reading).toHaveBeenCalledTimes(2);
    store.close();
  });

  it('is each diary’s own', async () => {
    const { store, backend, ownId, today, workspace } = await aPage();
    const other = await backend.diary.createDiary(ownId, { title: 'Runs' });
    workspace.receiveContainer(other);
    const yesterday = addDays(today, -1);
    await backend.diary.createEntry(other.id, yesterday);
    await backend.diary.createEntry(other.id, today);
    const second = new DiaryStore(workspace, other.id);
    await second.show(monthDayOf(today));
    await vi.waitFor(() => {
      expect(second.calendar.runs).toEqual([{ from: yesterday, to: today, days: 2 }]);
      expect(store.calendar.counted).toBe(true);
    });
    expect(store.calendar.runs).toEqual([]);
    await store.calendar.showMonth(today);
    expect(store.calendar.days).toEqual([]);
    store.close();
    second.close();
  });
});
