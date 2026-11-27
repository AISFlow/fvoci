/**
 * Reading a list the server answers in pages: every list takes `perPage`
 * from 1 to {@link MAX_PER_PAGE}, so what needs more reads page after page.
 */
import type { Paging } from './trash-reads.ts';

/** The most a page of any list holds. */
export const MAX_PER_PAGE = 1000;

/** One page of a list, as `read` asks for it. */
export type PageReader<T> = (paging: Paging) => Promise<T[]>;

/** Every item of a list, read a page of {@link MAX_PER_PAGE} at a time until a page is short. */
export function allPages<T>(read: PageReader<T>): Promise<T[]> {
  return firstOf(read, Infinity);
}

/**
 * Every item of a long list without waiting for one page before asking for
 * the next: after a full first page the next two are asked for together,
 * then the next four, twice as many each time, until a page is short. A list
 * of fewer than three full pages is read in two round trips (one of fewer than
 * 2000 items in as many as before, with a page more asked for), at the cost of
 * at most as many empty pages as the last wave asked for. Pages are put together in
 * their order; an item that moved from one page to a later one between two
 * readings (the list changed meanwhile) is kept once, where it was first
 * read (by its `id`).
 */
export async function pagesTogether<T extends { id: string }>(read: PageReader<T>): Promise<T[]> {
  const pages = [await read({ page: 0, perPage: MAX_PER_PAGE })];
  for (let ahead = 2; pages.at(-1)?.length === MAX_PER_PAGE; ahead *= 2) {
    const from = pages.length;
    const next = await Promise.all(
      Array.from({ length: ahead }, (_, index) =>
        read({ page: from + index, perPage: MAX_PER_PAGE }),
      ),
    );
    const short = next.findIndex((page) => page.length < MAX_PER_PAGE);
    pages.push(...(short === -1 ? next : next.slice(0, short + 1)));
  }
  const seen = new Set<string>();
  return pages.flat().filter((item) => {
    if (seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  });
}

/**
 * The first `count` items of a list, or all of them if it has fewer: one
 * page if they fit in one, else pages of {@link MAX_PER_PAGE} until there are
 * enough or a page is short.
 */
export async function firstOf<T>(read: PageReader<T>, count: number): Promise<T[]> {
  if (count <= 0) return [];
  const perPage = Math.min(count, MAX_PER_PAGE);
  const items: T[] = [];
  for (let page = 0; items.length < count; page += 1) {
    const one = await read({ page, perPage });
    items.push(...one);
    if (one.length < perPage) break;
  }
  return items.length > count ? items.slice(0, count) : items;
}
