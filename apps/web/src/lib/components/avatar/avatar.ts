/** What an avatar shows of a name, and which of the option colours it is drawn on. */

/** The option colours an avatar can have: the fill is `--option-<name>`. */
export const AVATAR_COLORS = [
  'blue',
  'green',
  'yellow',
  'orange',
  'pink',
  'purple',
  'brown',
  'gray',
] as const;

export type AvatarColor = (typeof AVATAR_COLORS)[number];

/** The same colour for the same person every time, whatever the order they appear in. */
export function avatarColor(key: string): AvatarColor {
  let hash = 0;
  for (const character of key) hash = (hash * 31 + (character.codePointAt(0) ?? 0)) % 9973;
  return AVATAR_COLORS[hash % AVATAR_COLORS.length] ?? 'gray';
}

/**
 * One character to stand for a name: its first letter, or for a name written
 * in Hangul, Han or kana its first character (a syllable or an ideograph is a
 * whole unit there). Counts by grapheme, so an emoji or an accented letter
 * stays in one piece.
 */
export function avatarInitial(name: string, locale: string): string {
  const trimmed = name.trim();
  if (trimmed === '') return '?';
  const [first] = new Intl.Segmenter(locale, { granularity: 'grapheme' }).segment(trimmed);
  return (first?.segment ?? '?').toLocaleUpperCase(locale);
}
