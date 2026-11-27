/** What an avatar shows of a name. */
import { describe, expect, it } from 'vitest';

import { AVATAR_COLORS, avatarColor, avatarInitial } from '$lib/components/avatar/avatar.ts';

describe('the character of an avatar', () => {
  it('is the first letter, uppercased, or the first whole character of other scripts', () => {
    expect(avatarInitial('alex rivera', 'en')).toBe('A');
    expect(avatarInitial('김하나', 'ko')).toBe('김');
    expect(avatarInitial('陳美玲', 'zh-Hant')).toBe('陳');
    expect(avatarInitial('佐藤陽翔', 'ja')).toBe('佐');
    expect(avatarInitial('  émile', 'fr')).toBe('É');
  });

  it('keeps an emoji or a combined character in one piece', () => {
    expect(avatarInitial('👩‍🚀 astronaut', 'en')).toBe('👩‍🚀');
    expect(avatarInitial('é́x', 'en')).toBe('É́');
  });

  it('is a question mark for no name', () => {
    expect(avatarInitial('', 'en')).toBe('?');
    expect(avatarInitial('   ', 'en')).toBe('?');
  });
});

describe('the colour of an avatar', () => {
  it('is one of the option colours and the same for the same person every time', () => {
    const colour = avatarColor('01930000-0000-7000-8000-000000000003');
    expect(AVATAR_COLORS).toContain(colour);
    expect(avatarColor('01930000-0000-7000-8000-000000000003')).toBe(colour);
  });

  it('spreads people over the colours', () => {
    const used = new Set(Array.from({ length: 40 }, (_, index) => avatarColor(`person-${index}`)));
    expect(used.size).toBeGreaterThan(4);
  });
});
