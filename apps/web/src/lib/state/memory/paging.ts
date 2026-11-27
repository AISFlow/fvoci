/**
 * The pages of a list, cut as the server cuts them: `page` is a whole number
 * from 0 to 2147483647 and `perPage` one from 1 to 1000, 50 when left out;
 * anything else is 400 at the parameter.
 */
import { BackendError, problemOf } from '../backend.ts';
import { MAX_PER_PAGE } from '../page-reads.ts';
import type { Paging } from '../trash-reads.ts';

/** The largest `page` the server takes. */
const MAX_PAGE = 2_147_483_647;

/** The `perPage` of a request without one. */
const DEFAULT_PER_PAGE = 50;

function checked(parameter: string, count: number, least: number, most: number): number {
  if (Number.isInteger(count) && count >= least && count <= most) return count;
  const detail = `invalid \`${parameter}\` parameter: ${count < least ? `below ${String(least)}` : `above ${String(most)}`}`;
  throw new BackendError(400, {
    ...problemOf(400, detail),
    errors: [{ parameter, detail, reason: 'invalid' }],
  });
}

/** The page `paging` asks for of `items`. */
export function pageOf<T>(items: readonly T[], paging: Paging | undefined): T[] {
  const page = checked('page', paging?.page ?? 0, 0, MAX_PAGE);
  const perPage = checked('perPage', paging?.perPage ?? DEFAULT_PER_PAGE, 1, MAX_PER_PAGE);
  return items.slice(page * perPage, (page + 1) * perPage);
}
