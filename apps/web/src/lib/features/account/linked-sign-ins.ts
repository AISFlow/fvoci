/**
 * What the connected sign-ins of the settings say about a refusal, and how
 * they name an identity: by the host of its issuer, which is what a reader
 * knows their single sign-on by.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError } from '$lib/state/index.ts';

/** The words for a refusal of connecting or disconnecting, by its status and reason. */
export function linkProblemOf(error: unknown): { text: string; password: boolean } {
  if (!isBackendError(error)) return { text: m.account_failed(), password: false };
  if (error.reason === 'invalid_credentials') {
    return { text: m.account_password_wrong(), password: true };
  }
  if (error.status === 429) return { text: m.account_rate_limited(), password: false };
  if (error.reason === 'password_not_set') {
    return { text: m.account_linked_no_password(), password: false };
  }
  if (error.reason === 'last_sign_in') return { text: m.account_linked_last(), password: false };
  if (error.reason === 'oidc_unavailable')
    return { text: m.account_oidc_failed(), password: false };
  return { text: m.account_failed(), password: false };
}

/** The host of an issuer, or the issuer as it is when it is no address. */
export function issuerHost(issuer: string): string {
  try {
    return new URL(issuer).host || issuer;
  } catch {
    return issuer;
  }
}
