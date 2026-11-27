/** A diary's entries in the trash, and bringing one back to its day (`entryTrash`), in memory. */
import { describe, expect, it } from 'vitest';

import { dateOfUtc } from '$lib/logic/dates.ts';
import { monthDayOf } from '$lib/logic/diary-days.ts';
import { DiaryStore, entryTrash, firstDiary } from '$lib/state/index.ts';

import { settle, setup } from './support.ts';

async function aDiary() {
  const made = setup();
  const own = (await made.backend.workspaces()).find((each) => each.kind === 'personal');
  if (!own) throw new Error('no own space');
  await made.workspace.load(own.id);
  const diary = await firstDiary(made.workspace);
  if (!diary) throw new Error('no diary');
  const store = new DiaryStore(made.workspace, diary.id);
  const today = dateOfUtc(made.world.now());
  await store.show(monthDayOf(today));
  return { ...made, diary, store, today };
}

describe('the trash of a diary', () => {
  it('lists a deleted entry by its day and mood, and brings it back to its day', async () => {
    const { backend, diary, store, today } = await aDiary();
    const entry = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, entry.id, { mood: 'good' });
    await backend.diary.deleteEntry(diary.id, entry.id);
    await settle();

    const trash = entryTrash(store);
    await trash.read();
    expect(trash.items).toMatchObject([{ id: entry.id, title: today, mood: 'good' }]);
    expect(await trash.restore(entry.id)).toBe(true);
    expect(trash.items).toEqual([]);
    expect(store.entries.map((one) => one.id)).toEqual([entry.id]);
    expect(await backend.diary.deletedEntries(diary.id)).toEqual([]);
    store.close();
  });

  it('refuses one whose day has an entry since, and keeps it in the trash', async () => {
    const { backend, diary, store, today, workspace } = await aDiary();
    const first = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, first.id, { weather: 'rain' });
    await backend.diary.deleteEntry(diary.id, first.id);
    await backend.diary.createEntry(diary.id, today);

    await expect(backend.diary.restoreEntry(diary.id, first.id)).rejects.toMatchObject({
      status: 409,
      reason: 'day_taken',
    });

    const trash = entryTrash(store);
    await trash.read();
    expect(await trash.restore(first.id)).toBe(false);
    expect(trash.items.map((item) => item.id)).toEqual([first.id]);
    expect(workspace.problems.at(-1)?.error).toMatchObject({ reason: 'day_taken' });
    store.close();
  });
});
