/**
 * The name a person is shown by, held against the vectors the server passes
 * too (`contracts/name-vectors.json`), and the lists of codes against the
 * contract's.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import {
  CREDENTIALS,
  emptyName,
  nameContains,
  NAME_SHOWN,
  searchedNames,
  shownName,
  TITLES,
  type PersonName,
} from '$lib/logic/person-names.ts';

const CONTRACTS = path.resolve(import.meta.dirname, '../../../../contracts');

interface Vectors {
  cases: { note: string; name: Partial<PersonName>; shown: string }[];
}

const vectors = JSON.parse(
  readFileSync(path.join(CONTRACTS, 'name-vectors.json'), 'utf8'),
) as Vectors;

const schemas = (
  JSON.parse(readFileSync(path.join(CONTRACTS, 'openapi.json'), 'utf8')) as {
    components: { schemas: Record<string, { enum?: string[] }> };
  }
).components.schemas;

describe('the name a person is shown by', () => {
  it('is the one the vectors say', () => {
    expect(vectors.cases.length).toBeGreaterThan(10);
    for (const { note, name, shown } of vectors.cases) {
      expect(shownName({ ...emptyName(), ...name }), note).toBe(shown);
    }
  });

  it('finds a person by every part of their name', () => {
    const kim: PersonName = {
      ...emptyName(),
      firstName: '영환',
      lastName: '김',
      latinFirstName: 'Younghwan',
      latinLastName: 'Kim',
      customName: 'YH',
    };
    for (const wanted of ['김영환', '영환', 'younghwan kim', 'kim younghwan', 'yh', '김 영환']) {
      expect(nameContains(kim, wanted), wanted).toBe(true);
    }
    expect(nameContains(kim, 'lee')).toBe(false);
    // What a mention is found by besides the name shown: none of them empty.
    expect(searchedNames({ ...emptyName(), latinFirstName: 'Ada' })).toEqual(['Ada', 'Ada']);
    expect(searchedNames(emptyName())).toEqual([]);
  });
});

describe('the lists of codes', () => {
  it('are the contract’s, in its order', () => {
    expect(NAME_SHOWN).toEqual(schemas.NameShown?.enum);
    expect(TITLES).toEqual(schemas.Title?.enum);
    expect(CREDENTIALS).toEqual(schemas.Credential?.enum);
  });
});
