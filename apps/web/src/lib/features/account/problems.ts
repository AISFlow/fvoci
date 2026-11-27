/**
 * What the pages that sign in and make accounts say of a refusal: by its
 * status and reason, never in the server's own words (`Problem` in
 * `lib/state/backend.ts`). Where the server names the members it refuses
 * (`errors`), each field says what is wrong with it, beside itself; any other
 * refusal is one line above the button.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError, type InvalidInput } from '$lib/state/index.ts';

/** A field of the forms: what is typed to sign in, to start an organization, to make an account. */
export type AccountField =
  'login' | 'organizationName' | 'username' | 'displayName' | 'email' | 'password';

/** What is wrong with each field, where the server said so of the field. */
export type FieldProblems = Partial<Record<AccountField, string>>;

/** The fields in the order the forms show them, for finding the first that is wrong. */
const FIELD_ORDER: readonly AccountField[] = [
  'login',
  'organizationName',
  'username',
  'displayName',
  'email',
  'password',
];

/** The words for what is wrong with a field. */
function fieldProblemOf(entry: InvalidInput, field: AccountField): string {
  switch (entry.reason) {
    case 'required':
      return m.account_field_required();
    case 'too_short':
      return field === 'password' ? m.account_password_too_short() : m.account_field_invalid();
    case 'too_long':
      if (field === 'username') return m.account_username_too_long();
      if (field === 'displayName') return m.account_display_name_too_long();
      return field === 'organizationName'
        ? m.account_organization_name_too_long()
        : m.account_field_too_long();
    case 'invalid_characters':
      if (field === 'username') return m.account_username_invalid_characters();
      return field === 'organizationName'
        ? m.account_organization_name_invalid_characters()
        : m.account_display_name_invalid_characters();
    case 'invalid_format':
      return field === 'email' ? m.account_email_invalid() : m.account_field_invalid();
    case 'username_taken':
      return m.account_username_taken();
    case 'email_taken':
      return m.account_email_taken();
    default:
      return m.account_field_invalid();
  }
}

/** The words for a refusal that is about no field. */
export function problemOf(error: unknown): string {
  if (!isBackendError(error)) return m.account_failed();
  switch (error.reason) {
    case 'invalid_credentials':
      return m.account_invalid_credentials();
    case 'email_unverified':
      return m.account_email_unverified();
    case 'session_not_kept':
      return m.account_session_not_kept();
    case 'username_taken':
      return m.account_username_taken();
    case 'email_taken':
      return m.account_email_taken();
    case 'invitation_required':
      return m.account_invitation_required();
    case 'invitation_unavailable':
      return m.invite_unavailable_title();
    default:
      break;
  }
  if (error.status === 429) return m.account_rate_limited();
  if (error.status === 401) return m.account_invalid_credentials();
  return m.account_failed();
}

/** What a refused request leaves a form to say. */
export interface FormProblems {
  /** The first word about each field the server named. */
  fields: FieldProblems;
  /** The one line for the whole form, if there is one. */
  problem: string | undefined;
  /** The first field that is wrong, in the order of the form: where to go on. */
  first: AccountField | undefined;
}

/**
 * What a refusal says to a form whose fields the members of its request are
 * typed into: `fieldOf` has the field by the member's JSON Pointer.
 */
export function formProblemsOf(
  error: unknown,
  fieldOf: Readonly<Record<string, AccountField>>,
): FormProblems {
  const fields: FieldProblems = {};
  let other = false;
  if (isBackendError(error)) {
    for (const entry of error.errors) {
      const field = entry.pointer === undefined ? undefined : fieldOf[entry.pointer];
      if (field) fields[field] ??= fieldProblemOf(entry, field);
      else other = true;
    }
  }
  const first = FIELD_ORDER.find((field) => fields[field] !== undefined);
  // One line for the whole form when no field has a word of its own, and
  // when something was refused that is no field: the fields are not all of it.
  let problem: string | undefined;
  if (first === undefined) problem = problemOf(error);
  else if (other) problem = m.account_failed();
  return { fields, problem, first };
}

/** The fields of a new account by the members of the account, under `prefix` in the request. */
export function accountFieldsUnder(prefix: string): Record<string, AccountField> {
  return {
    [`${prefix}/username`]: 'username',
    [`${prefix}/displayName`]: 'displayName',
    [`${prefix}/email`]: 'email',
    [`${prefix}/password`]: 'password',
  };
}
