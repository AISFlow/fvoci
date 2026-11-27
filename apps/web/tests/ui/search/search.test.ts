/**
 * The search's own behaviour: when it asks (once typing stops), which answer
 * it shows (the last asked), what it does when refused, which keys open it
 * and move in its list, and a row drawn as text.
 */
import { cleanup, render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  listMove,
  opensSearch,
  searchKeysLabel,
  searchKeysNames,
} from '$lib/features/search/search-keys.ts';
import {
  HITS_ASKED,
  SearchSession,
  TYPING_PAUSE,
} from '$lib/features/search/search-session.svelte.ts';
import { RETRY_SECONDS, SearchSwitch } from '$lib/features/search/search-switch.svelte.ts';
import SearchHitRow from '$lib/features/search/SearchHitRow.svelte';
import { BackendError, type SearchAsk, type SearchHit } from '$lib/state/backend.ts';

const hit = (id: string, title = id): SearchHit => ({
  id,
  kind: 'card',
  containerId: 'b',
  containerKind: 'board',
  title,
  snippet: [],
});

interface Asked {
  ask: SearchAsk;
  answer: (hits: SearchHit[]) => void;
  refuse: (error: unknown) => void;
}

/** A session whose requests wait until the test answers them. */
function session(): { session: SearchSession; asked: Asked[] } {
  const asked: Asked[] = [];
  const made = new SearchSession({
    find: (ask) =>
      new Promise((resolve, reject) => {
        asked.push({ ask, answer: resolve, refuse: reject });
      }),
  });
  return { session: made, asked };
}

describe('a search session', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('asks once typing stops, for what was typed last', async () => {
    const { session: search, asked } = session();
    search.type('ka');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE - 1);
    search.type('kan');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE - 1);
    expect(asked).toHaveLength(0);
    expect(search.status).toBe('searching');
    await vi.advanceTimersByTimeAsync(1);
    expect(asked.map((one) => one.ask)).toEqual([
      { query: 'kan', kind: undefined, limit: HITS_ASKED },
    ]);
    asked[0]?.answer([hit('a')]);
    await vi.runAllTimersAsync();
    expect(search.status).toBe('found');
    expect(search.hits).toEqual([hit('a')]);
  });

  it('shows the answer to the last request, not a late one before it', async () => {
    const { session: search, asked } = session();
    search.type('one');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    search.type('two');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    asked[1]?.answer([hit('two')]);
    await vi.runAllTimersAsync();
    asked[0]?.answer([hit('one')]);
    await vi.runAllTimersAsync();
    expect(search.hits).toEqual([hit('two')]);
  });

  it('asks at once for another kind, and finds nothing for nothing typed', async () => {
    const { session: search, asked } = session();
    search.choose('page');
    expect(search.status).toBe('empty');
    search.type('  ');
    await vi.runAllTimersAsync();
    expect(asked).toHaveLength(0);
    search.type('word');
    search.choose('card');
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.map((one) => one.ask.kind)).toEqual(['card']);
  });

  it('drops what comes after it is closed, and asks nothing more', async () => {
    const { session: search, asked } = session();
    search.type('word');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    search.close();
    asked[0]?.answer([hit('a')]);
    search.type('more');
    await vi.runAllTimersAsync();
    expect(search.hits).toEqual([]);
    expect(asked).toHaveLength(1);
  });

  it('told to wait, says so and asks once more when it may', async () => {
    const { session: search, asked } = session();
    search.type('word');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    asked[0]?.refuse(new BackendError(429, { retryAfter: 3 }));
    await vi.advanceTimersByTimeAsync(0);
    expect(search.status).toBe('limited');
    await vi.advanceTimersByTimeAsync(2999);
    expect(asked).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(asked).toHaveLength(2);
    // Told again: it says so, and waits for the reader to try again.
    asked[1]?.refuse(new BackendError(429, { retryAfter: 1 }));
    await vi.runAllTimersAsync();
    expect(search.status).toBe('stillLimited');
    expect(asked).toHaveLength(2);
    search.retry();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toHaveLength(3);
  });

  it('typing while it waits to ask again asks for the new words instead', async () => {
    const { session: search, asked } = session();
    search.type('word');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    asked[0]?.refuse(new BackendError(429, { retryAfter: 5 }));
    await vi.advanceTimersByTimeAsync(0);
    search.type('words');
    await vi.runAllTimersAsync();
    expect(asked.map((one) => one.ask.query)).toEqual(['word', 'words']);
  });

  it('says the search server does not answer, and tries again when asked', async () => {
    const { session: search, asked } = session();
    search.type('word');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    asked[0]?.refuse(new BackendError(503, { reason: 'search_unavailable' }));
    await vi.advanceTimersByTimeAsync(0);
    expect(search.status).toBe('unavailable');
    search.retry();
    await vi.advanceTimersByTimeAsync(0);
    asked[1]?.answer([]);
    await vi.runAllTimersAsync();
    expect(search.status).toBe('none');
  });

  it('says that a search server that cannot hold its index cannot search now, and asks again', async () => {
    const { session: search, asked } = session();
    search.type('word');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    asked[0]?.refuse(new BackendError(503, { reason: 'search_unavailable' }));
    await vi.runAllTimersAsync();
    expect(search.status).toBe('unavailable');
    search.type('more');
    await vi.runAllTimersAsync();
    expect(asked).toHaveLength(2);
  });

  it('gives what the words in the field find to who waits for it, and nothing for words changed', async () => {
    const { session: search, asked } = session();
    search.type('one');
    const forOne = search.answered();
    search.type('two');
    const forTwo = search.answered();
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    asked[0]?.answer([hit('two')]);
    expect(await forOne).toBeUndefined();
    expect(await forTwo).toEqual([hit('two')]);
    // Found already: what is shown.
    expect(await search.answered()).toEqual([hit('two')]);
    search.type('three');
    const forThree = search.answered();
    search.close();
    expect(await forThree).toBeUndefined();
  });

  it('passes a session that ended on to the page', async () => {
    const refused = vi.fn();
    const asked: Asked[] = [];
    const search = new SearchSession({
      find: (ask) =>
        new Promise((resolve, reject) => {
          asked.push({ ask, answer: resolve, refuse: reject });
        }),
      onRefused: refused,
    });
    search.type('word');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    const ended = new BackendError(401, {});
    asked[0]?.refuse(ended);
    await vi.runAllTimersAsync();
    expect(search.status).toBe('failed');
    expect(refused).toHaveBeenCalledWith(ended);
  });

  it('fails other refusals, and what is no refusal', async () => {
    const { session: search, asked } = session();
    search.type('word');
    await vi.advanceTimersByTimeAsync(TYPING_PAUSE);
    asked[0]?.refuse(new BackendError(404, {}));
    await vi.runAllTimersAsync();
    expect(search.status).toBe('failed');
    search.retry();
    await vi.advanceTimersByTimeAsync(0);
    asked[1]?.refuse(new Error('no'));
    await vi.runAllTimersAsync();
    expect(search.status).toBe('failed');
  });
});

const press = (init: KeyboardEventInit): KeyboardEvent => new KeyboardEvent('keydown', init);

describe('the keys of the search', () => {
  it('the platform’s ⌘K or Ctrl+K opens it, and the other modifier does not', () => {
    // The tests run with no Apple platform: Ctrl is the modifier, Meta no alias of it.
    expect(opensSearch(press({ key: 'K', code: 'KeyK', ctrlKey: true }))).toBe(true);
    expect(opensSearch(press({ key: 'k', code: 'KeyK', metaKey: true }))).toBe(false);
    expect(opensSearch(press({ key: 'k', code: 'KeyK', ctrlKey: true, metaKey: true }))).toBe(
      false,
    );
    expect(opensSearch(press({ key: 'k', code: 'KeyK' }))).toBe(false);
    expect(opensSearch(press({ key: 'k', code: 'KeyK', ctrlKey: true, shiftKey: true }))).toBe(
      false,
    );
    expect(opensSearch(press({ key: 'k', code: 'KeyK', ctrlKey: true, altKey: true }))).toBe(false);
  });

  it('on Apple’s devices ⌘K alone, and ⌘Enter opens a new tab', () => {
    for (const platform of ['MacIntel', 'iPad']) {
      expect(opensSearch(press({ key: 'k', code: 'KeyK', metaKey: true }), platform)).toBe(true);
      // On a Mac, Ctrl+K in a field deletes to the end of the line: it is the field's.
      expect(opensSearch(press({ key: 'k', code: 'KeyK', ctrlKey: true }), platform)).toBe(false);
      expect(listMove(press({ key: 'Enter', metaKey: true }), platform)).toBe('goNewTab');
      expect(listMove(press({ key: 'Enter', ctrlKey: true }), platform)).toBeUndefined();
    }
  });

  it('by the K of a Latin keyboard where the key gives a letter of another script', () => {
    expect(opensSearch(press({ key: 'ㅏ', code: 'KeyK', ctrlKey: true }))).toBe(true);
    // Where the K key gives a Latin letter, the letter is what counts (Dvorak's T there).
    expect(opensSearch(press({ key: 't', code: 'KeyK', ctrlKey: true }))).toBe(false);
  });

  it('takes a held key, for whoever listens to take its repeats', () => {
    expect(opensSearch(press({ key: 'k', code: 'KeyK', ctrlKey: true, repeat: true }))).toBe(true);
  });

  it('leaves a press something took already, or one of an input method', () => {
    const taken = press({ key: 'k', code: 'KeyK', ctrlKey: true, cancelable: true });
    taken.preventDefault();
    expect(opensSearch(taken)).toBe(false);
    expect(opensSearch(press({ key: 'k', code: 'KeyK', ctrlKey: true, isComposing: true }))).toBe(
      false,
    );
    // eslint-disable-next-line @typescript-eslint/no-deprecated -- the only mark Safari leaves
    expect(opensSearch(press({ key: 'k', code: 'KeyK', ctrlKey: true, keyCode: 229 }))).toBe(false);
  });

  it('moves in the list, and goes, but not while letters are put together', () => {
    expect(listMove(press({ key: 'ArrowDown' }))).toBe('next');
    expect(listMove(press({ key: 'ArrowUp' }))).toBe('previous');
    expect(listMove(press({ key: 'Enter' }))).toBe('go');
    expect(listMove(press({ key: 'Enter', ctrlKey: true }))).toBe('goNewTab');
    // The other platform's modifier neither goes nor opens a tab.
    expect(listMove(press({ key: 'Enter', metaKey: true }))).toBeUndefined();
    expect(listMove(press({ key: 'Enter', isComposing: true }))).toBeUndefined();
    // Safari's Enter that takes the letters: the key code is all that tells it.
    // eslint-disable-next-line @typescript-eslint/no-deprecated -- the only mark Safari leaves
    expect(listMove(press({ key: 'Enter', keyCode: 229 }))).toBeUndefined();
    expect(listMove(press({ key: 'ArrowDown', isComposing: true }))).toBeUndefined();
    expect(listMove(press({ key: 'a' }))).toBeUndefined();
  });

  it('are named as the keyboard of the device names them', () => {
    expect(searchKeysLabel('MacIntel')).toBe('⌘K');
    expect(searchKeysLabel('iPhone')).toBe('⌘K');
    expect(searchKeysLabel('Linux aarch64')).toBe('Ctrl+K');
    expect(searchKeysNames('MacIntel')).toBe('Meta+K');
    expect(searchKeysNames('Win32')).toBe('Control+K');
  });
});

describe('a row of what was found', () => {
  afterEach(() => {
    cleanup();
  });

  it('draws the snippet as text, the matched stretches marked', () => {
    const { container } = render(SearchHitRow, {
      props: {
        hit: {
          ...hit('c', 'A <i>title</i>'),
          snippet: [
            { text: 'before <script>alert(1)</script> ', matched: false },
            { text: '<b>word</b>', matched: true },
            { text: ' after', matched: false },
          ],
        },
        id: 'row',
        href: '/w/x/boards/b/-/c',
        container: 'Board',
        locale: 'en',
        active: true,
        onGo: () => undefined,
        onPoint: () => undefined,
      },
    });
    const row = container.querySelector('[role="option"]');
    expect(row?.getAttribute('aria-selected')).toBe('true');
    expect(container.querySelector('script, b, i')).toBeNull();
    expect(container.querySelector('[data-hit-title]')?.textContent).toBe('A <i>title</i>');
    expect(container.querySelector('[data-hit-snippet]')?.textContent).toBe(
      'before <script>alert(1)</script> <b>word</b> after',
    );
    expect([...container.querySelectorAll('mark')].map((mark) => mark.textContent)).toEqual([
      '<b>word</b>',
    ]);
  });

  it('names a diary entry by its day, as the reader reads a day', () => {
    const { container } = render(SearchHitRow, {
      props: {
        hit: { ...hit('e', '2026-11-21'), kind: 'entry', containerKind: 'diary' },
        id: 'row',
        href: '/w/x/diaries/d/11-21',
        container: 'Journal',
        locale: 'en',
        active: false,
        onGo: () => undefined,
        onPoint: () => undefined,
      },
    });
    expect(container.querySelector('[data-hit-title]')?.textContent).toBe(
      'Saturday, November 21, 2026',
    );
    expect(container.querySelector('[role="option"]')?.textContent).toContain('Diary entry:');
  });

  it('goes on a plain click, and leaves a click for a new tab to the browser', () => {
    const onGo = vi.fn();
    const { container } = render(SearchHitRow, {
      props: {
        hit: hit('c'),
        id: 'row',
        href: '/somewhere',
        container: '',
        locale: 'en',
        active: false,
        onGo,
        onPoint: () => undefined,
      },
    });
    const row = container.querySelector('a');
    const plain = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });
    row?.dispatchEvent(plain);
    expect(onGo).toHaveBeenCalledOnce();
    expect(plain.defaultPrevented).toBe(true);
    const newTab = new MouseEvent('click', { bubbles: true, cancelable: true, metaKey: true });
    row?.dispatchEvent(newTab);
    expect(onGo).toHaveBeenCalledOnce();
    expect(newTab.defaultPrevented).toBe(false);
    expect(container.querySelector('[data-hit-container]')).toBeNull();
  });
});

describe('whether the search is there', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  const searchAnswering = (answers: (boolean | Error)[]) => {
    const enabled = vi.fn(() => {
      const answer = answers.shift() ?? new Error('no more answers');
      return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer);
    });
    return { enabled, find: vi.fn() };
  };

  it('is off until the backend says, and as it says', async () => {
    const search = searchAnswering([true]);
    const shown = new SearchSwitch(search);
    expect(shown.on).toBe(false);
    await vi.advanceTimersByTimeAsync(0);
    expect(shown.on).toBe(true);
  });

  it('asks again after a failure, waiting longer each time, and then no more', async () => {
    const search = searchAnswering([new Error('offline'), new Error('offline'), true]);
    const shown = new SearchSwitch(search);
    await vi.advanceTimersByTimeAsync(0);
    expect(shown.on).toBe(false);
    await vi.advanceTimersByTimeAsync(RETRY_SECONDS[0] * 1000);
    expect(search.enabled).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(RETRY_SECONDS[1] * 1000);
    expect(shown.on).toBe(true);
    const failing = searchAnswering([]);
    new SearchSwitch(failing);
    await vi.runAllTimersAsync();
    expect(failing.enabled).toHaveBeenCalledTimes(RETRY_SECONDS.length + 1);
  });
});
