/**
 * Compares two dotted version numbers.
 *
 * Returns 1 when `second` is the newer one, -1 when `first` is, and 0 when
 * they are the same: the sign is that of `second - first`, the reverse of a
 * sort comparator. Parts that are not numbers are ignored.
 */
export function compareVersions(first: string, second: string): number {
  if (first === second) return 0;
  const numbers = (version: string): number[] =>
    version
      .split('.')
      .filter((part) => /^[0-9]+$/.test(part))
      .map((part) => Number.parseInt(part, 10));
  const a = numbers(first);
  const b = numbers(second);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const left = a[index] ?? 0;
    const right = b[index] ?? 0;
    if (left > right) return -1;
    if (left < right) return 1;
  }
  return 0;
}
