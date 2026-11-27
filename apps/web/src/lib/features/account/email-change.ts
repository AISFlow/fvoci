/**
 * What the form that changes the account's address says about a refusal, and
 * which field it is about. The words come from the refusal's status, reason
 * and the member `errors` points at, never from the server's detail.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError } from '$lib/state/index.ts';

export interface EmailChangeProblem {
  text: string;
  /** The field to point at, if the refusal is about one. */
  field: 'email' | 'password' | undefined;
}

/** `asked` is the address typed, `current` the one the account has. */
export function emailChangeProblemOf(
  error: unknown,
  asked: string,
  current: string,
): EmailChangeProblem {
  if (!isBackendError(error)) return { text: m.account_failed(), field: undefined };
  if (error.reason === 'invalid_credentials') {
    return { text: m.account_password_wrong(), field: 'password' };
  }
  if (error.status === 429) return { text: m.account_rate_limited(), field: undefined };
  // Only a server that mails no link says whose an address is.
  if (error.reason === 'email_taken') return { text: m.account_email_taken(), field: 'email' };
  if (error.reason === 'password_not_set') {
    return { text: m.email_change_no_password(), field: undefined };
  }
  if (error.status === 400 && error.refusalAt('/email')) {
    const same = asked.trim().toLowerCase() === current.toLowerCase();
    return { text: same ? m.email_change_same() : m.account_email_invalid(), field: 'email' };
  }
  return { text: m.account_failed(), field: undefined };
}
