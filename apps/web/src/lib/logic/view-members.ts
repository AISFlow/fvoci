/**
 * The members of a view's `fields` that a view has only where it was given
 * a value for them: what it is grouped by, the date it is placed by, and,
 * for a Gantt view, the card relation it reads and its zoom.
 *
 * A view is copied from what was stored member by member (wire.ts,
 * `createBoardView`). These members are copied only where they are there: a
 * member that is present with no value is not "missing" to a patch, and the
 * patch that takes a first choice back (an undo) would name nothing to drop
 * and send nothing, so that the server kept the choice.
 */

/** How far in a Gantt view is zoomed: what one mark of its axis is. */
export type GanttZoom = 'day' | 'week' | 'month';

/** What a Gantt view has beside the members every view has. */
export interface GanttViewFields {
  /**
   * The card relation property whose value on a card names the cards that
   * come before it. Missing or empty: no card comes after another.
   */
  dependencyPropertyId?: string;
  /** The zoom. Missing, or a word this version does not know: `week`. */
  ganttZoom?: GanttZoom;
}

export interface OptionalViewFields extends GanttViewFields {
  groupBy?: string;
  dateProperty?: string;
  /**
   * The view's place among the views of its board (`logic/list-places.ts`).
   * Missing, or no place: the view comes after those with one, by title.
   */
  order?: string;
}

/** Every optional member, by name: one that is added to the type and not here does not compile. */
const OPTIONAL: Record<keyof OptionalViewFields, true> = {
  groupBy: true,
  dateProperty: true,
  dependencyPropertyId: true,
  ganttZoom: true,
  order: true,
};

/** Those of the optional members that `fields` has a value for, and no others. */
export function optionalViewFields(fields: OptionalViewFields): OptionalViewFields {
  const members = Object.keys(OPTIONAL) as (keyof OptionalViewFields)[];
  return Object.fromEntries(
    members.flatMap((member) => (fields[member] === undefined ? [] : [[member, fields[member]]])),
  );
}
