/**
 * A name with its title and credentials, as the profile card and the lists
 * of members write it, in English and in Korean.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { credentialAfterName, titledName } from '$lib/components/person/titled-name.ts';
import { setLocale } from '$lib/i18n/paraglide/runtime.js';
import { emptyName, type PersonName } from '$lib/logic/person-names.ts';

const named = (change: Partial<PersonName>): PersonName => ({ ...emptyName(), ...change });

afterEach(() => {
  void setLocale('en', { reload: false });
});

describe('titledName', () => {
  it('is the name alone without a title, credentials or the parts', () => {
    expect(titledName('Ada', undefined)).toBe('Ada');
    expect(titledName('Ada', named({}))).toBe('Ada');
  });

  it('puts the title before the name in English, the credentials after it in their order', () => {
    void setLocale('en', { reload: false });
    expect(titledName('Younghwan Kim', named({ title: 'dr' }))).toBe('Dr. Younghwan Kim');
    expect(titledName('Younghwan Kim', named({ title: 'dr', credentials: ['phd'] }))).toBe(
      'Dr. Younghwan Kim, PhD',
    );
    expect(
      titledName('Kim', named({ title: 'prof', credentials: ['md', 'phd', 'kr_attorney'] })),
    ).toBe('Prof. Kim, MD, PhD, Attorney at law (Korea)');
    expect(titledName('kim', named({ credentials: ['mba'] }))).toBe('kim, MBA');
  });

  it('puts the title after the name in Korean, and the international credentials by abbreviation', () => {
    void setLocale('ko', { reload: false });
    expect(titledName('김영환', named({ title: 'dr' }))).toBe('김영환 박사');
    expect(
      titledName('김영환', named({ title: 'prof', credentials: ['phd', 'kr_attorney'] })),
    ).toBe('김영환 교수, PhD, 변호사');
    expect(credentialAfterName('edd')).toBe('EdD');
    expect(credentialAfterName('kr_korean_medicine_doctor')).toBe('한의사');
  });
});
