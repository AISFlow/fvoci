/**
 * How long one in a trash stays there: the server deletes it for good once
 * the days it keeps a trash (`retentionDays`) have passed since it was deleted.
 */
const DAY = 24 * 60 * 60 * 1000;

/**
 * The days, begun ones counted whole, until one deleted at `deletedAt` goes
 * for good, as of `now`; 0 once its time is up, until the server takes it.
 */
export function trashDaysLeft(deletedAt: number, retentionDays: number, now: number): number {
  const end = deletedAt + retentionDays * DAY;
  return Math.max(0, Math.ceil((end - now) / DAY));
}
