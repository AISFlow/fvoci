/** Human words for the provider settings' RFC 9457 refusals. */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError } from '$lib/state/backend.ts';

/**
 * What the address says of a sign-in through the pending settings that came
 * back to the organization's page (`oidc_pending=applied`, or the name of what
 * went wrong in `oidc_pending_error`); undefined when it says nothing of one.
 */
export function pendingOutcome(
  address: URLSearchParams,
): { applied: boolean; words: string } | undefined {
  if (address.get('oidc_pending') === 'applied') {
    return { applied: true, words: m.org_sso_pending_applied() };
  }
  const name = address.get('oidc_pending_error');
  if (name === null) return undefined;
  // The provider's answer and the account are what a trial tests; the rest is said as a
  // sign-in says it.
  const said: Record<string, () => string> = {
    'oidc-token-refused': m.org_sso_pending_refused,
    'oidc-trial-account': m.org_sso_pending_other_account,
    'oidc-trial-stale': m.org_sso_pending_stale,
  };
  const why = (said[name] ?? m.account_oidc_failed)();
  return { applied: false, words: `${m.org_sso_pending_failed()} ${why}` };
}

export function oidcProblem(error: unknown): string {
  if (!isBackendError(error)) return m.org_sso_failed();
  switch (error.reason) {
    case 'oidc_issuer_unverified':
      return m.org_sso_issuer_unverified();
    case 'secret_key_required':
      return m.org_sso_key_required();
    case 'sso_unproven':
      return m.org_sso_unproven();
    case 'sso_enforced':
      return m.org_sso_enforced();
    case 'oidc_unavailable':
      return m.account_oidc_failed();
  }
  if (error.status === 400) return m.org_sso_invalid();
  if (error.status === 409) return m.org_sso_conflict();
  if (error.status === 404) return m.org_missing();
  return m.org_sso_failed();
}
