/** What may be saved as the value of a number, email or URL property. */
import { describe, expect, it } from 'vitest';

import { isEmailEntry, isNumberEntry, isUrlEntry } from '$lib/logic/property-entry.ts';

describe('what a number property takes', () => {
  it.each(['', '0', '123', '-3', '12.5', ' 7', '12 kg', '1e5'])('takes %j', (text) => {
    expect(isNumberEntry(text)).toBe(true);
  });

  it.each(['abc', '.5', '-', ' ', 'kg 12', '₩100'])('refuses %j', (text) => {
    expect(isNumberEntry(text)).toBe(false);
  });
});

describe('what an email property takes', () => {
  it.each(['', 'a@b.co', 'first.last+tag@mail.example.org', '김철수@예시.한국', 'x_y@a-b.io'])(
    'takes %j',
    (text) => {
      expect(isEmailEntry(text)).toBe(true);
    },
  );

  it.each([
    'plain',
    'a@b',
    '@b.co',
    'a@.co',
    'a b@c.de',
    'a@b.c',
    'a@b.co ',
    'a@b@c.de',
    'a@b.c0',
    '<a@b.co>',
  ])('refuses %j', (text) => {
    expect(isEmailEntry(text)).toBe(false);
  });
});

describe('what a URL property takes', () => {
  it.each([
    '',
    'example.com',
    'https://example.com/a?b=c#d',
    ' https://example.com ',
    'localhost:8080',
    'mailto:a@b.co',
    '위키.한국/문서',
  ])('takes %j', (text) => {
    expect(isUrlEntry(text)).toBe(true);
  });

  it.each([' ', '???', 'two words', 'https://exa mple.com', '://'])('refuses %j', (text) => {
    expect(isUrlEntry(text)).toBe(false);
  });
});
