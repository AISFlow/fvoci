/**
 * What the menu of a unit opens below its head, which stays out of the way
 * until it is set: the amount and dates of the unit, and a review planned for
 * it. Each names the unit it is open for, so moving to another unit closes it.
 */
export class UnitOpenings {
  /** The unit whose amount and dates are being set. */
  schedule = $state<string | undefined>(undefined);
  /** The unit a review is being planned for. */
  review = $state<string | undefined>(undefined);
}
