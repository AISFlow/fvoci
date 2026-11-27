/**
 * The words for what went wrong with a sign-in through a provider, by the
 * name the server puts in `oidc_error` of the address it sends the browser
 * back to. A sign-in that was to connect the provider to the account comes
 * back the same way.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

export function oidcProblemOf(name: string): string {
  switch (name) {
    case 'oidc-state-invalid':
      return m.account_oidc_state_invalid();
    case 'oidc-denied':
      return m.account_oidc_denied();
    case 'oidc-email-missing':
      return m.account_oidc_email_missing();
    case 'oidc-email-unverified':
      return m.account_oidc_email_unverified();
    case 'oidc-email-taken':
      return m.account_oidc_email_taken();
    case 'oidc-join-closed':
      return m.account_oidc_join_closed();
    case 'oidc-identity-taken':
      return m.account_oidc_identity_taken();
    case 'oidc-email-mismatch':
      return m.account_oidc_email_mismatch();
    case 'oidc-carry-over-refused':
      return m.account_oidc_carry_over_refused();
    default:
      return m.account_oidc_failed();
  }
}
