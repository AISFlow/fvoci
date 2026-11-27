/**
 * What a version being read is compared with (version-compare.svelte.ts):
 * the version kept before it whatever its number, the body now, another
 * version, or nothing; the choice of the first two kept on the device and
 * another version not; a comparison answered late not taken; a body that
 * cannot be read or shown said so. And through the reading of an open
 * card's versions on the backend in memory.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { DocNode } from '$lib/editor/schema.ts';
import { cardVersions, VersionsReading } from '$lib/features/card/body-versions.svelte.ts';
import {
  targetOf,
  VersionComparing,
  type CompareSource,
  type ComparedReading,
} from '$lib/features/card/version-compare.svelte.ts';
import type { BodyVersion, BodyVersionContent } from '$lib/state/index.ts';

import { openFirstBoard, text } from './support.ts';

const version = (number: number): BodyVersion => ({
  version: number,
  keptAt: number * 1000,
  editors: [],
  isCurrent: false,
  cause: 'auto',
});

/** The newest first, with gaps: the server let 4 and 1 go. */
const KEPT = [5, 3, 2].map(version);
const [FIFTH, THIRD, SECOND] = KEPT as [BodyVersion, BodyVersion, BodyVersion];
const NONE = { kind: 'previous', whole: false, pinned: undefined } as const;

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe('what a version is compared with', () => {
  it('is the version kept before it, whatever its number; the oldest has none', () => {
    expect(targetOf(FIFTH, KEPT, NONE)).toEqual({ kind: 'previous', version: THIRD });
    expect(targetOf(THIRD, KEPT, NONE)).toEqual({ kind: 'previous', version: SECOND });
    expect(targetOf(SECOND, KEPT, NONE)).toEqual({ kind: 'oldest' });
  });

  it('is the body now, another version, or nothing, as chosen', () => {
    expect(targetOf(SECOND, KEPT, { ...NONE, kind: 'current' })).toEqual({ kind: 'current' });
    expect(targetOf(FIFTH, KEPT, { ...NONE, whole: true, pinned: 2 })).toEqual({ kind: 'whole' });
    // Another version, older or newer.
    expect(targetOf(FIFTH, KEPT, { ...NONE, pinned: 2 })).toEqual({
      kind: 'version',
      version: SECOND,
    });
    expect(targetOf(SECOND, KEPT, { ...NONE, pinned: 5 })).toEqual({
      kind: 'version',
      version: FIFTH,
    });
  });

  it('gives way to the kind chosen when the version pinned is the one read or is no longer kept', () => {
    expect(targetOf(THIRD, KEPT, { ...NONE, pinned: 3 })).toEqual({
      kind: 'previous',
      version: SECOND,
    });
    expect(targetOf(THIRD, KEPT, { kind: 'current', whole: false, pinned: 4 })).toEqual({
      kind: 'current',
    });
  });
});

const paragraph = (value: string): DocNode => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text: value }] }],
});
const content = (body: DocNode | null): BodyVersionContent => ({ ...FIFTH, body });

/** A comparison of the fifth version, whose body is `body`, with `source` to read the others from. */
function comparing(body: DocNode | null, source: Partial<CompareSource> = {}): VersionComparing {
  const reading: ComparedReading = { versions: KEPT, chosen: FIFTH, content: content(body) };
  return new VersionComparing(reading, {
    read: () => Promise.resolve(paragraph('before')),
    current: () => Promise.resolve(paragraph('now')),
    ...source,
  });
}

describe('a comparison', () => {
  it('finds what changed from the version before, reading each body once', async () => {
    const read = vi.fn(() => Promise.resolve(paragraph('the quick fox')));
    const compare = comparing(paragraph('the slow fox'), { read });
    await compare.refresh();
    expect(compare.status).toBe('ready');
    expect([compare.diff?.added, compare.diff?.removed]).toEqual([1, 1]);
    await compare.refresh();
    expect(read).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenCalledWith(3);
  });

  it('keeps the kind and the whole view on the device, and not another version', async () => {
    const compare = comparing(paragraph('now'));
    compare.compareWith('current');
    await vi.waitFor(() => {
      expect(compare.status).toBe('ready');
    });
    expect(compare.diff?.changed).toBe(false);
    compare.compareWith(2);
    compare.showWhole(true);
    expect(compare.status).toBe('idle');
    expect(compare.diff).toBeUndefined();
    const again = comparing(paragraph('now'));
    expect([again.kind, again.whole, again.pinned]).toEqual(['current', true, undefined]);
    // Choosing what to compare with shows the comparison again.
    again.compareWith('previous');
    expect(again.whole).toBe(false);
    expect(localStorage.getItem('fvoci.versions.compare')).toBe('previous');
  });

  it('takes only the answer to the last ask', async () => {
    let answer: (body: DocNode) => void = () => undefined;
    const compare = comparing(paragraph('after'), {
      read: () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    const late = compare.refresh();
    expect(compare.status).toBe('comparing');
    compare.showWhole(true);
    answer(paragraph('before'));
    await late;
    expect(compare.status).toBe('idle');
    expect(compare.diff).toBeUndefined();
  });

  it('says a body that cannot be read, asking for it again next time, and does not compare one that cannot be shown', async () => {
    const read = vi
      .fn<CompareSource['read']>()
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValue(paragraph('before'));
    const compare = comparing(paragraph('after'), { read });
    await compare.refresh();
    expect(compare.status).toBe('failed');
    await compare.refresh();
    expect(compare.status).toBe('ready');
    expect(read).toHaveBeenCalledTimes(2);

    const broken = comparing({ type: 'doc', content: [{ type: 'nothing known' }] });
    await broken.refresh();
    expect(broken.status).toBe('invalid');
    const unreadable = comparing(paragraph('after'), {
      read: () => Promise.resolve({ type: 'doc', content: [{ type: 'nothing known' }] }),
    });
    await unreadable.refresh();
    expect(unreadable.status).toBe('failed');
  });
});

describe('the versions of an open card, compared', () => {
  it('shows what a version changed from the one before, and nothing from the body now', async () => {
    const made = await openFirstBoard();
    const card = made.board.cards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('no card');
    const readAt = made.world.blocks.get(card.id)?.updatedAt ?? 0;
    await made.other.writeCardBody(card.containerId, card.id, {
      body: text('second body'),
      readAt,
    });
    const open = await made.board.showCard(card.id);
    const versions = new VersionsReading(cardVersions(made.board, open));
    await vi.waitFor(() => {
      expect(versions.status).toBe('ready');
    });
    const [newest, oldest] = versions.versions;
    if (!newest || !oldest) throw new Error('two versions were not kept');

    await versions.choose(newest);
    await vi.waitFor(() => {
      expect(versions.compare.status).toBe('ready');
    });
    expect(versions.compare.target).toEqual({ kind: 'previous', version: oldest });
    expect([versions.compare.diff?.added, versions.compare.diff?.removed]).toEqual([1, 1]);

    versions.compare.compareWith('current');
    await vi.waitFor(() => {
      expect(versions.compare.status).toBe('ready');
    });
    expect(versions.compare.diff?.changed).toBe(false);

    // The oldest is shown as it is; back at the list nothing is compared.
    versions.compare.compareWith('previous');
    await versions.choose(oldest);
    expect(versions.compare.target).toEqual({ kind: 'oldest' });
    expect(versions.compare.status).toBe('idle');
    versions.leave();
    expect(versions.compare.target).toBeUndefined();
    made.workspace.close();
  });
});

describe('a version shown as it is', () => {
  it('still says what it is compared with once compared again: the version chosen, not only its kind', () => {
    const compare = comparing(paragraph('now'));
    compare.compareWith(2);
    compare.showWhole(true);
    expect(compare.target).toEqual({ kind: 'whole' });
    expect(compare.chosenTarget).toEqual({ kind: 'version', version: SECOND });
    compare.showWhole(false);
    expect(compare.target).toEqual({ kind: 'version', version: SECOND });
  });
});
