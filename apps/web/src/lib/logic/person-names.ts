/**
 * A person's name: its parts, which of them the person is shown by, their
 * title and credentials. The server makes the name a person is shown by
 * (`displayName`); the rule here is the same one
 * (`crates/model/src/person_name.rs`), for the preview of the settings, and
 * passes the same vectors (`contracts/name-vectors.json`).
 */

/** Which name a person is shown by: the contract's `NameShown`. */
export type NameShown = (typeof NAME_SHOWN)[number];
/** A title written before a name: the contract's `Title`. */
export type Title = (typeof TITLES)[number];
/** A degree or a professional qualification written after a name: the contract's `Credential`. */
export type Credential = (typeof CREDENTIALS)[number];

/** The parts of a person's name, any of which may be empty. */
export interface PersonName {
  firstName: string;
  lastName: string;
  latinFirstName: string;
  latinLastName: string;
  customName: string;
  nameShown: NameShown;
  title: Title | null;
  credentials: Credential[];
}

/**
 * What a change of a name names, as `patchMe` takes it: a member left out
 * stays as it is; `null` is none, but for the choice, which is always one.
 */
export type NameChange = {
  [Member in keyof PersonName]?: Member extends 'nameShown' ? NameShown : PersonName[Member] | null;
};

/** The choices, in the order the server goes down when one is empty. */
export const NAME_SHOWN = ['name', 'latin_name', 'custom_name', 'username'] as const;

/** The titles, in the order of the server's list. */
export const TITLES = ['dr', 'prof'] as const;

/** The credentials, in the order of the server's list. */
export const CREDENTIALS = [
  'phd',
  'md',
  'dds',
  'jd',
  'edd',
  'mba',
  'ms',
  'ma',
  'cpa',
  'cfa',
  'pe',
  'rn',
  'kr_attorney',
  'kr_patent_attorney',
  'kr_tax_accountant',
  'kr_cpa',
  'kr_architect',
  'kr_professional_engineer',
  'kr_pharmacist',
  'kr_korean_medicine_doctor',
] as const;

/** The most credentials a person lists. */
export const MAX_CREDENTIALS = 3;

/** A name with nothing in it, as a new account has. */
export function emptyName(): PersonName {
  return {
    firstName: '',
    lastName: '',
    latinFirstName: '',
    latinLastName: '',
    customName: '',
    nameShown: 'name',
    title: null,
    credentials: [],
  };
}

/** White space as the server trims it (Unicode's `White_Space`). */
const SPACE =
  '[\\u0009-\\u000d\\u0020\\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]';
const AROUND = new RegExp(`^${SPACE}+|${SPACE}+$`, 'gu');

function trimmed(part: string): string {
  return part.replace(AROUND, '');
}

/** The blocks of Hangul, Han and Kana, as the server has them. */
const EAST_ASIAN = /[ᄀ-ᇿ々-〇぀-ゟ゠-ヿ㄰-㆏ㇰ-ㇿ㐀-䶿一-鿿ꥠ-꥿가-퟿豈-﫿ｦ-ﾟﾠ-ￜ\u{20000}-\u{323af}]/u;
const LETTER = /\p{Alphabetic}/u;

/** Whether a part has a letter of Hangul, Han or Kana and no letter of another script. */
function isEastAsian(part: string): boolean {
  let any = false;
  for (const char of part) {
    if (EAST_ASIAN.test(char)) any = true;
    else if (LETTER.test(char)) return false;
  }
  return any;
}

function joined(first: string, last: string): string {
  return first && last ? `${first} ${last}` : first || last;
}

/**
 * The name as the person writes it: the family name and the given name
 * joined, family name first, when both are Hangul, Han or Kana alone
 * ("김영환"); the given name, a space and the family name otherwise.
 */
export function fullName(name: PersonName): string {
  const first = trimmed(name.firstName);
  const last = trimmed(name.lastName);
  return isEastAsian(first) && isEastAsian(last) ? `${last}${first}` : joined(first, last);
}

/** The name in Latin letters: the given name, a space and the family name. */
export function latinName(name: PersonName): string {
  return joined(trimmed(name.latinFirstName), trimmed(name.latinLastName));
}

/** The name one choice gives, empty when it has none; the login name is none. */
export function nameOf(name: PersonName, shown: NameShown): string {
  if (shown === 'name') return fullName(name);
  if (shown === 'latin_name') return latinName(name);
  if (shown === 'custom_name') return trimmed(name.customName);
  return '';
}

/**
 * The name the person is shown by, as the server makes `displayName`: the
 * one they chose, or the next one down the list when it is empty. Empty when
 * that is the login name.
 */
export function shownName(name: PersonName): string {
  const start = Math.max(0, NAME_SHOWN.indexOf(name.nameShown));
  for (const shown of NAME_SHOWN.slice(start)) {
    const found = nameOf(name, shown);
    if (found) return found;
  }
  return '';
}

/**
 * The names a person is found by, as the server searches them: as written or
 * in Latin letters, either way round, and the name the person wrote to be
 * shown by. Empty ones left out.
 */
export function searchedNames(name: PersonName): string[] {
  return [
    fullName(name),
    latinName(name),
    joined(trimmed(name.latinLastName), trimmed(name.latinFirstName)),
    joined(trimmed(name.lastName), trimmed(name.firstName)),
    name.customName,
  ].filter((found) => found !== '');
}

/** Whether `wanted`, in small letters, is in one of the names a person is found by. */
export function nameContains(name: PersonName, wanted: string): boolean {
  return searchedNames(name).some((found) => found.toLowerCase().includes(wanted));
}
