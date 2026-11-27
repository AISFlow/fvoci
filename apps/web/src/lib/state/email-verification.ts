import type { AcceptedInvitation } from './account-types.ts';

/** Who a request for a new confirmation link names: the user name or the address. */
export interface WaitingAccount {
  username?: string;
  email?: string;
}

/** Where a confirmed registration's account was let in. */
export interface Confirmed extends AcceptedInvitation {
  /** Whether it started its organization (it gets a welcome board), or joined one by a link. */
  organizationStarted: boolean;
}

/**
 * Confirming the address of a registration, before there is an account:
 * where the server asks for it (`emailVerification` of the login options), a
 * registration makes no account until the link mailed to its address does.
 */
export interface EmailVerification {
  /**
   * Mails the registration a new link, which ends the one before. Answered
   * the same whether or not a registration waits under that name.
   */
  resend(waiting: WaitingAccount, language: string): Promise<void>;
  /**
   * Confirms by the link's token: makes the account, lets it in and signs this
   * browser in. A token that confirms nothing is refused with 400 naming
   * `/token`. One that would is enough in the browser the registration was
   * made from (its cookie shows it); anywhere else it is refused with 403
   * `password_required` until `password`, the one the registration was made
   * with, comes with it (a wrong one is 401 `invalid_credentials`, as a login's).
   */
  complete(token: string, password?: string): Promise<Confirmed>;
  /**
   * Makes the welcome board of the first workspace of an organization the
   * confirmation started, as a new account's first sign-in does.
   */
  welcome(workspaceId: string): Promise<string>;
}
