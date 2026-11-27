/** How the memory backend refuses: as the server answers, in the same shape. */
import { BackendError, problemOf, refusedMember, type ErrorStatus } from '../backend.ts';

/**
 * Rejects as the server does: with the status and the detail, and, where the
 * refusal is about a member of the request, that member in `errors` (with
 * the reason); a reason about no member is the problem's own (`last_admin`).
 */
export function fail(
  status: ErrorStatus,
  detail: string,
  extra: { field?: string; within?: string; reason?: string; limit?: number } = {},
): never {
  const { field, within, reason, limit } = extra;
  const entry = field === undefined ? undefined : refusedMember(field, detail, reason);
  // A member of a map the request carries is under the map's own member.
  if (entry && within !== undefined) entry.pointer = `/${within}${entry.pointer ?? ''}`;
  throw new BackendError(status, {
    ...problemOf(status, detail),
    ...(limit === undefined ? {} : { limit }),
    ...(entry === undefined ? {} : { errors: [entry] }),
    // A reason of the problem names its type, as on the server (`last_admin`: `/problems/last-admin`).
    ...(entry === undefined && reason !== undefined
      ? { reason, type: `/problems/${reason.replaceAll('_', '-')}` }
      : {}),
  });
}
