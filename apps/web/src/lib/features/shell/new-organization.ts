/**
 * What the dialog that starts an organization says of a refusal
 * (`createOrganization`). How many an account may start is the server's to say: the refusal
 * names the number (`limit`), and nothing here counts or knows it.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError } from '$lib/state/index.ts';

/** The words for the refusal, and whether they are about the name typed. */
export interface NewOrganizationProblem {
  text: string;
  ofName: boolean;
}

export function newOrganizationProblemOf(error: unknown): NewOrganizationProblem {
  if (!isBackendError(error)) return { text: m.org_new_failed(), ofName: false };
  if (error.reason === 'too_many_organizations' && error.limit !== undefined) {
    return { text: m.org_new_too_many({ limit: error.limit }), ofName: false };
  }
  if (error.reason === 'invitation_required') return { text: m.org_new_closed(), ofName: false };
  const name = error.errors.find((entry) => entry.pointer === '/name');
  if (name) return { text: nameProblemOf(name.reason), ofName: true };
  if (error.status === 429) return { text: m.account_rate_limited(), ofName: false };
  return { text: m.org_new_failed(), ofName: false };
}

/** The words for what is wrong with the name, read as a registration reads it. */
function nameProblemOf(reason: string): string {
  switch (reason) {
    case 'required':
      return m.account_field_required();
    case 'too_long':
      return m.account_organization_name_too_long();
    case 'invalid_characters':
      return m.account_organization_name_invalid_characters();
    default:
      return m.account_field_invalid();
  }
}
