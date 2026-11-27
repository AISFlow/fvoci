/** The language setting of an account: `auto`, or a tag in the shape `Intl` reads. */
import { describe, expect, it } from 'vitest';

import { isLanguageSetting, readLanguageSettings } from '$lib/logic/language.ts';

describe('the language setting', () => {
  it('takes auto and the tags the server takes, and not what it refuses', () => {
    for (const taken of [
      'auto',
      'ko',
      'en-US',
      'zh-Hant',
      'pt-BR',
      'de-1901',
      'ko-KR-u-ca-dangi',
    ]) {
      expect(isLanguageSetting(taken)).toBe(true);
    }
    for (const refused of ['', 'Auto', 'abcd', 'en-a', 'en-1', 'en-US-US', 'x'.repeat(65)]) {
      expect(isLanguageSetting(refused)).toBe(false);
    }
  });

  it('reads what the server sent, and auto for what is missing or has no shape', () => {
    expect(readLanguageSettings({ language: 'ko' })).toEqual({ language: 'ko' });
    expect(readLanguageSettings({ language: 'EN-us' })).toEqual({ language: 'EN-us' });
    expect(readLanguageSettings({})).toEqual({ language: 'auto' });
    expect(readLanguageSettings({ language: 7 })).toEqual({ language: 'auto' });
    expect(readLanguageSettings(null)).toEqual({ language: 'auto' });
  });
});

describe('the shape of a tag, as the server reads it', () => {
  it('takes extensions Intl may not, and refuses repeats and an empty extension', () => {
    for (const taken of ['en-u-12', 'en-t-12', 'en-x-a', 'sr-Latn-RS', 'es-419', 'de-DE-1996']) {
      expect(isLanguageSetting(taken)).toBe(true);
    }
    for (const refused of [
      'de-1996-1996',
      'en-u',
      'en-u-ca-u-nu',
      'en-x',
      'en--US',
      'en-US-x-a-b-',
    ]) {
      expect(isLanguageSetting(refused)).toBe(false);
    }
  });
});
