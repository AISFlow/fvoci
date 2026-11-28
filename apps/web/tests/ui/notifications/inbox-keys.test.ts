/** The keys of the notifications list (`inboxKey`). */
import { describe, expect, it } from 'vitest';

import { inboxKey } from '../../../src/lib/features/notifications/inbox-keys.ts';

const press = (key: string, more: Partial<KeyboardEvent> = {}) => ({
  key,
  code: /^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : key,
  shiftKey: false,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  isComposing: false,
  keyCode: 0,
  ...more,
});

describe('the keys of the notifications list', () => {
  it('goes from one to the next and back, opens, puts away and marks', () => {
    expect(
      ['j', 'ArrowDown', 'k', 'ArrowUp', 'Enter', 'e', 'u'].map((key) => inboxKey(press(key))),
    ).toEqual(['next', 'next', 'previous', 'previous', 'open', 'archive', 'toggleRead']);
    expect(inboxKey(press('U', { shiftKey: true }))).toBe('readAll');
    expect(inboxKey(press('J', { shiftKey: true }))).toBeUndefined();
  });

  it('reads a Hangul layout by where the key is', () => {
    expect(inboxKey(press('ㅓ', { code: 'KeyJ' }))).toBe('next');
    expect(inboxKey(press('ㄷ', { code: 'KeyE' }))).toBe('archive');
    expect(inboxKey(press('ㅕ', { code: 'KeyU', shiftKey: true }))).toBe('readAll');
  });

  it('is nothing while an input method composes, nor with a command key', () => {
    expect(inboxKey(press('Enter', { isComposing: true }))).toBeUndefined();
    // eslint-disable-next-line @typescript-eslint/no-deprecated -- Safari marks a key an input method takes with keyCode 229 alone
    expect(inboxKey(press('Process', { code: 'KeyJ', keyCode: 229 }))).toBeUndefined();
    expect(inboxKey(press('e', { metaKey: true }))).toBeUndefined();
    expect(inboxKey(press('u', { ctrlKey: true }))).toBeUndefined();
    expect(inboxKey(press('x'))).toBeUndefined();
  });
});
