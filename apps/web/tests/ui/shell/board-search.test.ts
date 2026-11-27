/** The sidebar's search asked of the backend: after a pause, the latest only. */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { BoardSearch } from '$lib/features/shell/board-search.svelte.ts';
import { createBoard } from '$lib/logic/wire.ts';

afterEach(() => {
  vi.useRealTimers();
});

const board = (title: string) => ({ ...createBoard(), title });

describe('the search of the sidebar', () => {
  it('asks once the typing stops, and keeps only the answer to the latest query', async () => {
    vi.useFakeTimers();
    const answers: ((boards: ReturnType<typeof board>[]) => void)[] = [];
    const asked: string[] = [];
    const search = new BoardSearch((query) => {
      asked.push(query);
      return new Promise((resolve) => answers.push(resolve));
    });
    search.run('ga');
    search.run('gar');
    await vi.advanceTimersByTimeAsync(300);
    expect(asked).toEqual(['gar']);
    search.run('garden');
    // What was found for another query is no answer to this one.
    expect(search.found).toEqual([]);
    expect(search.pending).toBe(true);
    await vi.advanceTimersByTimeAsync(300);
    expect(asked).toEqual(['gar', 'garden']);
    answers[1]?.([board('Garden plan')]);
    answers[0]?.([board('Garage')]);
    await vi.advanceTimersByTimeAsync(0);
    expect(search.found.map((one) => one.title)).toEqual(['Garden plan']);
    expect(search.pending).toBe(false);
  });

  it('finds nothing at once for an empty query, and an answer on its way is let go', async () => {
    vi.useFakeTimers();
    let answer: ((boards: ReturnType<typeof board>[]) => void) | undefined;
    const search = new BoardSearch(
      () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    );
    search.run('plan');
    await vi.advanceTimersByTimeAsync(300);
    search.run('  ');
    answer?.([board('Plan')]);
    await vi.advanceTimersByTimeAsync(0);
    expect(search.found).toEqual([]);
  });
});
