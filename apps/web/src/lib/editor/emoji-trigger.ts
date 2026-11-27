/**
 * When a colon starts the name of an emoji.
 *
 * `:rock` offers 🚀. `10:30` and `http:` must not: there the colon follows
 * an ASCII letter or digit. Anything else may stand in front of it: nothing
 * at all, a space, punctuation, and above all a letter of a script that puts
 * no space between words, such as Korean (`안녕:`), Chinese or Japanese.
 */

export interface EmojiTrigger {
  /** Where the colon is, counted in UTF-16 code units from the start of the text. */
  index: number;
  /** What was typed after the colon. May be empty. */
  query: string;
}

/** What a name may be typed with: letters and digits of any script, and `_ + -`. */
const QUERY = /^[\p{L}\p{M}\p{N}_+-]*$/u;
const ASCII_WORD = /[A-Za-z0-9]/;

/**
 * The name being typed at the end of `text` (the text of the line up to the
 * cursor), if the colon in front of it can start one.
 */
export function findEmojiTrigger(text: string): EmojiTrigger | undefined {
  const index = text.lastIndexOf(':');
  if (index === -1) return undefined;
  const query = text.slice(index + 1);
  if (!QUERY.test(query)) return undefined;
  // The character in front, whole even when it takes two code units.
  const before = Array.from(text.slice(Math.max(0, index - 2), index)).at(-1) ?? '';
  if (ASCII_WORD.test(before)) return undefined;
  return { index, query };
}
