/** Signing in and out and making an account, where a backend has accounts (part of `backend.ts`). */
import type { ContainerKind } from '../logic/wire.ts';

import type { EmailChange } from './email-change.ts';
import type { ApiTokens } from './api-tokens.ts';
import type { OrgKind } from './workspace-types.ts';

/** What an account is made with. */
export interface NewAccount {
  /** The name to sign in with. */
  username: string;
  email: string;
  password: string;
  /** The name to be shown by; empty or left out for none. */
  displayName?: string;
}

/** An account that starts an organization: whoever signs up without an invitation. */
export interface Registration extends NewAccount {
  /**
   * The name of the organization the account starts, and of its first
   * workspace; left out for none: the account starts in its own space alone.
   */
  organizationName?: string;
}

/** What the link of an invitation shows of where it leads, to whoever holds it. */
export interface InvitationPreview {
  /** The name of the workspace the link leads into; empty if it was given none. */
  workspaceName: string;
  /** Who made the link: the name they are shown by, or the one they sign in with. */
  inviterName: string;
  /** For a guest: the title of the board or the wiki they are invited to. */
  containerTitle?: string;
  /** For a guest: which of the two it is. */
  containerKind?: ContainerKind;
}

/** Where an accepted invitation let somebody in. */
export interface AcceptedInvitation {
  workspaceId: string;
  /** For a guest: the board or the wiki they were invited to. */
  containerId?: string;
  /** For a guest: which of the two it is, for its own address. */
  containerKind?: ContainerKind;
}

/** What the login page may know of the server before anybody signs in. */
export interface LoginOptions {
  /** The organization the page is for, if it is for one. */
  organization?: LoginOrganization;
  /**
   * Whether somebody without an invitation may make an account and start an
   * organization. Where they may not, the link of an invitation is the way in.
   */
  registrationOpen: boolean;
  /** Whether the server can deliver recovery and invitation mail. */
  mailEnabled?: boolean;
  /** Whether a registration waits for the link mailed to its address before there is an account. */
  emailVerification?: boolean;
  /**
   * Whether a registration must name the organization it starts: until the
   * server has its first user, who starts the server's own. After, an
   * account may start in its own space alone.
   */
  organizationRequired?: boolean;
}

/**
 * What a registration made: the account, or nothing yet, as it waits for the
 * link mailed to its address (`EmailVerification`).
 */
export type Registered = 'made' | 'waiting';

/** What the login page may know of an organization before anybody signs in. */
export interface LoginOrganization {
  id: string;
  /** Empty if it was given none. */
  name: string;
  /** Whether people sign in through its provider. */
  oidc: boolean;
}

/** Who the reader is at a single sign-on they connected to the account, or first signed in through. */
export interface LinkedIdentity {
  /** The provider, as it names itself: an address. */
  issuer: string;
  /** Who the reader is there. */
  subject: string;
  /** The email address the provider gave when it was connected; empty for none. */
  email: string;
  createdAt: number;
  /** This session was made through a provider of this issuer: disconnecting it signs this session out. */
  madeThisSession: boolean;
}

/** An organization of the reader's that signs in through a provider, as the settings name it. */
export interface SignInOrganization {
  orgId: string;
  name: string;
}

/** An organization the deletion of the account is about, as the reader is told it. */
export interface LeavingOrganization {
  id: string;
  /** `personal` for the account's own space, which always goes with it. */
  kind: OrgKind;
  /** Empty if it was given none, as the own space always is. */
  name: string;
  /**
   * How much is in it, which goes with it. Absent when the session does not
   * open the organization: it takes its single sign-on, or it is the own space
   * of an account with a password and the session was not made with it.
   */
  boards?: number;
  wikis?: number;
  /** Only the own space has any. */
  diaries?: number;
  files?: number;
}

/** What deleting the reader's account comes to, as things are now. */
export interface AccountDeletion {
  /** The days an account waits between the asking and the deleting. */
  graceDays: number;
  /** What proves the account is the reader's: its password, or a sign-in made just before. */
  proof: 'password' | 'sign_in';
  /** The reader is the only administrator of the server: the account is not deleted. */
  lastInstanceAdmin: boolean;
  /**
   * Organizations that stay and have no other administrator (those with other
   * people, and the server's own first one): nor then.
   */
  soleAdminOf: { id: string; name: string }[];
  /** The other organizations that have nobody else: they are deleted with the account. */
  organizations: LeavingOrganization[];
  /** When the deletion was asked for and when it is due, once it was. */
  requestedAt?: number;
  deleteAfter?: number;
}

/**
 * Signing in and out, where a backend has accounts: the server has, the one
 * in memory has not. Until somebody is signed in, every read of a backend
 * with accounts is refused with 401.
 */
export interface Account {
  /** `login` is the name to sign in with, or the email address. */
  signIn(login: string, password: string): Promise<void>;
  /**
   * Makes an account and the organization it starts. It does not sign in.
   * Where the server confirms addresses it makes nothing yet (`waiting`): the
   * link mailed to the address, in `language`, does that and signs in.
   */
  register(registration: Registration, language?: string): Promise<Registered>;
  /**
   * What the link of an invitation shows. A link that lets nobody in (there is
   * none, it ended, it was used up or revoked) is refused with 404 and the
   * reason `invitation_unavailable`, whatever is wrong with it.
   */
  previewInvitation(token: string): Promise<InvitationPreview>;
  /**
   * Lets somebody in by the link of an invitation: whoever is signed in, or
   * the `account` it makes for somebody who is not. It does not sign in.
   */
  acceptInvitation(
    token: string,
    account?: NewAccount,
    language?: string,
  ): Promise<AcceptedInvitation | 'waiting'>;
  signOut(): Promise<void>;
  /**
   * Changes the reader's password; `oldPassword` proves they know it. Their
   * other sessions end, this one goes on. A wrong old password is 400 with
   * the reason `invalid_credentials`, too many tries 429.
   */
  changePassword(oldPassword: string, newPassword: string): Promise<void>;
  /**
   * Gives an account that has no password (`User.hasPassword`) its first:
   * from a session made through the provider in the last five minutes, else
   * 403 `sign_in_again`. Its other sessions end, this one goes on. An account
   * with a password is 409; a password too short or too long is 400.
   */
  setPassword(password: string): Promise<void>;
  /**
   * What the login page shows: the organization it is for (the one `orgId`
   * names, or the only one of a server that holds one), and whether an
   * account is made without an invitation.
   */
  loginOptions(orgId: string | undefined): Promise<LoginOptions>;
  /**
   * Starts a sign-in through the organization's provider for this browser:
   * the provider's address the browser is then sent to. With `linkPassword`,
   * the reader's password, it connects instead: the sign-in comes back to
   * tie who the provider says they are to the account they are signed in to.
   */
  startOidc(orgId: string, linkPassword?: string): Promise<string>;
  /**
   * For a sign-in the server holds for this browser while the organization
   * moves to another provider (the login page's `oidc_held`): starts a sign-in
   * through the provider the organization used before, the address the browser
   * is then sent to. It comes back signed in to the account the reader had,
   * with the new sign-in connected to it. 409 with `oidc_carry_over_refused`
   * when nothing is held for this browser, or the old provider is kept no more.
   */
  startOidcCarryOver(orgId: string): Promise<string>;
  /**
   * Ends the held sign-in in a new account instead, as a first sign-in through
   * the provider does; the session comes with the answer. 409 with the reason
   * a first sign-in is refused with, or `oidc_carry_over_refused`.
   */
  finishHeldSignIn(orgId: string): Promise<void>;
  /** The sign-ins connected to the reader's account, oldest first. */
  identities(): Promise<LinkedIdentity[]>;
  /**
   * Disconnects one; `password` proves the reader knows the account's. An
   * account without a password keeps its last (409, `last_sign_in`).
   */
  unlinkIdentity(identity: LinkedIdentity, password: string): Promise<void>;
  /**
   * What deleting the account comes to. The one read, with `cancelDeletion`
   * and `signOut`, that the session of an account waiting to be deleted opens.
   */
  deletion(): Promise<AccountDeletion>;
  /**
   * Asks for the account to be deleted: `username` is its name typed out,
   * `password` its password, empty for an account that has none, which
   * signed in through its provider just before (403, `sign_in_again`,
   * otherwise). Every session of the account is over with the answer, which
   * is when the account is deleted at the earliest. 409 with `last_admin` or
   * `last_org_admin` for the only administrator.
   */
  requestDeletion(username: string, password: string): Promise<number>;
  /**
   * Takes the account out of the organization `orgId` that people work in
   * together: out of its workspaces, boards and wikis; what it wrote stays,
   * and so do its sessions. 404 for one it is no member of (a guest of its
   * workspaces is none); 409 with `last_instance_admin`, `sole_org_admin` or
   * `alone_in_organization` for what stands in the way, and nothing changes.
   */
  leaveOrganization(orgId: string): Promise<void>;
  /** Takes the deletion back; the session is an ordinary one again. */
  cancelDeletion(): Promise<void>;
  /** Changing the account's address, which `Backend.me` then reads anew. */
  readonly emailChange: EmailChange;
  /** The personal API tokens the reader gives agents. */
  readonly apiTokens: ApiTokens;
}
