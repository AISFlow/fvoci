/**
 * What the server takes for one emoji (`Emoji` of `crates/model/src/reaction.rs`),
 * for the in-memory backend: one grapheme of at most eight scalar values, made of
 * parts joined by zero-width joiners. A part is a flag of two regional indicators, a
 * keycap, or a pictograph (or a skin tone alone) with the selector of the emoji's
 * look, a skin tone and, on the black flag, the tags of a region.
 */

/** How many scalar values an emoji has at most. */
const EMOJI_MAX_SCALARS = 8;

/** The zero-width joiner, between the parts of one emoji. */
const JOINER = 0x200d;
/** The variation selector that asks for the emoji's look. */
const EMOJI_LOOK = 0xfe0f;
/** The mark that makes a digit, `#` or `*` a keycap. */
const KEYCAP = 0x20e3;
/** The black flag that tags make the flag of a region. */
const TAGGED_FLAG = 0x1f3f4;
/** The tag that ends the tags of a flag. */
const CANCEL_TAG = 0xe007f;
/** How many tags spell a region after the black flag. */
const REGION_TAGS = { least: 3, most: 6 };

const SINGLES: ReadonlySet<number> = new Set([
  0x00a9, 0x00ae, 0x203c, 0x2049, 0x2122, 0x2139, 0x2328, 0x2388, 0x23cf, 0x24c2, 0x25b6, 0x25c0,
  0x3030, 0x303d, 0x3297, 0x3299,
]);

/** The blocks of the pictographs, without the circled digits and the arrow among the dingbats. */
const RANGES: readonly (readonly [number, number])[] = [
  [0x2194, 0x2199],
  [0x21a9, 0x21aa],
  [0x231a, 0x231b],
  [0x23e9, 0x23f3],
  [0x23f8, 0x23fa],
  [0x25aa, 0x25ab],
  [0x25fb, 0x25fe],
  [0x2600, 0x2775],
  [0x2795, 0x27bf],
  [0x2934, 0x2935],
  [0x2b05, 0x2b07],
  [0x2b1b, 0x2b1c],
  [0x2b50, 0x2b50],
  [0x2b55, 0x2b55],
  [0x1f000, 0x1f1e5],
  [0x1f200, 0x1faff],
  [0x1fc00, 0x1fffd],
];

const isRegionalIndicator = (scalar: number): boolean => scalar >= 0x1f1e6 && scalar <= 0x1f1ff;
const isSkinTone = (scalar: number): boolean => scalar >= 0x1f3fb && scalar <= 0x1f3ff;
const isKeycapBase = (scalar: number): boolean =>
  (scalar >= 0x30 && scalar <= 0x39) || scalar === 0x23 || scalar === 0x2a;
/** The tag forms of the small letters and the digits. */
const isTag = (scalar: number): boolean =>
  (scalar >= 0xe0030 && scalar <= 0xe0039) || (scalar >= 0xe0061 && scalar <= 0xe007a);
const isPictograph = (scalar: number): boolean =>
  !isSkinTone(scalar) &&
  (SINGLES.has(scalar) || RANGES.some(([first, last]) => scalar >= first && scalar <= last));

/** How many scalars from `at` make one part of an emoji; undefined if they make none. */
function partLength(scalars: readonly number[], at: number): number | undefined {
  const [first, second, third] = [scalars[at], scalars[at + 1], scalars[at + 2]];
  if (first === undefined) return undefined;
  if (isRegionalIndicator(first) && second !== undefined && isRegionalIndicator(second)) return 2;
  if (isKeycapBase(first)) {
    if (second === EMOJI_LOOK && third === KEYCAP) return 3;
    return second === KEYCAP ? 2 : undefined;
  }
  if (!isPictograph(first) && !isSkinTone(first)) return undefined;
  let taken = 1;
  if (scalars[at + taken] === EMOJI_LOOK) taken += 1;
  const tone = scalars[at + taken];
  if (tone !== undefined && isSkinTone(tone) && !isSkinTone(first)) {
    taken += 1;
    if (scalars[at + taken] === EMOJI_LOOK) taken += 1;
  }
  const tag = scalars[at + taken];
  if (first === TAGGED_FLAG && tag !== undefined && isTag(tag)) {
    let tags = 0;
    for (let next = scalars[at + taken]; next !== undefined && isTag(next);) {
      tags += 1;
      next = scalars[at + taken + tags];
    }
    if (tags < REGION_TAGS.least || tags > REGION_TAGS.most) return undefined;
    if (scalars[at + taken + tags] !== CANCEL_TAG) return undefined;
    taken += tags + 1;
  }
  return taken;
}

/** Whether the scalars are emoji parts joined by zero-width joiners. */
function hasTheShapeOfAnEmoji(scalars: readonly number[]): boolean {
  let at = 0;
  for (;;) {
    const taken = partLength(scalars, at);
    if (taken === undefined) return false;
    at += taken;
    if (at === scalars.length) return true;
    if (scalars[at] !== JOINER || at + 1 === scalars.length) return false;
    at += 1;
  }
}

let graphemes: Intl.Segmenter | undefined;

/** Why the server would take `text` for no emoji, as it says it; undefined when it is one. */
export function emojiRefusal(text: string): string | undefined {
  // The scalar values, as the server counts them: an emoji's parts are what is asked.
  const scalars: number[] = [];
  for (const scalar of text) scalars.push(scalar.codePointAt(0) ?? 0);
  if (scalars.length === 0) return 'an emoji is not empty';
  if (scalars.length > EMOJI_MAX_SCALARS) {
    return `an emoji is at most ${String(EMOJI_MAX_SCALARS)} characters`;
  }
  graphemes ??= new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  if ([...graphemes.segment(text)].length !== 1 || !hasTheShapeOfAnEmoji(scalars)) {
    return 'a reaction is one emoji';
  }
  return undefined;
}
