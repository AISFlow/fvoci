/** Flatten infinite-query event pages in server order. */
export function flattenEventPages<T>(pages: { items: T[] }[] | undefined): T[] {
  return pages?.flatMap((page) => page.items) ?? [];
}
