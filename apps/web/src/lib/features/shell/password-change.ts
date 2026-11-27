/**
 * What the form that changes the password says about a refusal, and which
 * field it is about. The words come from the refusal's status, reason and
 * the member `errors` points at, never from the server's detail.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError } from '$lib/state/index.ts';

/** The server's bounds of a password: characters at least, UTF-8 bytes at most. */
export const PASSWORD_MIN_CHARACTERS = 8;
export const PASSWORD_MAX_BYTES = 1024;

export interface PasswordProblem {
  text: string;
  /** The field to point at, if the refusal is about one. */
  field: 'current' | 'next' | undefined;
}

export function passwordProblemOf(error: unknown, next: string): PasswordProblem {
  if (!isBackendError(error)) return { text: m.account_failed(), field: undefined };
  if (error.reason === 'invalid_credentials') {
    return { text: m.account_password_wrong(), field: 'current' };
  }
  if (error.status === 429) return { text: m.account_rate_limited(), field: undefined };
  if (error.status === 409) {
    return { text: m.account_password_changed_elsewhere(), field: undefined };
  }
  const refused = error.status === 400 ? error.refusalAt('/newPassword') : undefined;
  if (refused) {
    if (refused.reason === 'too_short' || characters(next) < PASSWORD_MIN_CHARACTERS) {
      return { text: m.account_new_password_too_short(), field: 'next' };
    }
    if (
      refused.reason === 'too_long' ||
      new TextEncoder().encode(next).length > PASSWORD_MAX_BYTES
    ) {
      return { text: m.account_new_password_too_long(), field: 'next' };
    }
  }
  return { text: m.account_failed(), field: undefined };
}

/** Characters as the server counts them: code points, not UTF-16 units. */
function characters(text: string): number {
  let count = 0;
  for (const _ of text) count += 1;
  return count;
}
