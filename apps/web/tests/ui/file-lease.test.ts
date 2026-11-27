/**
 * The lease of a file open in the editor, as the app keeps it
 * (lib/file-viewer/lease.svelte.ts): taken by "Edit" and given back where a
 * newer version came meanwhile; held on to every while and when the window
 * is shown again; let go of when the editor closes and as the page goes; a
 * lease lost told; let go of with no input for a while, said before, and
 * taken again at the next input. And who edits the file, as the viewer says it
 * (EditingNote.svelte): the reader in another window, somebody named, or
 * somebody else.
 */
import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import EditingNote from '$lib/components/file-viewer/EditingNote.svelte';
import {
  EditLease,
  LeaseWatch,
  startEditing,
  type FileLease,
  type HeldFileLease,
  type LeaseSource,
} from '$lib/file-viewer/lease.svelte.ts';

/** The times scaled down: held every second, let go of after 10 seconds with no input, said 3 before. */
const TIMES = { holdEvery: 1_000, watchEvery: 500, idleAfter: 10_000, warnBefore: 3_000 };

/** A file's lease as the server keeps it, for one reader, and what was asked of it. */
function sourceOf(me = 'me') {
  const asked: string[] = [];
  const state = {
    lease: { editing: false, editorId: '' } as FileLease,
    /** What the next holds answer: a lease, another's, or no answer. */
    holding: 'held' as 'held' | 'editing' | 'unreached',
    taken: 0,
    learned: [] as string[],
  };
  const source: LeaseSource = {
    me,
    lease: () => Promise.resolve(state.lease),
    holdLease: (leaseId): Promise<HeldFileLease | 'editing'> => {
      asked.push(`hold ${leaseId}`);
      if (state.holding === 'unreached') return Promise.reject(new Error('unreached'));
      if (state.holding === 'editing') return Promise.resolve('editing');
      if (leaseId === '') state.taken += 1;
      return Promise.resolve({ leaseId: leaseId || `l${String(state.taken)}`, seconds: 90 });
    },
    releaseLease: (leaseId, keepalive) => {
      asked.push(`release ${leaseId}${keepalive ? ' keepalive' : ''}`);
      return Promise.resolve();
    },
    learnUsers: (ids) => {
      state.learned.push(...ids);
      return Promise.resolve();
    },
  };
  return { source, asked, state };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('the lease of a file open in the editor', () => {
  it('is held on to every while and as the window is shown again, and let go of on close', async () => {
    const { source, asked } = sourceOf();
    const lease = new EditLease(source, TIMES);
    expect(await lease.take()).toBe('held');
    await vi.advanceTimersByTimeAsync(2_500);
    expect(asked).toEqual(['hold ', 'hold l1', 'hold l1']);
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.at(-1)).toBe('hold l1');
    await lease.release();
    expect(asked.at(-1)).toBe('release l1');
    const before = asked.length;
    await vi.advanceTimersByTimeAsync(5_000);
    document.dispatchEvent(new Event('visibilitychange'));
    await lease.release();
    expect(asked).toHaveLength(before);
  });

  it('goes with the page, by a request that outlives it', async () => {
    const { source, asked } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    window.dispatchEvent(new Event('pagehide'));
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.at(-1)).toBe('release l1 keepalive');
    await lease.release();
    expect(asked.filter((one) => one.startsWith('release'))).toHaveLength(1);
  });

  it('says so when another took the file’s meanwhile, and not while it is only unreached', async () => {
    const { source, state } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    state.holding = 'unreached';
    await vi.advanceTimersByTimeAsync(1_000);
    expect(lease.lost).toBe(false);
    state.holding = 'editing';
    await vi.advanceTimersByTimeAsync(1_000);
    expect(lease.lost).toBe(true);
    // Nobody holds it any more: it is the editor's again.
    state.holding = 'held';
    await vi.advanceTimersByTimeAsync(1_000);
    expect(lease.lost).toBe(false);
    await lease.release();
  });

  it('is not taken while another edits, nor where the server is not reached', async () => {
    const { source, state, asked } = sourceOf();
    state.holding = 'editing';
    expect(await new EditLease(source, TIMES).take()).toBe('editing');
    state.holding = 'unreached';
    expect(await new EditLease(source, TIMES).take()).toBe('failed');
    await vi.advanceTimersByTimeAsync(5_000);
    expect(asked).toEqual(['hold ', 'hold ']);
  });
});

describe('the lease of an editor nobody puts anything into', () => {
  it('is let go of after a while, said before, and not held on to since', async () => {
    const { source, asked } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    await vi.advanceTimersByTimeAsync(6_000);
    expect(lease.idleSoon).toBe(false);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(lease.idleSoon).toBe(true);
    expect(lease.resting).toBe(false);
    expect(asked.filter((one) => one.startsWith('release'))).toEqual([]);
    await vi.advanceTimersByTimeAsync(3_000);
    expect(lease.resting).toBe(true);
    expect(lease.idleSoon).toBe(false);
    expect(asked.at(-1)).toBe('release l1');
    const before = asked.length;
    await vi.advanceTimersByTimeAsync(5_000);
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toHaveLength(before);
    // Closed while let go of: nothing is let go of twice.
    await lease.release();
    expect(asked).toHaveLength(before);
  });

  it('is held on to while input comes, however long the editor is open', async () => {
    const { source, asked } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    for (let turn = 0; turn < 4; turn += 1) {
      await vi.advanceTimersByTimeAsync(8_000);
      expect(lease.idleSoon).toBe(true);
      lease.touch();
      expect(lease.idleSoon).toBe(false);
    }
    expect(lease.resting).toBe(false);
    expect(asked.filter((one) => one.startsWith('release'))).toEqual([]);
    await lease.release();
  });

  it('is taken again at the next input, where nobody else took it meanwhile', async () => {
    const { source, asked } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(lease.resting).toBe(true);
    lease.touch();
    expect(lease.resting).toBe(false);
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.slice(-2)).toEqual(['release l1', 'hold ']);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(asked.at(-1)).toBe('hold l2');
    expect(lease.lost).toBe(false);
    await lease.release();
    expect(asked.at(-1)).toBe('release l2');
  });

  it('is taken again only once the server let it go, input coming however soon', async () => {
    const { source, asked } = sourceOf();
    let answer = (): void => undefined;
    source.releaseLease = (leaseId) => {
      asked.push(`release ${leaseId}`);
      return new Promise((resolve) => {
        answer = resolve;
      });
    };
    const lease = new EditLease(source, TIMES);
    await lease.take();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(lease.resting).toBe(true);
    lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.at(-1)).toBe('release l1');
    answer();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.at(-1)).toBe('hold ');
    expect(lease.lost).toBe(false);
    const closed = lease.release();
    answer();
    await closed;
  });

  it('is held on to by its name at the next input where letting it go did not reach the server', async () => {
    const { source, asked } = sourceOf();
    source.releaseLease = (leaseId) => {
      asked.push(`release ${leaseId}`);
      return Promise.reject(new Error('unreached'));
    };
    const lease = new EditLease(source, TIMES);
    await lease.take();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(lease.resting).toBe(true);
    lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    // Still the editor's on the server: held on to, not taken anew and found held.
    expect(asked.slice(-2)).toEqual(['release l1', 'hold l1']);
    expect(lease.lost).toBe(false);
    await lease.release();
  });

  it('says still that somebody else took it, where they had as it went', async () => {
    const { source, state } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    state.holding = 'editing';
    await vi.advanceTimersByTimeAsync(1_000);
    expect(lease.lost).toBe(true);
    await vi.advanceTimersByTimeAsync(9_000);
    expect(lease.resting).toBe(true);
    expect(lease.lost).toBe(true);
    lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    expect(lease.lost).toBe(true);
    await lease.release();
  });

  it('says in how many minutes it goes, counting down at each turn', async () => {
    const { source } = sourceOf();
    const times = {
      holdEvery: 30_000,
      watchEvery: 15_000,
      idleAfter: 30 * 60_000,
      warnBefore: 5 * 60_000,
    };
    const lease = new EditLease(source, times);
    await lease.take();
    await vi.advanceTimersByTimeAsync(25 * 60_000 + 30_000);
    expect([lease.idleSoon, lease.minutesLeft]).toEqual([true, 5]);
    await vi.advanceTimersByTimeAsync(2 * 60_000);
    expect(lease.minutesLeft).toBe(3);
    await vi.advanceTimersByTimeAsync(2 * 60_000);
    expect(lease.minutesLeft).toBe(1);
    lease.touch();
    expect(lease.idleSoon).toBe(false);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(lease.minutesLeft).toBe(0);
    await lease.release();
  });

  it('asks again at the next turn where a request was not answered within one', async () => {
    const { source, asked } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    source.holdLease = (leaseId) => {
      asked.push(`hold ${leaseId}`);
      return new Promise(() => undefined);
    };
    await vi.advanceTimersByTimeAsync(3_500);
    expect(asked.filter((one) => one === 'hold l1').length).toBeGreaterThanOrEqual(2);
    await lease.release();
  });

  it('is held on to by its name a turn after the input where letting it go is not answered', async () => {
    const { source, asked } = sourceOf();
    source.releaseLease = (leaseId) => {
      asked.push(`release ${leaseId}`);
      return new Promise(() => undefined);
    };
    const lease = new EditLease(source, TIMES);
    await lease.take();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(lease.resting).toBe(true);
    lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.at(-1)).toBe('release l1');
    await vi.advanceTimersByTimeAsync(1_000);
    expect(asked.at(-1)).toBe('hold l1');
    expect(lease.lost).toBe(false);
  });

  it('is let go of once more where the editor closed while letting it go failed', async () => {
    const { source, asked } = sourceOf();
    let fail = (): void => undefined;
    source.releaseLease = (leaseId) => {
      asked.push(`release ${leaseId}`);
      return new Promise((_, reject) => {
        fail = () => {
          reject(new Error('unreached'));
        };
      });
    };
    const lease = new EditLease(source, TIMES);
    await lease.take();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(lease.resting).toBe(true);
    await lease.release();
    expect(asked.filter((one) => one.startsWith('release'))).toEqual(['release l1']);
    fail();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.filter((one) => one.startsWith('release'))).toEqual(['release l1', 'release l1']);
  });

  it('says somebody else holds it where they took it meanwhile, and is the editor’s once they let go', async () => {
    const { source, state } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    await vi.advanceTimersByTimeAsync(10_000);
    state.holding = 'editing';
    lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    expect(lease.resting).toBe(false);
    expect(lease.lost).toBe(true);
    state.holding = 'held';
    await vi.advanceTimersByTimeAsync(1_000);
    expect(lease.lost).toBe(false);
    await lease.release();
  });

  it('is let go of as a window asleep past the while wakes, not held on to first', async () => {
    const { source, asked } = sourceOf();
    const lease = new EditLease(source, TIMES);
    await lease.take();
    // Asleep: no turn came, and the clock went on.
    vi.setSystemTime(Date.now() + 60_000);
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(0);
    expect(lease.resting).toBe(true);
    expect(asked).toEqual(['hold ', 'release l1']);
    await lease.release();
  });
});

/**
 * How long the server keeps a lease not held on to: a little over three turns, as 90 seconds are
 * three of 30 (the half keeps a hold in the same millisecond as the end from deciding a test).
 */
const TERM = 3_500;

/** What comes of a request: answered, answered when the test says, reached and not answered, not reached. */
type Fate = 'answered' | 'late' | 'unanswered' | 'unreached';

/**
 * A file's lease as the server keeps it (crates/server/src/app/file_leases.rs), for more than
 * one person: held on to by its name while it lasts, taken anew where nobody holds the file's
 * (whatever name was given), refused otherwise and then not lengthened; let go of only by its
 * holder, by its name. Each person's requests come to what their fates say.
 */
function serverOf() {
  let held: { id: string; userId: string; until: number } | undefined;
  let made = 0;
  const live = () => {
    if (held && held.until <= Date.now()) held = undefined;
    return held;
  };
  const late: (() => void)[] = [];
  function carry<T>(fate: Fate, act: () => T): Promise<T> {
    if (fate === 'unreached') return new Promise(() => undefined);
    const answer = act();
    if (fate === 'answered') return Promise.resolve(answer);
    if (fate === 'unanswered') return new Promise(() => undefined);
    return new Promise((resolve) => {
      late.push(() => {
        resolve(answer);
      });
    });
  }
  function personOf(userId: string) {
    const asked: string[] = [];
    const fates = {
      hold: 'answered' as Fate,
      release: 'answered' as Fate,
      lease: 'answered' as Fate,
    };
    const source: LeaseSource = {
      me: userId,
      lease: () =>
        carry(fates.lease, () => {
          const lease = live();
          return lease
            ? { editing: true, editorId: lease.userId }
            : { editing: false, editorId: '' };
        }),
      holdLease: (leaseId) => {
        asked.push(`hold ${leaseId}`);
        return carry(fates.hold, (): HeldFileLease | 'editing' => {
          const lease = live();
          if (lease && (lease.id !== leaseId || lease.userId !== userId)) return 'editing';
          if (lease) lease.until = Date.now() + TERM;
          else {
            made += 1;
            held = { id: `l${String(made)}`, userId, until: Date.now() + TERM };
          }
          return { leaseId: live()?.id ?? '', seconds: TERM / 1000 };
        });
      },
      releaseLease: (leaseId) => {
        asked.push(`release ${leaseId}`);
        return carry(fates.release, () => {
          if (live()?.id === leaseId && held?.userId === userId) held = undefined;
        });
      },
      learnUsers: () => Promise.resolve(),
    };
    return { source, asked, fates };
  }
  return {
    personOf,
    /** Who holds the file's lease now, by its name. */
    holder: () => live()?.id,
    /** Answers the requests answered late, as they came. */
    answerLate: () => {
      for (const answer of late.splice(0)) answer();
    },
  };
}

/** An editor of `server`'s file let go of for want of input, its lease let go of on the server. */
async function rested(server: ReturnType<typeof serverOf>) {
  const me = server.personOf('me');
  const lease = new EditLease(me.source, TIMES);
  expect(await lease.take()).toBe('held');
  await vi.advanceTimersByTimeAsync(10_000);
  expect(lease.resting).toBe(true);
  return { lease, ...me };
}

/** Runs `turns` turns of holding on, `lost` read after each. */
async function lostOver(lease: EditLease, turns: number): Promise<boolean[]> {
  const lost: boolean[] = [];
  for (let turn = 0; turn < turns; turn += 1) {
    await vi.advanceTimersByTimeAsync(TIMES.holdEvery);
    lost.push(lease.lost);
  }
  return lost;
}

describe('the lease of an editor whose request to the server was given up on', () => {
  it('a lease taken again by a request answered past its turn is the editor’s, by the name it comes with', async () => {
    const server = serverOf();
    const { lease, asked, fates } = await rested(server);
    expect(server.holder()).toBeUndefined();
    fates.hold = 'late';
    lease.touch();
    // The server gave the editor l2, and the answer is on its way.
    await vi.advanceTimersByTimeAsync(0);
    expect(server.holder()).toBe('l2');
    fates.hold = 'answered';
    // The turns after the one given up on find the file's lease held: the editor's own.
    expect(await lostOver(lease, 2)).toEqual([false, false]);
    server.answerLate();
    await vi.advanceTimersByTimeAsync(0);
    expect(await lostOver(lease, 2)).toEqual([false, false]);
    expect(asked.at(-1)).toBe('hold l2');
    expect(server.holder()).toBe('l2');
    await lease.release();
    expect(server.holder()).toBeUndefined();
  });

  it('a lease taken again by a request never answered is not told as another’s, and the file’s is taken once it ends', async () => {
    const server = serverOf();
    const { lease, asked, fates } = await rested(server);
    fates.hold = 'unanswered';
    lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    expect(server.holder()).toBe('l2');
    fates.hold = 'answered';
    // l2 lasts its term from when it was given, unknown to the editor: then it goes, and the
    // next turn takes the file's anew.
    expect(await lostOver(lease, 5)).toEqual([false, false, false, false, false]);
    expect(server.holder()).toBe('l3');
    expect(asked.at(-1)).toBe('hold l3');
    await lease.release();
  });

  it('a lease let go of by a request that did not reach the server is held on to by its name at the next input', async () => {
    const server = serverOf();
    const me = server.personOf('me');
    const lease = new EditLease(me.source, TIMES);
    await lease.take();
    me.fates.release = 'unreached';
    await vi.advanceTimersByTimeAsync(10_000);
    expect(lease.resting).toBe(true);
    // Still held on the server, until its term ends.
    expect(server.holder()).toBe('l1');
    lease.touch();
    expect(await lostOver(lease, 3)).toEqual([false, false, false]);
    expect(me.asked.filter((one) => one.startsWith('hold')).slice(-3)).toEqual([
      'hold l1',
      'hold l1',
      'hold l1',
    ]);
    expect(server.holder()).toBe('l1');
    me.fates.release = 'answered';
    await lease.release();
    expect(server.holder()).toBeUndefined();
  });

  it('closed while letting go of it is not answered: let go of by its name', async () => {
    const server = serverOf();
    const me = server.personOf('me');
    const lease = new EditLease(me.source, TIMES);
    await lease.take();
    me.fates.release = 'unreached';
    await vi.advanceTimersByTimeAsync(10_000);
    me.fates.release = 'answered';
    await lease.release();
    // Once letting it go is given up on, a turn later.
    await vi.advanceTimersByTimeAsync(TIMES.holdEvery);
    expect(me.asked.slice(-2)).toEqual(['release l1', 'release l1']);
    expect(server.holder()).toBeUndefined();
  });

  it('a lease another took meanwhile is told at once, though a request was given up on', async () => {
    const server = serverOf();
    const { lease, fates } = await rested(server);
    fates.hold = 'unreached';
    lease.touch();
    await vi.advanceTimersByTimeAsync(TIMES.holdEvery);
    const ann = new EditLease(server.personOf('ann').source, TIMES);
    expect(await ann.take()).toBe('held');
    fates.hold = 'answered';
    expect(await lostOver(lease, 1)).toEqual([true]);
    await ann.release();
    await lease.release();
  });

  it('the reader’s other window, holding the file’s while a request was given up on, is told once that could be no lease of this one', async () => {
    const server = serverOf();
    const { lease, fates, source } = await rested(server);
    fates.hold = 'unreached';
    lease.touch();
    await vi.advanceTimersByTimeAsync(TIMES.holdEvery);
    const other = new EditLease(source, TIMES);
    fates.hold = 'answered';
    expect(await other.take()).toBe('held');
    // The same person holds it: as well this editor's lost answer, for a term from the turn
    // given up on.
    expect(await lostOver(lease, 3)).toEqual([false, false, false]);
    expect(await lostOver(lease, 2)).toEqual([true, true]);
    await other.release();
    await lease.release();
  });

  it('who holds it not read meanwhile: not told as another’s', async () => {
    const server = serverOf();
    const { lease, fates } = await rested(server);
    fates.hold = 'unanswered';
    lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    fates.hold = 'answered';
    fates.lease = 'unreached';
    expect(await lostOver(lease, 2)).toEqual([false, false]);
    await lease.release();
  });

  it('a lease answered late to an editor closed, or let go of again, meanwhile is let go of', async () => {
    const server = serverOf();
    const closed = await rested(server);
    closed.fates.hold = 'late';
    closed.lease.touch();
    // Given up on, a turn later, and closed.
    await vi.advanceTimersByTimeAsync(TIMES.holdEvery);
    await closed.lease.release();
    expect(server.holder()).toBe('l2');
    server.answerLate();
    await vi.advanceTimersByTimeAsync(0);
    expect(closed.asked.at(-1)).toBe('release l2');
    expect(server.holder()).toBeUndefined();

    const resting = await rested(server);
    resting.fates.hold = 'late';
    resting.lease.touch();
    await vi.advanceTimersByTimeAsync(0);
    expect(server.holder()).toBe('l4');
    resting.fates.hold = 'answered';
    // No input since: it rests again at the next turn.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(resting.lease.resting).toBe(true);
    server.answerLate();
    await vi.advanceTimersByTimeAsync(0);
    expect(resting.asked.at(-1)).toBe('release l4');
    expect(server.holder()).toBeUndefined();
    await resting.lease.release();
  });
});

describe('"Edit"', () => {
  it('holds the lease where the version shown is the newest', async () => {
    const { source, asked } = sourceOf();
    const started = await startEditing(source, 3, () => Promise.resolve(3));
    expect(started.as).toBe('held');
    if (started.as === 'held') await started.lease.release();
    expect(asked).toEqual(['hold ', 'release l1']);
  });

  it('gives the lease back where a newer version came meanwhile', async () => {
    const { source, asked } = sourceOf();
    expect(await startEditing(source, 3, () => Promise.resolve(4))).toEqual({ as: 'newer' });
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toEqual(['hold ', 'release l1']);
  });

  it('opens nothing while another edits the file', async () => {
    const { source, state } = sourceOf();
    state.holding = 'editing';
    const newest = vi.fn(() => Promise.resolve(3));
    expect(await startEditing(source, 3, newest)).toEqual({ as: 'editing' });
    expect(newest).not.toHaveBeenCalled();
  });
});

describe('who edits the file, as the viewer says it', () => {
  const said = () => screen.getByTestId('file-viewer-editing');

  async function shown(lease: FileLease, note?: 'failed' | 'newer') {
    const { source, state } = sourceOf('me');
    state.lease = lease;
    const watch = new LeaseWatch(source, TIMES.watchEvery);
    await watch.read();
    render(EditingNote, {
      watch,
      note,
      userName: (userId: string) => (userId === 'ann' ? 'Ann' : undefined),
    });
    return { watch, state };
  }

  it('names who edits, where the reader may know who', async () => {
    const { state } = await shown({ editing: true, editorId: 'ann' });
    expect(said().textContent.trim()).toBe('Ann is editing this file.');
    expect(said().dataset.editing).toBe('true');
    expect(said().getAttribute('role')).toBe('status');
    expect(state.learned).toEqual(['ann']);
  });

  it('says somebody else where the reader may not know who, and the reader’s other window', async () => {
    await shown({ editing: true, editorId: '' });
    expect(said().textContent.trim()).toBe('Someone else is editing this file.');
    cleanup();
    await shown({ editing: true, editorId: 'me' });
    expect(said().textContent.trim()).toBe('You are editing this file in another window.');
  });

  it('says nothing while nobody edits, and what came of the last "Edit" otherwise', async () => {
    await shown({ editing: false, editorId: '' });
    expect(said().textContent.trim()).toBe('');
    expect(said().dataset.editing).toBe('false');
    cleanup();
    await shown({ editing: false, editorId: '' }, 'newer');
    expect(said().textContent).toContain('A newer version was saved in the meantime.');
  });

  it('asks again every while the page is shown, once the reader’s own lease went', async () => {
    const { source, state } = sourceOf();
    const watch = new LeaseWatch(source, TIMES.watchEvery);
    let release: () => void = () => undefined;
    const released = new Promise<void>((resolve) => {
      release = resolve;
    });
    state.lease = { editing: true, editorId: 'me' };
    const stop = watch.watch(released);
    await vi.advanceTimersByTimeAsync(0);
    expect(watch.lease).toBeUndefined();
    state.lease = { editing: false, editorId: '' };
    release();
    await vi.advanceTimersByTimeAsync(0);
    expect(watch.editing).toBe(false);
    state.lease = { editing: true, editorId: '' };
    await vi.advanceTimersByTimeAsync(500);
    expect(watch.editor).toEqual({ who: 'someone' });
    stop();
    state.lease = { editing: false, editorId: '' };
    await vi.advanceTimersByTimeAsync(5_000);
    expect(watch.editing).toBe(true);
  });
});
