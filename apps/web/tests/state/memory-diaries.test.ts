/**
 * The diaries of the backend in memory keep the server's rules
 * (`crates/store/src/diaries.rs`): diaries are in the reader's own space
 * alone, one is made at most where none is asked for twice, an entry is one
 * a day, its days are listed the latest first, and a stale body is refused.
 *
 * Over them the stores of a diary's page (`DiaryStore`, `EntryStore`): an
 * entry is made by what is written first, once, and an entry emptied here is
 * deleted as it is left.
 */
import { describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { dateOfUtc, type CalendarDate } from '$lib/logic/dates.ts';
import { monthDayOf, parseMonthDay, type MonthDay } from '$lib/logic/diary-days.ts';
import { BackendError, DiaryStore, firstDiary, type Change } from '$lib/state/index.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';
import { DIARIES_PER_SPACE } from '$lib/state/memory/diaries.ts';
import { FIELD } from '$lib/state/memory/rooms.ts';

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

const day = (text: string): MonthDay => {
  const parsed = parseMonthDay(text);
  if (!parsed) throw new Error(`not a day of the year: ${text}`);
  return parsed;
};

/** A diary in the reader's own space, and the world's today. */
async function aDiary() {
  const made = setup();
  const ownId = await ownSpace(made.backend);
  const diaries = made.backend.diary;
  const diary = await diaries.createDiary(ownId, { title: 'Days' });
  const today = dateOfUtc(made.world.now());
  return { ...made, ownId, diaries, diary, today };
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

describe('diaries in memory', () => {
  it('are in the reader’s own space alone: anywhere else is not there', async () => {
    const { backend, world } = setup();
    const shared = world.workspace.id;
    expect(await statusOf(backend.diary.diaries(shared))).toBe(404);
    expect(await statusOf(backend.diary.createDiary(shared, {}))).toBe(404);
    expect(await backend.diary.diaries(await ownSpace(backend))).toEqual([]);
  });

  it('are made once where none is asked for twice, and no more than the limit', async () => {
    const { backend } = setup();
    const own = await ownSpace(backend);
    const first = await backend.diary.createDiary(own, { ifNone: true });
    const again = await backend.diary.createDiary(own, { ifNone: true });
    expect(again.id).toBe(first.id);
    expect(first).toMatchObject({ kind: 'diary', visibility: 'private', workspaceId: own });
    for (let made = 1; made < DIARIES_PER_SPACE; made += 1) {
      await backend.diary.createDiary(own, {});
    }
    const refused = await statusOf(backend.diary.createDiary(own, {}));
    expect(refused).toBe(400);
    expect(await backend.diary.diaries(own)).toHaveLength(DIARIES_PER_SPACE);
  });

  it('are among the diaries, not the boards or their search', async () => {
    const { backend, ownId, diary } = await aDiary();
    expect((await backend.diary.diaries(ownId)).map((one) => one.id)).toEqual([diary.id]);
    expect((await backend.boards(ownId)).some((board) => board.id === diary.id)).toBe(false);
    expect(await backend.searchBoards(ownId, 'Days')).toEqual([]);
  });

  it('are another person’s to nobody else', async () => {
    const { other, diary } = await aDiary();
    expect(await statusOf(other.diary.entriesOn(diary.id, day('11-21')))).toBe(404);
  });

  it('have one entry a day, the days of the year listed the latest first', async () => {
    const { diaries, diary, today } = await aDiary();
    const monthDay = monthDayOf(today);
    const year = Number(today.slice(0, 4));
    const earlier = `${String(year - 2)}-${monthDay}` as CalendarDate;
    const last = `${String(year - 1)}-${monthDay}` as CalendarDate;
    const made = await diaries.createEntry(diary.id, earlier);
    expect((await diaries.createEntry(diary.id, earlier)).id).toBe(made.id);
    await diaries.createEntry(diary.id, last);
    await diaries.createEntry(diary.id, today);
    const listed = await diaries.entriesOn(diary.id, monthDay);
    expect(listed.map((entry) => entry.date)).toEqual(
      monthDay === '02-29' ? [today] : [today, last, earlier],
    );
  });

  it('take no day more than two days past the world’s', async () => {
    const { diaries, diary } = await aDiary();
    expect(await statusOf(diaries.createEntry(diary.id, '9999-01-01' as CalendarDate))).toBe(400);
    expect(await statusOf(diaries.createEntry(diary.id, '2025-02-30' as CalendarDate))).toBe(400);
  });

  it('refuse a body read before the entry last changed, and delete into the trash', async () => {
    const { backend, diaries, diary, today, world } = await aDiary();
    const told: Change[] = [];
    backend.watchContainer(diary.id, (change) => told.push(change));
    const entry = await diaries.createEntry(diary.id, today);
    const made = world.now();
    world.useClock(() => made + 1000);
    const written = await diaries.patchEntry(diary.id, entry.id, {
      body: text('first'),
      readAt: entry.updatedAt,
    });
    expect(written.body).toEqual(text('first'));
    const stale = diaries.patchEntry(diary.id, entry.id, {
      body: text('late'),
      readAt: entry.updatedAt,
    });
    expect(await statusOf(stale)).toBe(409);
    await diaries.deleteEntry(diary.id, entry.id);
    await settle();
    expect(await diaries.entriesOn(diary.id, monthDayOf(today))).toEqual([]);
    expect(told.some((change) => change.kind === 'entries')).toBe(true);
    expect(told.at(-1)).toMatchObject({ kind: 'blocksDeleted', blockIds: [entry.id] });
  });
});

describe('a diary’s page', () => {
  it('is in the sidebar of the own space, and nowhere in the shared workspace', async () => {
    const { workspace, diary, world } = await aPage();
    expect(workspace.containers.some((one) => one.id === diary.id)).toBe(true);
    await workspace.enter(world.workspace.id);
    expect(workspace.containers.some((one) => one.kind === 'diary')).toBe(false);
  });

  it('makes no entry for a day before something is written, then one, once', async () => {
    const { store, backend, diary, today } = await aPage();
    const making = vi.spyOn(backend.diary, 'createEntry');
    const entry = store.day(today);
    expect(entry.entry).toBeUndefined();
    entry.write(null);
    expect(making).not.toHaveBeenCalled();
    entry.write(text('h'));
    entry.write(text('he'));
    entry.write(text('hello'));
    await vi.waitFor(() => {
      expect(entry.entry).toBeDefined();
    });
    await settle();
    expect(making).toHaveBeenCalledTimes(1);
    // Nothing opened the room: what was written is the body.
    const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
    expect(listed?.body).toEqual(text('hello'));
    expect(store.entries.map((one) => one.id)).toEqual([listed?.id]);
    store.close();
  });

  it('gives the draft to the room once it is open, and keeps what was written there', async () => {
    const { store, today, diary, backend } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('kept'));
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    expect(entry.takeDraft()).toEqual(text('kept'));
    expect(entry.takeDraft()).toBeNull();
    const doc = entry.room?.doc;
    if (!doc) throw new Error('no document');
    const paragraph = new Y.XmlElement('paragraph');
    paragraph.insert(0, [new Y.XmlText('kept')]);
    doc.getXmlFragment(FIELD).insert(0, [paragraph]);
    entry.noteBlank(false);
    entry.closeBody();
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(JSON.stringify(listed?.body)).toContain('kept');
    });
    store.close();
  });

  it('deletes an entry the reader emptied here as it is left', async () => {
    const { store, today, diary, backend } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('x'));
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    entry.takeDraft();
    const doc = entry.room?.doc;
    if (!doc) throw new Error('no document');
    const fragment = doc.getXmlFragment(FIELD);
    const paragraph = new Y.XmlElement('paragraph');
    paragraph.insert(0, [new Y.XmlText('x')]);
    fragment.insert(0, [paragraph]);
    fragment.delete(0, fragment.length);
    fragment.insert(0, [new Y.XmlElement('paragraph')]);
    entry.noteBlank(true);
    entry.closeBody();
    await vi.waitFor(async () => {
      expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toEqual([]);
    });
    await vi.waitFor(() => {
      expect(store.entries).toEqual([]);
    });
    store.close();
  });

  it('keeps an entry opened and left untouched, empty or not', async () => {
    const { store, today, diary, backend } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    expect(entry.entry?.id).toBe(there.id);
    entry.openBody();
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    entry.noteBlank(true);
    entry.closeBody();
    await settle();
    expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toHaveLength(1);
    store.close();
  });

  it('hears an entry another device made on the day shown', async () => {
    const { store, today, diary, backend } = await aPage();
    const device = backend.as(backend.userId);
    await device.diary.createEntry(diary.id, today);
    await vi.waitFor(() => {
      expect(store.entries.map((entry) => entry.date)).toEqual([today]);
    });
    store.close();
  });
  it('writes what was drafted as the body when the day is left before its entry is made', async () => {
    const { store, today, diary, backend } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('quick'));
    // The page goes (another day, another board) while the entry is still being made.
    store.close();
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.body).toEqual(text('quick'));
    });
  });

  it('puts a draft after what another device wrote, never in its place', async () => {
    const { store, today, diary, backend } = await aPage();
    const device = backend.as(backend.userId);
    const theirs = await device.diary.createEntry(diary.id, today);
    await device.diary.patchEntry(diary.id, theirs.id, {
      body: text('theirs'),
      readAt: theirs.updatedAt,
    });
    await vi.waitFor(() => {
      expect(store.entries[0]?.body).toEqual(text('theirs'));
    });
    const entry = store.day(today);
    entry.write(text('mine'));
    entry.closeBody();
    await vi.waitFor(async () => {
      const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
      expect(listed?.body).toEqual({
        type: 'doc',
        content: [...(text('theirs').content ?? []), ...(text('mine').content ?? [])],
      });
    });
    store.close();
  });

  it('keeps a draft whose write was refused, to give it again', async () => {
    const { store, today, backend } = await aPage();
    backend.diary.failNext('patchEntry', new BackendError(500, {}));
    const entry = store.day(today);
    entry.write(text('kept'));
    await vi.waitFor(() => {
      expect(entry.unsaved).toBe(true);
    });
    expect(entry.draft).toEqual(text('kept'));
    expect(entry.takeDraft()).toEqual(text('kept'));
    store.close();
  });

  it('deletes an entry made here and left blank, nothing typed in its room', async () => {
    const { store, today, diary, backend } = await aPage();
    const entry = store.day(today);
    entry.openBody();
    entry.write(text('x'));
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    // The editor of the room took the draft and was emptied before anything reached the room.
    entry.takeDraft();
    entry.noteBlank(true);
    entry.closeBody();
    await vi.waitFor(async () => {
      expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toEqual([]);
    });
    store.close();
  });

  it('keeps an emptied entry while somebody else is in its room', async () => {
    const { store, today, diary, backend, ownId } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    entry.openBody();
    const device = backend.as(backend.userId).openDoc(ownId, diary.id, there.id, {
      state: () => undefined,
      discarded: () => undefined,
    });
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
      expect(device.state.status).toBe('joined');
    });
    const theirs = device.state.doc;
    if (!theirs) throw new Error('no document');
    const place = Y.createRelativePositionFromTypeIndex(theirs.getXmlFragment(FIELD), 0);
    device.state.carets?.setLocalStateField('user', {});
    device.state.carets?.setLocalStateField('cursor', { anchor: place, head: place });
    await vi.waitFor(() => {
      expect(entry.room?.carets?.getStates().size).toBe(2);
    });
    entry.write(text('x'));
    entry.noteBlank(true);
    entry.takeDraft();
    // Emptied here: but not alone.
    const doc = entry.room?.doc;
    if (!doc) throw new Error('no document');
    doc.getXmlFragment(FIELD).insert(0, [new Y.XmlElement('paragraph')]);
    entry.closeBody();
    await settle();
    device.close();
    expect(await backend.diary.entriesOn(diary.id, monthDayOf(today))).toHaveLength(1);
    store.close();
  });

  it('reads an entry open here again as its room writes it: folded, it shows what was written', async () => {
    const { store, today, diary, backend } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    entry.openBody();
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    await backend.as(backend.userId).diary.patchEntry(diary.id, there.id, {
      body: text('written'),
      readAt: there.updatedAt,
    });
    await vi.waitFor(() => {
      expect(store.entries[0]?.body).toEqual(text('written'));
      expect(entry.entry?.body).toEqual(text('written'));
    });
    store.close();
  });

  it('keeps what is being written on a day whose entry was deleted elsewhere, and makes another with it', async () => {
    const { store, today, diary, backend } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    entry.openBody();
    // Written before the room took it (its editor has not come yet).
    entry.write(text('again'));
    await backend.as(backend.userId).diary.deleteEntry(diary.id, there.id);
    await vi.waitFor(() => {
      expect(entry.entry?.id).toBeDefined();
      expect(entry.entry?.id).not.toBe(there.id);
    });
    expect(entry.draft).toEqual(text('again'));
    store.close();
  });

  it('neither gives a draft being written to the room, nor deletes the entry it is written into', async () => {
    const { store, today, diary, backend } = await aPage();
    let open = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      open = resolve;
    });
    const patch = backend.diary.patchEntry.bind(backend.diary);
    vi.spyOn(backend.diary, 'patchEntry').mockImplementation(
      async (...args: Parameters<typeof patch>) => {
        await gate;
        return patch(...args);
      },
    );
    const entry = store.day(today);
    // Written and folded before the entry was made: the draft goes as its body.
    entry.write(text('kept'));
    await vi.waitFor(() => {
      expect(entry.flushing).toBe(true);
    });
    // Opened again meanwhile, its room's editor still empty, and folded again.
    entry.openBody();
    await vi.waitFor(() => {
      expect(entry.room?.status).toBe('joined');
    });
    expect(entry.takeDraft()).toBeNull();
    entry.noteBlank(true);
    entry.closeBody();
    open();
    await vi.waitFor(() => {
      expect(entry.flushing).toBe(false);
    });
    await settle();
    const [listed] = await backend.diary.entriesOn(diary.id, monthDayOf(today));
    expect(listed?.body).toEqual(text('kept'));
    store.close();
  });

  it('asks again for an entry it could not make, with nothing more typed', async () => {
    const { store, today, backend } = await aPage();
    backend.diary.failNext('createEntry', new BackendError(503, {}));
    const entry = store.day(today);
    entry.write(text('later'));
    await vi.waitFor(() => {
      expect(entry.unsaved).toBe(true);
    });
    await vi.waitFor(
      () => {
        expect(entry.entry).toBeDefined();
        expect(entry.unsaved).toBe(false);
      },
      { timeout: 8000 },
    );
    store.close();
  }, 10_000);

  it('asks no more by itself for an entry refused for good, and says so once', async () => {
    const { store, today, backend, workspace } = await aPage();
    const said = workspace.problems.length;
    backend.diary.failNext('createEntry', new BackendError(400, {}));
    const asked = vi.spyOn(backend.diary, 'createEntry');
    const entry = store.day(today);
    entry.write(text('refused'));
    await vi.waitFor(() => {
      expect(entry.unsaved).toBe(true);
    });
    await new Promise((resolve) => setTimeout(resolve, 6000));
    expect(asked).toHaveBeenCalledTimes(1);
    expect(workspace.problems.length - said).toBe(1);
    store.close();
  }, 10_000);

  it('drops what it read of an entry deleted before the answer came', async () => {
    const { store, today, diary, backend } = await aPage();
    const there = await backend.diary.createEntry(diary.id, today);
    await store.show(monthDayOf(today));
    let open = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      open = resolve;
    });
    const read = backend.diary.entry.bind(backend.diary);
    const asked = vi
      .spyOn(backend.diary, 'entry')
      .mockImplementation(async (...args: Parameters<typeof read>) => {
        const answer = await read(...args);
        await gate;
        return answer;
      });
    const device = backend.as(backend.userId).diary;
    await device.patchEntry(diary.id, there.id, { body: text('then'), readAt: there.updatedAt });
    await vi.waitFor(() => {
      expect(asked).toHaveBeenCalled();
    });
    await device.deleteEntry(diary.id, there.id);
    await vi.waitFor(() => {
      expect(store.entries).toEqual([]);
    });
    open();
    await settle();
    expect(store.entries).toEqual([]);
    store.close();
  });
});
