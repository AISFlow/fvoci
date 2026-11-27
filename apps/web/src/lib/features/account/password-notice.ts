/**
 * What the notice that asks an account without a password to set one says
 * of a refusal of `setPassword`. The words come from the refusal's status,
 * reason and the member `errors` points at, never from the server's detail.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError } from '$lib/state/index.ts';

export interface SetPasswordProblem {
  text: string;
  /** The password itself is refused: the field is where to go on. */
  field: boolean;
  /** The session is too old to vouch: a sign-in through the provider comes first. */
  again: boolean;
  /** The account has a password already: there is nothing left to ask. */
  had: boolean;
}

const problem = (text: string, of: Partial<SetPasswordProblem> = {}): SetPasswordProblem => ({
  text,
  field: false,
  again: false,
  had: false,
  ...of,
});

export function setPasswordProblemOf(error: unknown): SetPasswordProblem {
  if (!isBackendError(error)) return problem(m.account_failed());
  if (error.status === 403 && error.reason === 'sign_in_again') {
    return problem(m.password_notice_sign_in_again(), { again: true });
  }
  if (error.status === 409) return problem(m.password_notice_already(), { had: true });
  if (error.status === 429) return problem(m.account_rate_limited());
  const refused = error.status === 400 ? error.refusalAt('/password') : undefined;
  if (refused?.reason === 'too_long') {
    return problem(m.account_new_password_too_long(), { field: true });
  }
  if (refused) return problem(m.account_new_password_too_short(), { field: true });
  return problem(m.account_failed());
}
