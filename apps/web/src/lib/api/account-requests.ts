/**
 * Account, authentication, invitation and workspace-list requests of the server
 * backend (`requests.ts`): pure HTTP calls that do not depend on a workspace path.
 */
import type { NameChange } from '../logic/person-names.ts';
import type { Registered } from '../state/account-types.ts';
import type {
  AcceptedInvitation,
  AccountDeletion,
  InvitationPreview,
  LinkedIdentity,
  LoginOptions,
  NewAccount,
  Registration,
  User,
  UserOrganization,
  WorkspaceInfo,
} from '../state/backend.ts';

import { userOf, workspacesOf, type Schemas } from './convert.ts';
import type { Http } from './http.ts';

/** The account as the server takes it: a display name that is empty is left out. */
function accountOf(account: NewAccount): Schemas['NewAccount'] {
  return {
    username: account.username,
    email: account.email,
    password: account.password,
    ...(account.displayName ? { displayName: account.displayName } : {}),
  };
}

export class AccountRequests {
  readonly #http: Http;
  #userId: string | undefined;

  constructor(http: Http) {
    this.#http = http;
  }

  /** Clears the cached signed-in user ID on sign-in or sign-out (§1.3.5). */
  protected forgetCachedUser(): void {
    this.#userId = undefined;
  }

  /** Returns the signed-in user's ID, reusing the last `me()` result when known (§1.3.5). */
  protected async currentUserId(): Promise<string> {
    if (this.#userId !== undefined) return this.#userId;
    return (await this.me()).id;
  }

  async register(registration: Registration, language = ''): Promise<Registered> {
    const request: Schemas['RegisterRequest'] = {
      ...accountOf(registration),
      // Left out for none: the account starts in its own space alone.
      ...(registration.organizationName ? { organizationName: registration.organizationName } : {}),
      // The language of the confirmation mail, where the server sends one.
      ...(language ? { language } : {}),
    };
    // 201 with the account, or 202 with nothing: the address is to be confirmed first.
    const made = await this.#http.request<Schemas['User'] | undefined>(
      '/api/v1/register',
      {},
      { method: 'POST', json: request },
    );
    return made === undefined ? 'waiting' : 'made';
  }

  /** The token goes in the body: an address would keep it in logs. */
  async previewInvitation(token: string): Promise<InvitationPreview> {
    const request: Schemas['InvitationToken'] = { token };
    const preview = await this.#http.request<Schemas['InvitationPreview']>(
      '/api/v1/invitations/preview',
      {},
      { method: 'POST', json: request },
    );
    return {
      workspaceName: preview.workspaceName,
      inviterName: preview.inviterName,
      ...(typeof preview.containerTitle === 'string'
        ? { containerTitle: preview.containerTitle }
        : {}),
      ...(preview.containerKind ? { containerKind: preview.containerKind } : {}),
    };
  }

  async acceptInvitation(
    token: string,
    account?: NewAccount,
    language = '',
  ): Promise<AcceptedInvitation | 'waiting'> {
    const request: Schemas['AcceptInvitation'] = {
      token,
      ...(language ? { language } : {}),
      ...(account ? { account: accountOf(account) } : {}),
    };
    // 202 with nothing: the new account waits for its address to be confirmed.
    const accepted = await this.#http.request<Schemas['AcceptedInvitation'] | undefined>(
      '/api/v1/invitations/accept',
      {},
      { method: 'POST', json: request },
    );
    if (accepted === undefined) return 'waiting';
    return {
      workspaceId: accepted.workspaceId,
      ...(accepted.containerId ? { containerId: accepted.containerId } : {}),
      ...(accepted.containerKind ? { containerKind: accepted.containerKind } : {}),
    };
  }

  async loginOptions(orgId: string | undefined): Promise<LoginOptions> {
    const options = await this.#http.request<Schemas['LoginOptions']>(
      '/api/v1/login-options',
      {},
      { query: { org: orgId } },
    );
    return {
      ...(options.organization ? { organization: options.organization } : {}),
      registrationOpen: options.registrationOpen,
      mailEnabled: options.mailEnabled,
      emailVerification: options.emailVerification,
      organizationRequired: options.organizationRequired,
    };
  }

  /**
   * Starts a sign-in through the organization's provider, as a request of this
   * page (which another site's page cannot send): the address to send the
   * browser to. The cookie that ties the sign-in to this browser comes with it.
   * With `linkPassword` it connects the provider to the reader's account.
   */
  async startOidc(orgId: string, linkPassword?: string): Promise<string> {
    const body: Schemas['OidcStartRequest'] | undefined =
      linkPassword === undefined ? undefined : { linkPassword };
    const started = await this.#http.request<Schemas['OidcStart']>(
      '/api/v1/auth/oidc/start',
      {},
      { method: 'POST', query: { org: orgId }, json: body },
    );
    return started.authorizationUrl;
  }

  async startOidcCarryOver(orgId: string): Promise<string> {
    const started = await this.#http.request<Schemas['OidcStart']>(
      '/api/v1/auth/oidc/held/carry-over',
      {},
      { method: 'POST', query: { org: orgId } },
    );
    return started.authorizationUrl;
  }

  async finishHeldSignIn(orgId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/auth/oidc/held/sign-in',
      {},
      { method: 'POST', query: { org: orgId } },
    );
  }

  async identities(): Promise<LinkedIdentity[]> {
    return this.#http.request<Schemas['LinkedIdentity'][]>('/api/v1/users/me/identities');
  }

  async deletion(): Promise<AccountDeletion> {
    return this.#http.request<Schemas['AccountDeletion']>('/api/v1/users/me/deletion');
  }

  async requestDeletion(username: string, password: string): Promise<number> {
    const request: Schemas['DeleteAccountRequest'] = password
      ? { username, password }
      : { username };
    const asked = await this.#http.request<Schemas['AccountDeletionAsked']>(
      '/api/v1/users/me/deletion',
      {},
      { method: 'POST', json: request },
    );
    return asked.deleteAfter;
  }

  async cancelDeletion(): Promise<void> {
    await this.#http.request('/api/v1/users/me/deletion', {}, { method: 'DELETE' });
  }

  async leaveOrganization(orgId: string): Promise<void> {
    await this.#http.request('/api/v1/org/{orgId}/leave', { orgId }, { method: 'POST' });
  }

  async unlinkIdentity(identity: LinkedIdentity, password: string): Promise<void> {
    const request: Schemas['UnlinkRequest'] = {
      issuer: identity.issuer,
      subject: identity.subject,
      password,
    };
    await this.#http.request(
      '/api/v1/users/me/identities',
      {},
      { method: 'DELETE', json: request },
    );
  }

  /** The reader, with what only they are told of their account: its address, whether it has a password. */
  async me(): Promise<User> {
    const me = await this.#http.request<Schemas['Me']>('/api/v1/users/me');
    this.#userId = me.id;
    const { email, pendingEmail, hasPassword } = me;
    return { ...userOf(me), email, pendingEmail: pendingEmail ?? undefined, hasPassword };
  }

  /**
   * Gives an account without a password its first one; the session goes on
   * under a new cookie. A session not made through the provider in the last
   * five minutes is 403 (`sign_in_again`), an account with one 409.
   */
  async setPassword(password: string): Promise<void> {
    const request: Schemas['SetPasswordRequest'] = { password };
    await this.#http.request(
      '/api/v1/users/me/password',
      {},
      { method: 'PUT', json: request, rotates: true },
    );
  }

  /** The session goes on under a new cookie, which the browser keeps by itself. */
  async changePassword(oldPassword: string, newPassword: string): Promise<void> {
    const request: Schemas['ChangePasswordRequest'] = { oldPassword, newPassword };
    await this.#http.request(
      '/api/v1/users/me/password',
      {},
      { method: 'POST', json: request, rotates: true },
    );
  }

  /**
   * The session goes on under a new cookie, and its connections stay: the
   * server asks them as the session is from then on, so what the own space
   * had refused is asked again on the connection the page has.
   */
  async confirmPassword(password: string): Promise<void> {
    const request: Schemas['ConfirmPasswordRequest'] = { password };
    const path = '/api/v1/users/me/password/confirm';
    await this.#http.request(path, {}, { method: 'POST', json: request, rotates: true });
  }

  async patchMe(patch: NameChange): Promise<User> {
    const change: Schemas['AccountChange'] = patch;
    return userOf(
      await this.#http.request<Schemas['User']>(
        '/api/v1/users/me',
        {},
        { method: 'PATCH', json: change },
      ),
    );
  }

  async workspaces(): Promise<WorkspaceInfo[]> {
    return workspacesOf(
      await this.#http.request<Schemas['UserWorkspace'][]>('/api/v1/users/me/workspaces'),
    );
  }

  async organizations(): Promise<UserOrganization[]> {
    return this.#http.request<Schemas['UserOrganization'][]>('/api/v1/users/me/organizations');
  }
}
