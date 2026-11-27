/**
 * Diary contract guarantees on the web boundary (`diary-2-t2`):
 * - manual body versions (`keepDiaryEntryBodyVersion`, `entryVersions`, `VersionsReading`);
 * - `readAt` compares against the body's change timestamp so metadata-only patches never
 *   cause spurious 409 conflicts while actual body edits still conflict;
 * - `appendBody` appends atomically after stored/room text, rejects `body` + `appendBody`,
 *   enforces combined body limits, and powers `EntryStore` draft flush / `pagehide`;
 * - deleting a completely empty entry purges it directly without piling empty shells in trash,
 *   while entries with body or metadata still enter trash.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import {
  entryVersions,
  keepVersion,
  VersionsReading,
} from '$lib/features/card/body-versions.svelte.ts';
import { MAX_NODES, type DocNode } from '$lib/editor/schema.ts';
import { dateOfUtc } from '$lib/logic/dates.ts';
import { monthDayOf } from '$lib/logic/diary-days.ts';
import { BackendError, DiaryStore, firstDiary } from '$lib/state/index.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';
import { BY_HAND_FLOOR } from '$lib/state/memory/body-versions.ts';
import { FIELD } from '$lib/state/memory/rooms.ts';
import { LIMITS } from '$lib/state/memory/shapes.ts';

import { serverBackend, settle, setup, text } from './support.ts';

async function ownSpace(backend: MemoryBackend): Promise<string> {
  const own = (await backend.workspaces()).find((workspace) => workspace.kind === 'personal');
  if (!own) throw new Error('no own space');
  return own.id;
}

async function aDiary() {
  const made = setup();
  const ownId = await ownSpace(made.backend);
  const diaries = made.backend.diary;
  const diary = await diaries.createDiary(ownId, { title: 'Days' });
  const today = dateOfUtc(made.world.now());
  return { ...made, ownId, diaries, diary, today };
}

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

async function rejection(promise: Promise<unknown>): Promise<BackendError> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('expected promise to reject');
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('diary entry manual body versions', () => {
  it('keeps, lists, reads, and restores entry body versions with cooldown and isolation', async () => {
    const { world, backend, other, diaries, diary, today } = await aDiary();
    let offset = 0;
    const base = world.now();
    world.useClock(() => base + offset);

    const entry = await diaries.createEntry(diary.id, today);
    const body = backend.openDoc(diary.workspaceId, diary.id, entry.id, {
      state: () => undefined,
      discarded: () => undefined,
    });
    await vi.waitFor(() => {
      expect(body.state.status).toBe('joined');
    });
    const doc = body.state.doc;
    if (!doc) throw new Error('joined without document');
    const appendLine = async (line: string): Promise<void> => {
      const paragraph = new Y.XmlElement('paragraph');
      paragraph.insert(0, [new Y.XmlText(line)]);
      const fragment = doc.getXmlFragment(FIELD);
      fragment.insert(fragment.length, [paragraph]);
      await body.settle();
    };

    // Empty entry has nothing to keep yet.
    expect(await diaries.bodyVersions.keep(diary.id, entry.id)).toEqual({
      outcome: 'unchanged',
      version: null,
    });

    await appendLine('first draft');
    const kept = await diaries.bodyVersions.keep(diary.id, entry.id);
    expect(kept).toMatchObject({
      outcome: 'kept',
      version: { version: 1, cause: 'manual', editors: [backend.userId], isCurrent: true },
    });
    expect(await diaries.bodyVersions.keep(diary.id, entry.id)).toMatchObject({
      outcome: 'unchanged',
      version: { version: 1 },
    });

    await appendLine('second line');
    expect(await diaries.bodyVersions.keep(diary.id, entry.id)).toMatchObject({
      outcome: 'recent',
      version: { version: 1, cause: 'manual' },
    });

    offset += BY_HAND_FLOOR + 1000;
    const second = await diaries.bodyVersions.keep(diary.id, entry.id);
    expect(second).toMatchObject({
      outcome: 'kept',
      version: { version: 2, cause: 'manual', isCurrent: true },
    });

    const v1 = await diaries.bodyVersions.read(diary.id, entry.id, 1);
    expect(JSON.stringify(v1.body)).toContain('first draft');
    expect(JSON.stringify(v1.body)).not.toContain('second line');

    await diaries.bodyVersions.restore(diary.id, entry.id, 1);
    const restored = await diaries.entry(diary.id, entry.id);
    expect(JSON.stringify(restored.body)).toContain('first draft');
    expect(JSON.stringify(restored.body)).not.toContain('second line');
    expect((await diaries.bodyVersions.list(diary.id, entry.id)).map((v) => v.cause)).toEqual([
      'restored',
      'manual',
      'manual',
    ]);

    // Another user cannot see or touch the diary's versions.
    expect((await rejection(other.diary.bodyVersions.list(diary.id, entry.id))).status).toBe(404);
    expect((await rejection(other.diary.bodyVersions.keep(diary.id, entry.id))).status).toBe(404);
    body.close();
  });

  it('routes ServerDiaries.bodyVersions to diary entry body-version endpoints', async () => {
    const answer = { outcome: 'kept', version: null };
    const sent: string[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      sent.push(`${init.method ?? 'GET'} ${url}`);
      return Promise.resolve(
        new Response(JSON.stringify(answer), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    const backend = serverBackend('');
    expect(await backend.diary.bodyVersions.keep('d1', 'e1')).toEqual(answer);
    expect(sent).toEqual(['POST /api/v1/w/w1/diaries/d1/entries/e1/body-versions']);
  });

  it('connects entryVersions and VersionsReading to an open EntryStore', async () => {
    const { store, backend, diary, today } = await aPage();
    const created = await backend.diary.createEntry(diary.id, today);
    await backend.diary.patchEntry(diary.id, created.id, {
      body: text('morning walk'),
      readAt: created.updatedAt,
    });
    await store.show(monthDayOf(today));
    const entry = store.day(today);
    entry.openBody();
    await vi.waitFor(() => {
      expect(entry.room?.writable).toBe(true);
    });
    const source = entryVersions(entry);
    // Already captured as version 1 by patchEntry, so keep without edits is unchanged.
    expect(await keepVersion(source)).toBe('unchanged');

    const doc = entry.room?.doc;
    if (!doc) throw new Error('no room doc');
    const paragraph = new Y.XmlElement('paragraph');
    paragraph.insert(0, [new Y.XmlText('afternoon coffee')]);
    const fragment = doc.getXmlFragment(FIELD);
    fragment.insert(fragment.length, [paragraph]);

    const reading = new VersionsReading(source);
    await vi.waitFor(() => {
      expect(reading.status).toBe('ready');
    });
    await reading.keep();
    expect(reading.kept).toBe('kept');
    expect(reading.versions[0]).toMatchObject({ cause: 'manual', isCurrent: true });

    // While an unadopted draft is still held (e.g., IME composition), keepVersion waits.
    entry.write(text('still composing'));
    expect(entry.drafting).toBe(true);
    expect(source.writable()).toBe(false);
    expect(source.opening()).toBe(true);
    expect(await keepVersion(source)).toBe('opening');
    entry.takeDraft();

    entry.closeBody();
    store.close();
  });
});

describe('diary readAt body timestamp comparison', () => {
  it('allows conditional body writes across metadata-only patches but rejects body edits', async () => {
    const { diaries, diary, today, world } = await aDiary();
    const entry = await diaries.createEntry(diary.id, today);
    const first = await diaries.patchEntry(diary.id, entry.id, {
      body: text('first body'),
      readAt: entry.updatedAt,
    });

    const t1 = world.now();
    world.useClock(() => t1 + 1000);
    const withMeta = await diaries.patchEntry(diary.id, entry.id, {
      mood: 'great',
      weather: 'sunny',
      place: 'park',
    });
    expect(withMeta.updatedAt).toBeGreaterThan(first.updatedAt);

    // Writing body with the pre-metadata readAt succeeds because the body did not change.
    const second = await diaries.patchEntry(diary.id, entry.id, {
      body: text('second body'),
      readAt: first.updatedAt,
    });
    expect(second.body).toEqual(text('second body'));
    expect(second.mood).toBe('great');

    // A stale readAt from before the body change is rejected with 409.
    const stale = await rejection(
      diaries.patchEntry(diary.id, entry.id, {
        body: text('stale write'),
        readAt: first.updatedAt,
      }),
    );
    expect(stale.status).toBe(409);

    // Another metadata-only edit still allows writing with either second.updatedAt or afterMeta.updatedAt.
    const afterMeta = await diaries.patchEntry(diary.id, entry.id, { weather: 'cloudy' });
    const third = await diaries.patchEntry(diary.id, entry.id, {
      body: text('third body'),
      readAt: afterMeta.updatedAt,
    });
    expect(third.body).toEqual(text('third body'));
  });
});

describe('diary appendBody and empty-entry trash cleanup', () => {
  it('appends atomically, rejects body + appendBody, and enforces body limits', async () => {
    const { diaries, diary, today } = await aDiary();
    const entry = await diaries.createEntry(diary.id, today);

    const first = await diaries.patchEntry(diary.id, entry.id, {
      appendBody: text('part one'),
      weather: 'rain',
    });
    expect(first.body).toEqual(text('part one'));
    expect(first.weather).toBe('rain');

    const second = await diaries.patchEntry(diary.id, entry.id, {
      appendBody: text('part two'),
    });
    expect(second.body).toEqual({
      type: 'doc',
      content: [...(text('part one').content ?? []), ...(text('part two').content ?? [])],
    });

    // Sending both body and appendBody is refused with 400 on appendBody.
    const both = await rejection(
      diaries.patchEntry(diary.id, entry.id, {
        body: text('replace'),
        appendBody: text('extra'),
        readAt: second.updatedAt,
      }),
    );
    expect(both.status).toBe(400);
    expect(both.errors[0]?.pointer).toBe('/appendBody');

    // Combined body exceeding MAX_BODY_BYTES is refused on appendBody without modifying entry.
    const huge: DocNode = {
      type: 'doc',
      content: [
        { type: 'paragraph', content: [{ type: 'text', text: 'a'.repeat(LIMITS.bodyBytes) }] },
      ],
    };
    const tooBig = await rejection(diaries.patchEntry(diary.id, entry.id, { appendBody: huge }));
    expect(tooBig.status).toBe(400);
    expect(tooBig.errors[0]?.pointer).toBe('/appendBody');

    // Combined body exceeding MAX_NODES is refused on appendBody.
    const manyParagraphs: DocNode = {
      type: 'doc',
      content: Array.from({ length: MAX_NODES }, () => ({ type: 'paragraph' })),
    };
    const tooManyNodes = await rejection(
      diaries.patchEntry(diary.id, entry.id, { appendBody: manyParagraphs }),
    );
    expect(tooManyNodes.status).toBe(400);
    expect(tooManyNodes.errors[0]?.pointer).toBe('/appendBody');
    expect((await diaries.entry(diary.id, entry.id)).body).toEqual(second.body);
  });

  it('flushes EntryStore drafts on leave with a single keepalive appendBody patch', async () => {
    const { store, backend, diary, today } = await aPage();
    const device = backend.as(backend.userId);
    const theirs = await device.diary.createEntry(diary.id, today);
    await device.diary.patchEntry(diary.id, theirs.id, {
      body: text('from laptop'),
      readAt: theirs.updatedAt,
    });
    await vi.waitFor(() => {
      expect(store.entries[0]?.body).toEqual(text('from laptop'));
    });

    const entry = store.day(today);
    // Simulate a second remote edit the page has not seen yet before pagehide.
    await device.diary.patchEntry(diary.id, theirs.id, { appendBody: text('later edit') });
    const patching = vi.spyOn(backend.diary, 'patchEntry');
    entry.write(text('offline draft'));
    store.leave(false);

    expect(patching).toHaveBeenCalledTimes(1);
    expect(patching).toHaveBeenCalledWith(
      diary.id,
      theirs.id,
      { appendBody: text('offline draft') },
      { keepalive: true },
    );
    await vi.waitFor(async () => {
      const stored = await backend.diary.entry(diary.id, theirs.id);
      expect(stored.body).toEqual({
        type: 'doc',
        content: [
          ...(text('from laptop').content ?? []),
          ...(text('later edit').content ?? []),
          ...(text('offline draft').content ?? []),
        ],
      });
    });
    store.close();
  });

  it('purges completely empty entries directly while moving non-empty entries to trash', async () => {
    const { world, diaries, diary, today } = await aDiary();
    const empty = await diaries.createEntry(diary.id, today);
    await diaries.deleteEntry(diary.id, empty.id);
    await settle();
    expect(world.blocks.has(empty.id)).toBe(false);
    expect(world.deletedBlocks.has(empty.id)).toBe(false);

    // An entry with metadata only still moves to trash.
    const noted = await diaries.createEntry(diary.id, today);
    await diaries.patchEntry(diary.id, noted.id, { mood: 'good' });
    await diaries.deleteEntry(diary.id, noted.id);
    await settle();
    expect(world.deletedBlocks.has(noted.id)).toBe(true);

    // An entry with a body moves to trash too.
    const written = await diaries.createEntry(diary.id, today);
    await diaries.patchEntry(diary.id, written.id, { appendBody: text('worth keeping') });
    await diaries.deleteEntry(diary.id, written.id);
    await settle();
    expect(world.deletedBlocks.has(written.id)).toBe(true);
  });
});
