/**
 * The reader's choice of the numbers of the lines of code blocks, as the
 * switch in the settings holds it (code-lines.svelte.ts): put on the page
 * as the app starts, and followed when another tab of the device changes it.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { CODE_LINES_ATTRIBUTE, CODE_LINES_STORAGE_KEY } from '$lib/editor/code-lines.ts';
import { codeLines, startCodeLines } from '$lib/editor/code-lines.svelte.ts';

const shownOnPage = (): boolean => document.documentElement.hasAttribute(CODE_LINES_ATTRIBUTE);

/** What another tab of the device does to the storage, as this tab hears it. */
function elsewhere(key: string | null, value: string | null): void {
  if (key === null) localStorage.clear();
  else if (value !== null) localStorage.setItem(key, value);
  window.dispatchEvent(new StorageEvent('storage', { key, newValue: value }));
}

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  document.documentElement.removeAttribute(CODE_LINES_ATTRIBUTE);
});

describe('the numbers of the lines, in more than one tab', () => {
  it('follows another tab, and its storage cleared, without minding other keys', () => {
    startCodeLines();
    expect([codeLines.shown, shownOnPage()]).toEqual([false, false]);
    elsewhere(CODE_LINES_STORAGE_KEY, '1');
    expect([codeLines.shown, shownOnPage()]).toEqual([true, true]);
    // Another setting of the device changes nothing, and is not even read for this one.
    const read = vi.spyOn(Storage.prototype, 'getItem');
    elsewhere('fvoci:theme', 'dark');
    expect(read).not.toHaveBeenCalled();
    expect([codeLines.shown, shownOnPage()]).toEqual([true, true]);
    read.mockRestore();
    // Cleared: nothing is remembered, so they are off.
    elsewhere(null, null);
    expect([codeLines.shown, shownOnPage()]).toEqual([false, false]);
  });
});
