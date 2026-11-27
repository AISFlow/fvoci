/**
 * Names found by what is typed of them, in Korean as well: a syllable is
 * compared by the letters (jamo) it is made of, so that what an input method
 * shows while it composes (`젬` on the way to `제목`) still finds the name, and
 * consonants alone (`ㅈㅁ`) find a name by the first consonants of its
 * syllables. Spaces count for nothing (`할일` finds `할 일`).
 */

const FIRST = 'ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ';
const MIDDLE = 'ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ';
const LAST = [
  '',
  ...'ㄱ ㄲ ㄳ ㄴ ㄵ ㄶ ㄷ ㄹ ㄺ ㄻ ㄼ ㄽ ㄾ ㄿ ㅀ ㅁ ㅂ ㅄ ㅅ ㅆ ㅇ ㅈ ㅊ ㅋ ㅌ ㅍ ㅎ'.split(' '),
];

/** A letter typed as two keys, as the two. */
const TWO_KEYS: Readonly<Record<string, string>> = {
  ㅘ: 'ㅗㅏ',
  ㅙ: 'ㅗㅐ',
  ㅚ: 'ㅗㅣ',
  ㅝ: 'ㅜㅓ',
  ㅞ: 'ㅜㅔ',
  ㅟ: 'ㅜㅣ',
  ㅢ: 'ㅡㅣ',
  ㄳ: 'ㄱㅅ',
  ㄵ: 'ㄴㅈ',
  ㄶ: 'ㄴㅎ',
  ㄺ: 'ㄹㄱ',
  ㄻ: 'ㄹㅁ',
  ㄼ: 'ㄹㅂ',
  ㄽ: 'ㄹㅅ',
  ㄾ: 'ㄹㅌ',
  ㄿ: 'ㄹㅍ',
  ㅀ: 'ㄹㅎ',
  ㅄ: 'ㅂㅅ',
};

const SYLLABLES = { first: 0xac00, last: 0xd7a3 };
/** Consonants alone, as typed before a vowel comes. */
const CONSONANTS = /^[ㄱ-ㅎ]+$/;

const keysOf = (letter: string): string => TWO_KEYS[letter] ?? letter;

/** A text as the keys that type it: each syllable taken apart, spaces left out, in lower case. */
export function lettersOf(text: string): string {
  let letters = '';
  for (const character of text.toLowerCase().replace(/\s+/g, '')) {
    const code = character.codePointAt(0) ?? 0;
    if (code < SYLLABLES.first || code > SYLLABLES.last) {
      letters += keysOf(character);
      continue;
    }
    const at = code - SYLLABLES.first;
    letters += FIRST[Math.floor(at / 588)] ?? '';
    letters += keysOf(MIDDLE[Math.floor((at % 588) / 28)] ?? '');
    letters += keysOf(LAST[at % 28] ?? '');
  }
  return letters;
}

/** The first consonant of each syllable of a text, other characters as they are, without spaces. */
export function initialsOf(text: string): string {
  let initials = '';
  for (const character of text.toLowerCase().replace(/\s+/g, '')) {
    const code = character.codePointAt(0) ?? 0;
    const syllable = code >= SYLLABLES.first && code <= SYLLABLES.last;
    initials += syllable ? (FIRST[Math.floor((code - SYLLABLES.first) / 588)] ?? '') : character;
  }
  return initials;
}

/** Whether `typed` finds `name`: by its letters, or by its first consonants. */
export function finds(name: string, typed: string): boolean {
  const wanted = typed.trim();
  if (wanted === '') return true;
  if (lettersOf(name).includes(lettersOf(wanted))) return true;
  return CONSONANTS.test(wanted) && initialsOf(name).includes(wanted);
}
