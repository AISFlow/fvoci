/**
 * Places in a list that people put in order by hand: each thing keeps a
 * place of its own, so that moving one writes that one alone, and two people
 * moving two things at once both keep their moves.
 *
 * A place is the server's order of a wiki's pages (`order_between` and
 * `spread_orders` in crates/model): a fraction written in the digits
 * `0-9A-Za-z` without its last zeros. Places compare as strings, by code
 * unit and not by a locale, so every browser lists them alike; there is
 * always one between two that differ. At an end it steps by one digit,
 * between two it goes half way.
 *
 * A list may hold things without a place (made before places were kept):
 * they come after those with one. The first thing moved, added or copied
 * gives each of them the place it shows at (`Placing.keep`), after which a
 * move is one write.
 */

/** The digits of a place, least first: in the order of their code units. */
const DIGITS = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz';
const BASE = DIGITS.length;
const TOP = BASE - 1;
/** The longest place given: past it, the list is numbered anew (as the server's `ORDER_MAX_CHARS`). */
const LONGEST = 32;

/** The digits of a place as their indices in `DIGITS`; undefined for what is no place. */
function digitsOf(place: string): number[] | undefined {
  // By code point: what is not one of the digits, of whatever length, is no place.
  const digits = Array.from(place, (char) => DIGITS.indexOf(char));
  if (digits.length === 0 || digits.includes(-1) || digits.at(-1) === 0) return undefined;
  return digits;
}

/** Whether a value is a place: a string `placeBetween` makes and takes. */
export function isPlace(value: unknown): value is string {
  return typeof value === 'string' && digitsOf(value) !== undefined;
}

/** A place above `low` with nothing above it: `low` up to its first digit below the top, that one up. */
function stepAbove(low: readonly number[]): number[] {
  let atTop = 0;
  while (low[atTop] === TOP) atTop += 1;
  return [...low.slice(0, atTop), (low[atTop] ?? 0) + 1];
}

/**
 * A place below `high` with nothing below it: `high` up to its first digit that is not
 * zero, that one down; below a one, a zero and the top, which leaves the next step its room.
 */
function stepBelow(high: readonly number[]): number[] {
  let zeros = 0;
  while (high[zeros] === 0) zeros += 1;
  // A place does not end in a zero, so there is a digit after them.
  const digit = high[zeros] ?? 1;
  return [...high.slice(0, zeros), ...(digit === 1 ? [0, TOP] : [digit - 1])];
}

const midpoint = (one: number, other: number): number => Math.floor((one + other) / 2);

/** A place between `low` and `high`, where `low` is less than `high`. */
function halfway(low: readonly number[], high: readonly number[]): number[] {
  const digitOf = (digits: readonly number[], at: number): number => digits[at] ?? 0;
  // Where the two first differ, `low` taken as going on in zeros.
  let at = 0;
  while (high[at] === digitOf(low, at)) at += 1;
  const [below, above] = [digitOf(low, at), digitOf(high, at)];
  const made = high.slice(0, at);
  if (above - below > 1) return [...made, midpoint(below, above)];
  // Digits that follow each other: `high` cut here is between the two.
  if (high.length > at + 1) return [...made, above];
  // And `high` ends here: the place goes on from `low`, half way to the top.
  const rest = low.slice(at + 1);
  let atTop = 0;
  while (rest[atTop] === TOP) atTop += 1;
  return [...made, below, ...rest.slice(0, atTop), midpoint(digitOf(rest, atTop) + 1, BASE)];
}

/**
 * A place after `before` (undefined: the first place) and before `after`
 * (undefined: the last place). Undefined when a bound is no place, or
 * `before` is not less than `after`.
 */
export function placeBetween(
  before: string | undefined,
  after: string | undefined,
): string | undefined {
  const low = before === undefined ? undefined : digitsOf(before);
  const high = after === undefined ? undefined : digitsOf(after);
  if ((before !== undefined && !low) || (after !== undefined && !high)) return undefined;
  if (before !== undefined && after !== undefined && before >= after) return undefined;
  let made: number[];
  if (low && high) made = halfway(low, high);
  else if (low) made = stepAbove(low);
  else if (high) made = stepBelow(high);
  else made = [BASE / 2];
  return made.map((digit) => DIGITS[digit]).join('');
}

/**
 * `count` places, each after the one before, spread evenly over the places
 * of as few digits as leave room between every two and at both ends.
 */
export function spreadPlaces(count: number): string[] {
  const slots = count + 1;
  let width = 1;
  while (BASE ** width < slots * 2) width += 1;
  const span = BASE ** width;
  return Array.from({ length: count }, (_, index) => {
    let value = Math.floor(((index + 1) * span) / slots);
    let place = '';
    for (let digit = 0; digit < width; digit += 1) {
      place = DIGITS.charAt(value % BASE) + place;
      value = Math.floor(value / BASE);
    }
    return place.replace(/0+$/, '');
  });
}

/** A thing of a list, in the order the list shows, and its place if it has one. */
export interface Placed {
  readonly id: string;
  readonly place: string | undefined;
}

/** A place to write: the thing, and the place it takes. */
export interface PlaceWrite {
  id: string;
  place: string;
}

/** Where a thing goes, and the places of others written with it that change nothing shown. */
export interface Placing {
  /** The place of the thing. */
  place: string;
  /**
   * Places that keep the others where they show: those without one get the
   * place they show at; where the thing goes into a tie (two of one place,
   * moved there at once) those of the tie after it go up; where places would
   * grow too long, every one is given a new place. No step of undo.
   */
  keep: PlaceWrite[];
}

/**
 * The places of a list as it shows, those without one (they come last)
 * given the places they show at: each a step after the one before. The same
 * list gives the same places in every browser.
 */
function shownPlaces(list: readonly Placed[]): string[] {
  let last: string | undefined;
  return list.map((item) => {
    last = item.place ?? placeBetween(last, undefined) ?? '';
    return last;
  });
}

/**
 * Places for a thing put into a tie at `at` and for those of the tie after
 * it, each above the one before, from the tie up to the next place; undefined
 * where there is no tie there, or no room.
 */
function raiseTie(shown: readonly string[], at: number): string[] | undefined {
  const tie = shown[at];
  if (at === 0 || tie === undefined || shown[at - 1] !== tie) return undefined;
  let end = at;
  while (shown[end] === tie) end += 1;
  const raised: string[] = [];
  let low = tie;
  for (let count = at; count <= end; count += 1) {
    const next = placeBetween(low, shown[end]);
    if (next === undefined || next.length > LONGEST) return undefined;
    raised.push(next);
    low = next;
  }
  return raised;
}

const writesOf = (places: ReadonlyMap<string, string>): PlaceWrite[] =>
  [...places].map(([id, place]) => ({ id, place }));

/** The place of a thing put at index `at` of `list` (as it shows, without the thing). */
export function placeAt(list: readonly Placed[], at: number): Placing {
  const shown = shownPlaces(list);
  const to = Math.max(0, Math.min(at, list.length));
  const keep = new Map<string, string>();
  list.forEach((item, index) => {
    if (item.place === undefined) keep.set(item.id, shown[index] ?? '');
  });
  const place = placeBetween(shown[to - 1], shown[to]);
  if (place !== undefined && place.length <= LONGEST) return { place, keep: writesOf(keep) };
  const raised = raiseTie(shown, to);
  if (raised) {
    list.slice(to, to + raised.length - 1).forEach((item, index) => {
      keep.set(item.id, raised[index + 1] ?? '');
    });
    return { place: raised[0] ?? '', keep: writesOf(keep) };
  }
  const spread = spreadPlaces(list.length + 1);
  list.forEach((item, index) => {
    const given = spread[index < to ? index : index + 1] ?? '';
    if (item.place === given) keep.delete(item.id);
    else keep.set(item.id, given);
  });
  return { place: spread[to] ?? '', keep: writesOf(keep) };
}

/**
 * The place that moves `id` to index `to` of `list` (as it shows), what
 * keeps the others where they show, and `was`: where it shows now, as a place
 * among the others' once written, which an undo puts back. That is the
 * place it has, or shows at where it has none, unless their writes passed it.
 */
export function movedTo(
  list: readonly Placed[],
  id: string,
  to: number,
): Placing & { was: string } {
  // Of the whole list as it shows, the moved one too: whoever moves what, a thing
  // without a place is given the same one.
  const shown = shownPlaces(list);
  const at = list.findIndex((item) => item.id === id);
  const others = list.flatMap((item, index) =>
    item.id === id ? [] : [{ id: item.id, place: shown[index] }],
  );
  const placing = placeAt(others, to);
  const keep = new Map<string, string>();
  list.forEach((item, index) => {
    if (item.place === undefined && item.id !== id) keep.set(item.id, shown[index] ?? '');
  });
  for (const write of placing.keep) keep.set(write.id, write.place);
  const placeOf = (other: Placed | undefined) => other && (keep.get(other.id) ?? other.place);
  // Its neighbours before it moved: what an undo puts it back between.
  const was = placeAmong(shown[at] ?? placing.place, placeOf(others[at - 1]), placeOf(others[at]));
  return { place: placing.place, keep: writesOf(keep), was };
}

/**
 * The place `was` where it is between `before` and `after`, the places of
 * its neighbours once the others' are written; else one between them (the
 * others' writes went past it: a tie raised, the list numbered anew).
 */
function placeAmong(was: string, before: string | undefined, after: string | undefined): string {
  if ((before === undefined || before < was) && (after === undefined || was < after)) return was;
  const between = placeBetween(before, after);
  return between !== undefined && between.length <= LONGEST ? between : was;
}

/** The place of a copy of `id` right after it, and what keeps the others where they show. */
export function placeAfter(list: readonly Placed[], id: string): Placing {
  const at = list.findIndex((item) => item.id === id);
  return placeAt(list, at < 0 ? list.length : at + 1);
}

/** The place of a new thing at the end of the list, and what keeps the others where they show. */
export function placeAtEnd(list: readonly Placed[]): Placing {
  return placeAt(list, list.length);
}
