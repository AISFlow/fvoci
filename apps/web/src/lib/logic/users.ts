/** How a person's name is shown. */

export interface UserNames {
  /** The name to sign in with. */
  username: string;
  /** The name the person chose to be shown by; empty for none. */
  displayName: string;
}

/**
 * The name to show for a user, wherever people are shown: the display name
 * with the login name after it, "Hana Kim (@hana)", so that two people who
 * chose the same name, or one who chose another's login name, are still
 * told apart. Without a display name, the login name alone.
 */
export function userDisplayName(user: UserNames): string {
  return user.displayName === '' ? user.username : `${user.displayName} (@${user.username})`;
}

/**
 * Whether the account of the user was deleted: what is left of it is its
 * identifier, without a name. What they wrote stays, and whoever reads it is
 * shown a deleted user.
 */
export function isGone(user: UserNames): boolean {
  return user.username === '';
}

/** The longest display name the server takes, in UTF-16 code units as `String.length` counts. */
export const DISPLAY_NAME_MAX_LENGTH = 64;

/**
 * Characters a display name refuses beside the control characters (the
 * server's rule): those that show nothing (the Hangul fillers and the blank
 * Braille pattern among them, which are letters), break a line or turn the
 * text around, with which a
 * name could look empty, or like another.
 */
const REFUSED_IN_DISPLAY_NAME =
  // The grapheme joiner (U+034F) combines, and so stands outside the class.
  // eslint-disable-next-line no-control-regex -- the control characters are what is refused
  /\u034f|[\u0000-\u001f\u007f-\u009f\u00ad\u061c\u115f\u1160\u180e\u200b-\u200f\u2028-\u202e\u2060-\u206f\u2800\u3164\ufeff\uffa0\ufff9-\ufffb]/u;

/**
 * White space as the server trims it (Unicode's `White_Space`), which is not
 * quite `String.prototype.trim`'s: that one takes U+FEFF too, and not U+0085.
 */
const SPACE =
  '[\\u0009-\\u000d\\u0020\\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]';
const LEADING_SPACE = new RegExp(`^${SPACE}+`, 'u');
const TRAILING_SPACE = new RegExp(`${SPACE}+$`, 'u');

/**
 * Reads a display name as the server does: without the white space around
 * it, empty for none. Answers the name to store, or why it is refused
 * (`too_long`, `invalid_characters`), the reasons the server gives.
 */
export function readDisplayName(
  raw: string,
): { name: string } | { refused: 'too_long' | 'invalid_characters' } {
  const name = raw.replace(LEADING_SPACE, '').replace(TRAILING_SPACE, '');
  if (name.length > DISPLAY_NAME_MAX_LENGTH) return { refused: 'too_long' };
  if (REFUSED_IN_DISPLAY_NAME.test(name)) return { refused: 'invalid_characters' };
  return { name };
}
