/**
 * The link of a card as it is copied: the address as text, which a body
 * takes as a reference when pasted, and the same address under the card's
 * number and title for what takes a link with words; the text alone where
 * the browser writes nothing else.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { cardLinkText, copyCardLink } from '$lib/components/card-number/card-link.ts';

const ADDRESS = 'https://app.example/w/ws/boards/b/v/c';

/** A clipboard that keeps what is written to it. */
function clipboard(withWrite: boolean): { texts: string[]; items: Record<string, Blob>[] } {
  const kept = { texts: [] as string[], items: [] as Record<string, Blob>[] };
  const writeText = (text: string): Promise<void> => {
    kept.texts.push(text);
    return Promise.resolve();
  };
  const write = (items: { parts: Record<string, Blob> }[]): Promise<void> => {
    kept.items.push(...items.map((item) => item.parts));
    return Promise.resolve();
  };
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: withWrite ? { writeText, write } : { writeText },
  });
  return kept;
}

afterEach(() => {
  vi.unstubAllGlobals();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
});

describe('the link of a card', () => {
  it('says the number and the title, or the title alone, or the words for none', () => {
    expect(cardLinkText({ title: 'Fix the login', number: 12 })).toBe('#12 Fix the login');
    expect(cardLinkText({ title: 'A template' })).toBe('A template');
    expect(cardLinkText({ title: '', number: 3 })).toBe('#3 Untitled');
  });

  it('is copied as the address, and as the address under the number and title', async () => {
    vi.stubGlobal(
      'ClipboardItem',
      class {
        readonly parts: Record<string, Blob>;
        constructor(parts: Record<string, Blob>) {
          this.parts = parts;
        }
      },
    );
    const kept = clipboard(true);
    await copyCardLink(ADDRESS, { title: 'Fix <the> login', number: 12 });
    expect(kept.texts).toEqual([]);
    const [item] = kept.items;
    expect(await item?.['text/plain']?.text()).toBe(ADDRESS);
    const html = new DOMParser().parseFromString(
      (await item?.['text/html']?.text()) ?? '',
      'text/html',
    );
    const link = html.querySelector('a');
    expect(link?.getAttribute('href')).toBe(ADDRESS);
    expect(link?.textContent).toBe('#12 Fix <the> login');
    expect(html.querySelector('the')).toBeNull();
  });

  it('is copied as the address alone where the browser writes text only', async () => {
    const kept = clipboard(false);
    await copyCardLink(ADDRESS, { title: 'Plan', number: 1 });
    expect(kept.texts).toEqual([ADDRESS]);
  });
});
