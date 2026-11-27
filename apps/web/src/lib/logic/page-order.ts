/**
 * The places of pages among their siblings, made as the server makes them
 * (`crates/model/src/page.rs`, `order_between`): strings of base-62 digits
 * that sort by code unit, never ending in the lowest digit, so that there is
 * always one between two.
 */
const DIGITS = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz';
const BASE = DIGITS.length;
const TOP = BASE - 1;

function digitsOf(order: string): number[] {
  const digits = Array.from({ length: order.length }, (_, at) => DIGITS.indexOf(order.charAt(at)));
  if (digits.length === 0 || digits.some((digit) => digit < 0) || digits.at(-1) === 0) {
    throw new Error(`no order of the server: ${order}`);
  }
  return digits;
}

const digitAt = (digits: readonly number[], place: number): number => digits[place] ?? 0;

function stepAbove(low: readonly number[]): number[] {
  let atTop = 0;
  while (low[atTop] === TOP) atTop += 1;
  return [...low.slice(0, atTop), digitAt(low, atTop) + 1];
}

function stepBelow(high: readonly number[]): number[] {
  let zeros = 0;
  while (high[zeros] === 0) zeros += 1;
  const made = high.slice(0, zeros);
  const digit = high[zeros] ?? 1;
  if (digit === 1) made.push(0, TOP);
  else made.push(digit - 1);
  return made;
}

function halfway(low: readonly number[], high: readonly number[]): number[] {
  let place = 0;
  while (high[place] === digitAt(low, place)) place += 1;
  const below = digitAt(low, place);
  const above = digitAt(high, place);
  const made = high.slice(0, place);
  if (above - below > 1) made.push(Math.floor((below + above) / 2));
  else if (high.length > place + 1) made.push(above);
  else {
    made.push(below);
    const rest = low.slice(place + 1);
    let atTop = 0;
    while (rest[atTop] === TOP) atTop += 1;
    made.push(...rest.slice(0, atTop), Math.floor((digitAt(rest, atTop) + 1 + BASE) / 2));
  }
  return made;
}

/** A place after `before` and before `after`; either may be left out (the first or last place). */
export function orderBetween(before: string | undefined, after: string | undefined): string {
  const low = before === undefined ? undefined : digitsOf(before);
  const high = after === undefined ? undefined : digitsOf(after);
  let made: number[];
  if (!low && !high) made = [Math.floor(BASE / 2)];
  else if (low && !high) made = stepAbove(low);
  else if (!low && high) made = stepBelow(high);
  else if (low && high && before !== undefined && after !== undefined && before < after) {
    made = halfway(low, high);
  } else throw new Error('the first order is not before the second');
  return made.map((digit) => DIGITS.charAt(digit)).join('');
}
