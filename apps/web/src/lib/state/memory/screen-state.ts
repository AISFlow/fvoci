/** The pocket of screen state of a reader in memory, checked as the server checks it. */
import { fail } from './refuse.ts';
import { LIMITS, SCREEN_STATE_KEY } from './shapes.ts';

/**
 * What a reader's screen state becomes: `current` without the keys of
 * `deleted`, with those of `updated`. A key the server would not take, a
 * value too long or too many keys are refused (400), and nothing changes.
 */
export function nextScreenState(
  current: Readonly<Record<string, string>>,
  updated: Readonly<Record<string, string>>,
  deleted: readonly string[],
): Record<string, string> {
  const next = Object.fromEntries(
    Object.entries(current).filter(([key]) => !deleted.includes(key)),
  );
  for (const [key, value] of Object.entries(updated)) {
    if (!SCREEN_STATE_KEY.test(key)) {
      fail(400, 'invalid key', {
        field: key,
        within: 'updatedFields',
        reason: 'invalid_characters',
      });
    }
    if (new TextEncoder().encode(value).length > LIMITS.screenStateValueBytes) {
      fail(400, 'value too long', {
        field: key,
        within: 'updatedFields',
        reason: 'too_long',
        limit: LIMITS.screenStateValueBytes,
      });
    }
    next[key] = value;
  }
  if (Object.keys(next).length > LIMITS.screenStateKeys) {
    fail(400, 'too many keys', { limit: LIMITS.screenStateKeys });
  }
  return next;
}
