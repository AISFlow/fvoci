/**
 * What the value of an attribute of a body may be: the rules the list of
 * schema.ts gives each attribute, and the rules of a link's address and of
 * a callout's tone that the editor's own checks use as well.
 */

/** Whether a value is acceptable for an attribute. */
export type AttributeRule = (value: unknown) => boolean;

export const isText =
  (max: number): AttributeRule =>
  (value) =>
    typeof value === 'string' && value.length <= max;
export const isNonEmptyText =
  (max: number): AttributeRule =>
  (value) =>
    typeof value === 'string' && value.length > 0 && value.length <= max;
export const isIntegerBetween =
  (low: number, high: number): AttributeRule =>
  (value) =>
    typeof value === 'number' && Number.isInteger(value) && value >= low && value <= high;
export const isBoolean: AttributeRule = (value) => typeof value === 'boolean';
/** One of a few strings, as they are written. */
export const isOneOf =
  (allowed: readonly string[]): AttributeRule =>
  (value) =>
    typeof value === 'string' && allowed.includes(value);

const NIL_ID = '00000000-0000-0000-0000-000000000000';
/** A user's identifier as the server writes one: a UUID in lower case with hyphens, not the nil one. */
export const isUserId: AttributeRule = (value) =>
  typeof value === 'string' &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value) &&
  value !== NIL_ID;
/** A block's identifier, which has the form of a user's: whether it names anything is not asked. */
export const isBlockId: AttributeRule = isUserId;
/**
 * An uploaded file's identifier (`<secret>.<ext>` or a block id): single path
 * segment of ASCII letters, digits, `.`, `_`, `-`, never `.` or `..` traversal.
 */
export const isFileId: AttributeRule = (value) =>
  typeof value === 'string' &&
  value !== '.' &&
  !value.includes('..') &&
  /^[A-Za-z0-9._-]{1,100}$/.test(value);
/** A language tag of a code block: `rust`, `c++`, `objective-c`. */
export const isLanguage: AttributeRule = (value) =>
  typeof value === 'string' && /^[A-Za-z0-9+#._-]{1,40}$/.test(value);

/**
 * The schemes a link may use. Anything without a scheme is a path inside
 * the app. Whitespace and control characters do not hide a scheme: browsers
 * drop them before reading one, so this does too. A leading `//` or a
 * backslash would leave the site and is refused.
 */
const LINK_SCHEMES = ['http', 'https', 'mailto'];

export function isAllowedHref(href: unknown): href is string {
  if (typeof href !== 'string' || href.length === 0 || href.length > 2048) return false;
  // eslint-disable-next-line no-control-regex -- control characters are the point
  const compact = href.replace(/[\u0000- \u007f]+/g, '');
  if (compact.length === 0) return false;
  const scheme = /^([A-Za-z][A-Za-z0-9+.-]*):/.exec(compact)?.[1];
  if (scheme !== undefined) return LINK_SCHEMES.includes(scheme.toLowerCase());
  return !compact.startsWith('//') && !compact.includes('\\');
}

/**
 * The tones of a callout, as stored: what it says of what it holds (a note,
 * a tip, a warning, a danger), drawn each with its own icon and colour.
 */
export const CALLOUT_TONES = ['info', 'tip', 'warn', 'danger'] as const;

export type CalloutTone = (typeof CALLOUT_TONES)[number];

/** The tone a callout has when nothing says another: a plain note. */
export const DEFAULT_TONE: CalloutTone = 'info';

export const isCalloutTone = (value: unknown): value is CalloutTone =>
  isOneOf(CALLOUT_TONES)(value);
