/** The language of the page following the account's setting, and the device's copy of it. */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { setLocale } from '$lib/i18n/paraglide/runtime.js';

import {
  browserLocale,
  followAccountLanguage,
  shownLanguage,
  startLocale,
} from '../../../src/app/locale.ts';

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  void setLocale('en', { reload: false });
});

describe('following the account', () => {
  it('keeps a copy on the device that the next page starts in, and none for auto', () => {
    void setLocale('en', { reload: false });
    const reload = vi.fn();
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      href: window.location.href,
      reload,
    } as unknown as Location);
    expect(followAccountLanguage('ko')).toBe(true);
    expect(reload).toHaveBeenCalledOnce();
    expect(localStorage.getItem('fvoci:locale')).toBe('ko');
    expect(startLocale()).toBe('ko');
    // Already in it: nothing to do.
    expect(followAccountLanguage('ko')).toBe(false);
    expect(reload).toHaveBeenCalledOnce();
    followAccountLanguage('auto');
    expect(localStorage.getItem('fvoci:locale')).toBeNull();
    expect(startLocale()).toBe(browserLocale());
  });

  it('does not load the page again when the device cannot keep the copy: no loop', () => {
    void setLocale('en', { reload: false });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('full', 'QuotaExceededError');
    });
    expect(followAccountLanguage('ko')).toBe(false);
    expect(startLocale()).toBe('en');
  });
});

describe('the language the picker shows', () => {
  it('is the language the page is in, or auto when that is the browser’s and the account says auto', () => {
    void setLocale('en', { reload: false });
    expect(shownLanguage('auto')).toBe(browserLocale() === 'en' ? 'auto' : 'en');
    // Another device chose Korean: until the page follows, it shows what the page is in.
    expect(shownLanguage('ko')).toBe('en');
    void setLocale('ko', { reload: false });
    expect(shownLanguage('ko')).toBe('ko');
  });
});
