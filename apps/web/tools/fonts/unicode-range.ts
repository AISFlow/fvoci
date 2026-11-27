/** Sets of code points and the `unicode-range` notation of CSS. */

export type Range = readonly [first: number, last: number];

/** Expands ranges into single code points. */
export function expand(ranges: readonly Range[]): number[] {
  const codePoints: number[] = [];
  for (const [first, last] of ranges) {
    for (let code = first; code <= last; code += 1) codePoints.push(code);
  }
  return codePoints;
}

/** Merges code points into ascending ranges without gaps or overlaps. */
export function toRanges(codePoints: Iterable<number>): Range[] {
  const sorted = [...new Set(codePoints)].sort((a, b) => a - b);
  const ranges: [number, number][] = [];
  for (const code of sorted) {
    const current = ranges.at(-1);
    if (current?.[1] === code - 1) current[1] = code;
    else ranges.push([code, code]);
  }
  return ranges;
}

const hex = (code: number): string => code.toString(16).toUpperCase();

/** `U+AC00-D7A3,U+3131`: the value of a `unicode-range` descriptor. */
export function formatUnicodeRange(codePoints: Iterable<number>): string {
  return toRanges(codePoints)
    .map(([first, last]) => (first === last ? `U+${hex(first)}` : `U+${hex(first)}-${hex(last)}`))
    .join(',');
}

/** The code points of a string, as a text to hand to the subsetter. */
export function toText(codePoints: Iterable<number>): string {
  return String.fromCodePoint(...codePoints);
}
