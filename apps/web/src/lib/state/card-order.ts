/**
 * Where dragged cards land in the manual order of a view.
 *
 * A view keeps one flat list of card identifiers (`cardOrder`), whatever its
 * columns are. Dropping cards at a place in a column means: take them out of
 * the list, and put them back just before the card that is at that place in
 * the column, or just after the column's last card when they were dropped at
 * its end.
 */

/**
 * The new `cardOrder`.
 *
 * - `shown`: every card of the view in the order it is shown now.
 * - `column`: the cards of the column the drop is in, in shown order,
 *   **without** the moved ones.
 * - `moved`: the cards that were dropped, in the order they are to keep.
 * - `index`: the place in `column` they were dropped at.
 */
export function droppedCardOrder(
  shown: readonly string[],
  column: readonly string[],
  moved: readonly string[],
  index: number,
): string[] {
  const carried = new Set(moved);
  const rest = shown.filter((id) => !carried.has(id));
  const before = column[Math.max(0, index)];
  const last = column.at(-1);
  let at: number;
  if (before !== undefined && index < column.length) at = rest.indexOf(before);
  else if (last !== undefined) at = rest.indexOf(last) + 1;
  else at = rest.length;
  if (at < 0) at = rest.length;
  return [...rest.slice(0, at), ...moved, ...rest.slice(at)];
}

/**
 * A place among the cards a view shows in a column, as the place among all
 * the cards of that column.
 *
 * A filter or a search hides cards from the page that the order still
 * counts, so the two places differ. The dropped cards go right after the
 * nearest shown card above the place that the column has, else right before
 * the nearest one below, else to the end of the column: next to what the
 * reader saw next to them.
 *
 * - `column`: every card of the column in the manual order.
 * - `shown`: the cards of it the page shows, in shown order.
 * - `index`: the place among `shown`.
 *
 * Both lists come **without** the moved cards.
 */
export function placeAmongAll(
  column: readonly string[],
  shown: readonly string[],
  index: number,
): number {
  const place = new Map(column.map((id, at) => [id, at]));
  for (let above = Math.min(index, shown.length) - 1; above >= 0; above -= 1) {
    const at = place.get(shown[above] ?? '');
    if (at !== undefined) return at + 1;
  }
  for (let below = Math.max(index, 0); below < shown.length; below += 1) {
    const at = place.get(shown[below] ?? '');
    if (at !== undefined) return at;
  }
  return column.length;
}
