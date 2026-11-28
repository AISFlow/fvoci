/**
 * The order of the reader's stars after one is dropped among them (the
 * sidebar's "즐겨찾기"): the star taken out, and put back at the place it was
 * dropped at, counted without it. Undefined where nothing moves.
 */
import type { Destination } from './drop-target.ts';

export function favoriteOrderAfter(
  ids: readonly string[],
  itemId: string,
  to: Destination,
): string[] | undefined {
  if (to.kind !== 'index' || !ids.includes(itemId)) return undefined;
  const rest = ids.filter((id) => id !== itemId);
  rest.splice(Math.min(Math.max(to.index, 0), rest.length), 0, itemId);
  return rest.every((id, at) => id === ids[at]) ? undefined : rest;
}
